use crate::{
    config::{copilot_base_url, copilot_headers},
    errors::{AppError, AppResult},
    state::{ModelsResponse, RuntimeState},
};

pub async fn get_models(
    http: &reqwest::Client,
    state: &RuntimeState,
) -> AppResult<ModelsResponse> {
    let url = format!("{}/models", copilot_base_url(state));
    let resp = http
        .get(url)
        .headers(super::github::headers_map(copilot_headers(state, false)))
        .send()
        .await
        .map_err(|e| AppError::Upstream(format!("Models request failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!("Failed to get models: {}", text)));
    }

    resp.json::<ModelsResponse>()
        .await
        .map_err(|e| AppError::Upstream(format!("Models parse failed: {}", e)))
}
