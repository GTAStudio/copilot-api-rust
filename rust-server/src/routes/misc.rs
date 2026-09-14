use axum::{Json, extract::State, response::IntoResponse};

use crate::{
    approval::check_manual_approval,
    auth_flow::{ensure_copilot_token, ensure_github_token},
    errors::{ApiError, ApiResult},
    rate_limit::check_rate_limit,
    services::github::get_copilot_usage,
    services::{azure, copilot::EmbeddingRequest, openai},
    state::AppState,
};

pub async fn root() -> impl IntoResponse {
    "Server running"
}

pub async fn usage(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let github_token = ensure_github_token(&state).await?;
    let config = state.config.read().await.clone();
    let usage = get_copilot_usage(&state.client, &config, &github_token).await?;
    Ok(Json(usage))
}

pub async fn embeddings(
    State(state): State<AppState>,
    crate::protocol::ApiJson(payload): crate::protocol::ApiJson<EmbeddingRequest>,
) -> ApiResult<impl IntoResponse> {
    crate::protocol::validate_embedding(&payload)?;
    check_manual_approval(&state).await?;
    check_rate_limit(&state).await?;
    let provider = std::env::var("COPILOT_PROVIDER").unwrap_or_else(|_| "copilot".to_string());

    if (provider == "azure" || payload.model.starts_with("azure:"))
        && let Some(cfg) = azure::load_azure_config(&payload.model)
    {
        let mut azure_payload = payload.clone();
        if azure_payload.model.starts_with("azure:") {
            azure_payload.model = cfg.deployment.clone();
        }
        let resp = azure::create_embeddings(
            &state.client,
            &cfg,
            &serde_json::to_value(&azure_payload).unwrap(),
        )
        .await?;
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ApiError::Upstream(format!("Invalid Azure embeddings response: {e}")))?;
        return Ok(Json(json));
    }

    if provider == "openai" || payload.model.starts_with("openai:") {
        let mut payload = payload;
        if payload.model.starts_with("openai:") {
            payload.model = payload.model.trim_start_matches("openai:").to_string();
        }
        let resp =
            openai::create_embeddings(&state.client, &serde_json::to_value(&payload).unwrap())
                .await?;
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ApiError::Upstream(format!("Invalid OpenAI embeddings response: {e}")))?;
        return Ok(Json(json));
    }

    if provider != "copilot" {
        return Err(ApiError::BadRequest(
            "The configured provider does not support embeddings".to_string(),
        ));
    }
    let token = ensure_copilot_token(&state).await?;
    let config = state.config.read().await.clone();

    let resp =
        crate::services::copilot::create_embeddings(&state.client, &config, &token, &payload)
            .await?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid embeddings response: {e}")))?;
    Ok(Json(json))
}

#[cfg(test)]
mod tests {
    use super::root;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn root_is_alive() {
        let resp = root().await.into_response();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(bytes, "Server running");
    }
}
