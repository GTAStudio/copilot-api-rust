use axum::{Json, extract::State, response::IntoResponse};

use crate::{
    errors::{ApiError, ApiResult},
    services::github::{get_device_code, poll_access_token},
    state::AppState,
    token_store::{read_github_token, write_github_credential},
};

pub async fn device_code(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let config = state.config.read().await.clone();
    let device = get_device_code(&state.client, &config).await?;
    Ok(Json(device))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollRequest {
    pub device_code: String,
    pub interval: u64,
}

pub async fn poll_token(
    State(state): State<AppState>,
    Json(payload): Json<PollRequest>,
) -> ApiResult<impl IntoResponse> {
    if payload.device_code.is_empty()
        || payload.device_code.len() > 1024
        || !(1..=60).contains(&payload.interval)
    {
        return Err(ApiError::BadRequest(
            "Invalid device code or polling interval".to_string(),
        ));
    }
    let device = crate::services::github::DeviceCodeResponse {
        device_code: payload.device_code,
        user_code: "".to_string(),
        verification_uri: "".to_string(),
        expires_in: 900,
        interval: payload.interval,
    };

    let config = state.config.read().await.clone();
    let credential = poll_access_token(&state.client, &config, &device).await?;
    crate::services::github::get_github_user(&state.client, &config, &credential.access_token)
        .await?;
    let refresh_lock = state.config.read().await.token_refresh_lock.clone();
    let _refresh = refresh_lock.lock().await;
    let github_lock = state.config.read().await.github_refresh_lock.clone();
    let _github_refresh = github_lock.lock().await;
    write_github_credential(&credential).await?;

    {
        let mut config = state.config.write().await;
        config.github_token = Some(credential.access_token);
        config.github_refresh_token = credential.refresh_token;
        config.github_token_expires_at = credential.expires_at;
        config.copilot_token = None;
        config.copilot_token_expires_at = None;
        config.copilot_base_url = None;
        config.models = None;
    }

    Ok(Json(serde_json::json!({ "authenticated": true })))
}

pub async fn status(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let authenticated =
        state.config.read().await.github_token.is_some() || read_github_token().await?.is_some();
    Ok(Json(serde_json::json!({ "authenticated": authenticated })))
}
