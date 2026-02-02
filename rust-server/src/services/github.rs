use serde::{Deserialize, Serialize};

use crate::{
    config::{apply_headers, GITHUB_API_BASE_URL, GITHUB_BASE_URL, GITHUB_CLIENT_ID, GITHUB_APP_SCOPES, github_headers, standard_headers},
    errors::{ApiError, ApiResult},
    state::AppConfig,
    utils::sleep_ms,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CopilotTokenResponse {
    pub token: String,
    pub refresh_in: u64,
    pub expires_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubUser {
    pub login: String,
}

pub async fn get_device_code(client: &reqwest::Client) -> ApiResult<DeviceCodeResponse> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, standard_headers());

    let resp = client
        .post(format!("{GITHUB_BASE_URL}/login/device/code"))
        .headers(headers)
        .json(&serde_json::json!({
            "client_id": GITHUB_CLIENT_ID,
            "scope": GITHUB_APP_SCOPES,
        }))
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get device code: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to get device code: {text}")));
    }

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid device code response: {e}")))
}

pub async fn poll_access_token(
    client: &reqwest::Client,
    device: &DeviceCodeResponse,
) -> ApiResult<String> {
    let sleep_duration = (device.interval + 1) * 1000;

    loop {
        let mut headers = reqwest::header::HeaderMap::new();
        apply_headers(&mut headers, standard_headers());

        let resp = client
            .post(format!("{GITHUB_BASE_URL}/login/oauth/access_token"))
            .headers(headers)
            .json(&serde_json::json!({
                "client_id": GITHUB_CLIENT_ID,
                "device_code": device.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .map_err(|e| ApiError::Upstream(format!("Failed to poll access token: {e}")))?;

        if resp.status().is_success() {
            let json = resp
                .json::<AccessTokenResponse>()
                .await
                .map_err(|e| ApiError::Upstream(format!("Invalid access token response: {e}")))?;

            if let Some(token) = json.access_token {
                return Ok(token);
            }
        }

        sleep_ms(sleep_duration).await;
    }
}

pub async fn get_copilot_token(
    client: &reqwest::Client,
    config: &AppConfig,
    github_token: &str,
) -> ApiResult<CopilotTokenResponse> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, github_headers(config, github_token));

    let resp = client
        .get(format!("{GITHUB_API_BASE_URL}/copilot_internal/v2/token"))
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get Copilot token: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to get Copilot token: {text}")));
    }

    resp.json::<CopilotTokenResponse>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid Copilot token response: {e}")))
}

pub async fn get_github_user(
    client: &reqwest::Client,
    config: &AppConfig,
    github_token: &str,
) -> ApiResult<GitHubUser> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, github_headers(config, github_token));

    let resp = client
        .get(format!("{GITHUB_API_BASE_URL}/user"))
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to fetch user: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to fetch user: {text}")));
    }

    resp.json::<GitHubUser>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid user response: {e}")))
}

pub async fn get_copilot_usage(
    client: &reqwest::Client,
    config: &AppConfig,
    github_token: &str,
) -> ApiResult<serde_json::Value> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, github_headers(config, github_token));

    let resp = client
        .get(format!("{GITHUB_API_BASE_URL}/copilot_internal/user"))
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get Copilot usage: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to get Copilot usage: {text}")));
    }

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid usage response: {e}")))
}
