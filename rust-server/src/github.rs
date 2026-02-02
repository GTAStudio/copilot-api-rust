use crate::{
    config::{github_headers, GITHUB_API_BASE_URL, GITHUB_APP_SCOPES, GITHUB_BASE_URL, GITHUB_CLIENT_ID},
    errors::{AppError, AppResult},
    state::RuntimeState,
    utils::sleep_ms,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
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
    pub expires_at: u64,
    pub refresh_in: u64,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GithubUserResponse {
    pub login: String,
}

pub async fn get_device_code(http: &reqwest::Client) -> AppResult<DeviceCodeResponse> {
    let url = format!("{}/login/device/code", GITHUB_BASE_URL);
    let body = serde_json::json!({
        "client_id": GITHUB_CLIENT_ID,
        "scope": GITHUB_APP_SCOPES,
    });

    let resp = http
        .post(url)
        .headers(headers_map(crate::config::standard_headers()))
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Upstream(format!("Device code request failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!("Failed to get device code: {}", text)));
    }

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| AppError::Upstream(format!("Device code parse failed: {}", e)))
}

pub async fn poll_access_token(
    http: &reqwest::Client,
    device_code: &DeviceCodeResponse,
) -> AppResult<String> {
    let url = format!("{}/login/oauth/access_token", GITHUB_BASE_URL);
    let sleep_duration = (device_code.interval + 1) * 1000;

    loop {
        let body = serde_json::json!({
            "client_id": GITHUB_CLIENT_ID,
            "device_code": device_code.device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        });

        let resp = http
            .post(&url)
            .headers(headers_map(crate::config::standard_headers()))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Upstream(format!("Token poll failed: {}", e)))?;

        if !resp.status().is_success() {
            sleep_ms(sleep_duration).await;
            continue;
        }

        let json = resp
            .json::<AccessTokenResponse>()
            .await
            .map_err(|e| AppError::Upstream(format!("Token poll parse failed: {}", e)))?;

        if let Some(token) = json.access_token {
            return Ok(token);
        }

        sleep_ms(sleep_duration).await;
    }
}

pub async fn get_github_user(
    http: &reqwest::Client,
    state: &RuntimeState,
) -> AppResult<GithubUserResponse> {
    let url = format!("{}/user", GITHUB_API_BASE_URL);
    let resp = http
        .get(url)
        .headers(headers_map(github_headers(state)))
        .send()
        .await
        .map_err(|e| AppError::Upstream(format!("GitHub user failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!("Failed to get GitHub user: {}", text)));
    }

    resp.json::<GithubUserResponse>()
        .await
        .map_err(|e| AppError::Upstream(format!("GitHub user parse failed: {}", e)))
}

pub async fn get_copilot_token(
    http: &reqwest::Client,
    state: &RuntimeState,
) -> AppResult<CopilotTokenResponse> {
    let url = format!("{}/copilot_internal/v2/token", GITHUB_API_BASE_URL);
    let resp = http
        .get(url)
        .headers(headers_map(github_headers(state)))
        .send()
        .await
        .map_err(|e| AppError::Upstream(format!("Copilot token failed: {}", e)))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream(format!("Failed to get Copilot token: {}", text)));
    }

    resp.json::<CopilotTokenResponse>()
        .await
        .map_err(|e| AppError::Upstream(format!("Copilot token parse failed: {}", e)))
}

pub(crate) fn headers_map(headers: std::collections::HashMap<String, String>) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .expect("invalid header name");
        let value = reqwest::header::HeaderValue::from_str(&v).unwrap_or_default();
        map.insert(name, value);
    }
    map
}
