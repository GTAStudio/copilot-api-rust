use crate::{
    config::{github_headers, GITHUB_API_BASE_URL},
    errors::{AppError, AppResult},
    state::RuntimeState,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CopilotUsageResponse {
    pub access_type_sku: String,
    pub analytics_tracking_id: String,
    pub assigned_date: String,
    pub can_signup_for_limited: bool,
    pub chat_enabled: bool,
    pub copilot_plan: String,
    pub organization_login_list: Vec<serde_json::Value>,
    pub organization_list: Vec<serde_json::Value>,
    pub quota_reset_date: String,
    pub quota_snapshots: serde_json::Value,
}

pub async fn get_copilot_usage(
    http: &reqwest::Client,
    state: &RuntimeState,
) -> AppResult<CopilotUsageResponse> {
    let url = format!("{}/copilot_internal/user", GITHUB_API_BASE_URL);
    let resp = http
        .get(url)
        .headers(super::github::headers_map(github_headers(state)))
        .send()
        .await
        .map_err(|e| AppError::Upstream(format!("Usage request failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!("Failed to get usage: {}", text)));
    }

    resp.json::<CopilotUsageResponse>()
        .await
        .map_err(|e| AppError::Upstream(format!("Usage parse failed: {}", e)))
}
