use axum::{Json, extract::State, response::IntoResponse};

use crate::{
    auth_flow::ensure_copilot_token,
    errors::{ApiError, ApiResult},
    services::{anthropic, azure, copilot::ensure_models, openai},
    state::{AppState, Model},
};

pub async fn list(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult<impl IntoResponse> {
    let provider = std::env::var("COPILOT_PROVIDER").unwrap_or_else(|_| "copilot".to_string());
    if provider == "anthropic" {
        return Ok(Json(anthropic::list_models(&state.client, &headers).await?));
    }
    if provider == "openai" {
        let models = openai::list_models(&state.client).await?;
        return Ok(Json(models));
    }

    if provider == "azure"
        && let Some(cfg) = azure::load_azure_config("azure:")
    {
        let model_id = format!("azure:{}", cfg.deployment);
        return Ok(Json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": model_id,
                    "object": "model",
                    "type": "model",
                    "created": 0,
                    "created_at": "1970-01-01T00:00:00Z",
                    "owned_by": "azure",
                    "display_name": "Azure OpenAI Deployment",
                }
            ],
            "has_more": false
        })));
    }

    let token = ensure_copilot_token(&state).await?;

    ensure_models(&state, &token).await?;
    let config = state.config.read().await;
    let models = config
        .models
        .as_ref()
        .ok_or_else(|| ApiError::Upstream("Model catalogue unavailable".to_string()))?;
    let data: Vec<serde_json::Value> = models
        .data
        .iter()
        .filter(|model| {
            !model.id.is_empty()
                && !model
                    .policy
                    .as_ref()
                    .is_some_and(|policy| policy.state == "disabled")
        })
        .map(model_to_openai)
        .collect();

    Ok(Json(serde_json::json!({
        "object": "list",
        "first_id": data.first().and_then(|model| model.get("id")),
        "last_id": data.last().and_then(|model| model.get("id")),
        "data": data,
        "has_more": false,
    })))
}

fn model_to_openai(model: &Model) -> serde_json::Value {
    let mut result = serde_json::json!({
        "id": model.id,
        "object": "model",
        "type": "model",
        "created": 0,
        "created_at": "1970-01-01T00:00:00Z",
        "owned_by": model.vendor,
        "display_name": if model.name.is_empty() { &model.id } else { &model.name },
        "supported_endpoints": model.supported_endpoints,
    });
    if let Some(limit) = model
        .capabilities
        .limits
        .max_prompt_tokens
        .or(model.capabilities.limits.max_context_window_tokens)
    {
        result["max_input_tokens"] = limit.into();
    }
    if let Some(limit) = model.capabilities.limits.max_output_tokens {
        result["max_output_tokens"] = limit.into();
    }
    if model.id.starts_with("claude-")
        && let Some(tier) = ["opus", "sonnet", "haiku"]
            .into_iter()
            .find(|tier| model.id.contains(tier))
    {
        result["anthropic_family_tier"] = tier.into();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{Model, model_to_openai};

    #[test]
    fn minimal_metadata_preserves_model_identity() {
        let model: Model =
            serde_json::from_value(serde_json::json!({"id": "future-model"})).expect("metadata");
        let value = model_to_openai(&model);
        assert_eq!(value["id"], "future-model");
        assert_eq!(value["display_name"], "future-model");
        assert!(value.get("max_input_tokens").is_none());
        assert!(value.get("anthropic_family_tier").is_none());
    }

    #[test]
    fn anthropic_model_tier_is_derived_from_actual_model() {
        let model: Model =
            serde_json::from_value(serde_json::json!({"id": "claude-opus-4.8"})).expect("metadata");
        assert_eq!(model_to_openai(&model)["anthropic_family_tier"], "opus");
    }
}
