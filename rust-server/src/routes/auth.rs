use axum::{extract::State, response::IntoResponse, Json};

use crate::{
    errors::ApiResult,
    services::github::{get_device_code, poll_access_token},
    state::AppState,
    token_store::{read_github_token, write_github_token},
};

pub async fn device_code(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let device = get_device_code(&state.client).await?;
    Ok(Json(device))
}

#[derive(serde::Deserialize)]
pub struct PollRequest {
    pub device_code: String,
    pub interval: u64,
}

pub async fn poll_token(
    State(state): State<AppState>,
    Json(payload): Json<PollRequest>,
) -> ApiResult<impl IntoResponse> {
    let device = crate::services::github::DeviceCodeResponse {
        device_code: payload.device_code,
        user_code: "".to_string(),
        verification_uri: "".to_string(),
        expires_in: 0,
        interval: payload.interval,
    };

    let token = poll_access_token(&state.client, &device).await?;
    write_github_token(&token).await?;

    {
        let mut config = state.config.write().await;
        config.github_token = Some(token.clone());
    }

    Ok(Json(serde_json::json!({ "token": token })))
}

pub async fn current_token(State(_state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let token = read_github_token().await?;
    Ok(Json(serde_json::json!({ "token": token })))
}
