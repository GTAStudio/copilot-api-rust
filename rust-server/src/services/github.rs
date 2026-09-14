use serde::{Deserialize, Serialize};

use crate::{
    config::{
        GITHUB_API_BASE_URL, GITHUB_APP_SCOPES, GITHUB_BASE_URL, GITHUB_CLIENT_ID, apply_headers,
        github_headers, standard_headers,
    },
    errors::{ApiError, ApiResult},
    state::AppConfig,
    token_store::GitHubCredential,
};

#[derive(Debug, Clone)]
pub struct GitHubEndpoints {
    pub web: String,
    pub api: String,
}

impl Default for GitHubEndpoints {
    fn default() -> Self {
        Self {
            web: GITHUB_BASE_URL.to_string(),
            api: GITHUB_API_BASE_URL.to_string(),
        }
    }
}

fn endpoint(base: &str, path: &str) -> ApiResult<url::Url> {
    let mut url = url::Url::parse(base)
        .map_err(|_| ApiError::Internal("Invalid GitHub endpoint configuration".to_string()))?;
    url.path_segments_mut()
        .map_err(|_| ApiError::Internal("Invalid GitHub endpoint configuration".to_string()))?
        .pop_if_empty()
        .extend(path.split('/'));
    Ok(url)
}

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
    error: Option<String>,
    interval: Option<u64>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

enum PollOutcome {
    Authorized(GitHubCredential),
    Pending(u64),
}

fn interpret_poll(
    response: AccessTokenResponse,
    interval: u64,
    now: u64,
) -> ApiResult<PollOutcome> {
    if let Some(error) = response.error.as_deref() {
        return match error {
            "authorization_pending" => Ok(PollOutcome::Pending(interval.max(1))),
            "slow_down" => Ok(PollOutcome::Pending(
                interval
                    .saturating_add(5)
                    .max(response.interval.unwrap_or(0))
                    .min(600),
            )),
            "expired_token" | "token_expired" => Err(ApiError::Unauthorized(
                "Device authorization expired. Start authentication again.".to_string(),
            )),
            "access_denied" => Err(ApiError::Unauthorized(
                "Device authorization was denied.".to_string(),
            )),
            _ => Err(ApiError::Unauthorized(
                "GitHub authorization failed. Start authentication again.".to_string(),
            )),
        };
    }
    let token = response
        .access_token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| ApiError::Upstream("Missing GitHub access token".to_string()))?;
    if response
        .token_type
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
        || response.expires_in == Some(0)
    {
        return Err(ApiError::Upstream(
            "Invalid GitHub token response".to_string(),
        ));
    }
    Ok(PollOutcome::Authorized(GitHubCredential {
        access_token: token,
        refresh_token: response
            .refresh_token
            .filter(|token| !token.trim().is_empty()),
        expires_at: response
            .expires_in
            .map(|seconds| now.saturating_add(seconds)),
    }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CopilotTokenResponse {
    pub token: String,
    #[serde(default)]
    pub refresh_in: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub endpoints: Option<CopilotEndpoints>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CopilotEndpoints {
    pub api: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubUser {
    pub login: String,
}

pub async fn get_device_code(
    client: &reqwest::Client,
    config: &AppConfig,
) -> ApiResult<DeviceCodeResponse> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, standard_headers());

    let resp = client
        .post(endpoint(&config.github_endpoints.web, "login/device/code")?)
        .headers(headers)
        .json(&serde_json::json!({
            "client_id": GITHUB_CLIENT_ID,
            "scope": GITHUB_APP_SCOPES,
        }))
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get device code: {e}")))?;

    crate::errors::check_upstream(resp)
        .await?
        .json::<DeviceCodeResponse>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid device code response: {e}")))
}

pub async fn poll_access_token(
    client: &reqwest::Client,
    config: &AppConfig,
    device: &DeviceCodeResponse,
) -> ApiResult<GitHubCredential> {
    if device.device_code.trim().is_empty()
        || device.device_code.len() > 1024
        || device.expires_in == 0
    {
        return Err(ApiError::BadRequest(
            "Invalid device authorization".to_string(),
        ));
    }
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(device.expires_in.min(900));
    let mut interval = device.interval.clamp(1, 60);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining <= std::time::Duration::from_secs(interval) {
            return Err(ApiError::Unauthorized(
                "Device authorization expired. Start authentication again.".to_string(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        let mut headers = reqwest::header::HeaderMap::new();
        apply_headers(&mut headers, standard_headers());

        let resp = client
            .post(endpoint(
                &config.github_endpoints.web,
                "login/oauth/access_token",
            )?)
            .timeout(
                deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .min(std::time::Duration::from_secs(30)),
            )
            .headers(headers)
            .json(&serde_json::json!({
                "client_id": GITHUB_CLIENT_ID,
                "device_code": device.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .map_err(|e| ApiError::Upstream(format!("Failed to poll access token: {e}")))?;

        let json = resp
            .json::<AccessTokenResponse>()
            .await
            .map_err(|_| ApiError::Upstream("Invalid access token response".to_string()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        match interpret_poll(json, interval, now)? {
            PollOutcome::Authorized(credential) => return Ok(credential),
            PollOutcome::Pending(next_interval) => interval = next_interval,
        }
    }
}

pub async fn refresh_access_token(
    client: &reqwest::Client,
    config: &AppConfig,
    refresh_token: &str,
) -> ApiResult<GitHubCredential> {
    let response = client.post(endpoint(&config.github_endpoints.web, "login/oauth/access_token")?)
        .timeout(std::time::Duration::from_secs(30))
        .header("accept", "application/json")
        .json(&serde_json::json!({
            "client_id": GITHUB_CLIENT_ID, "grant_type": "refresh_token", "refresh_token": refresh_token
        })).send().await.map_err(|_| ApiError::Upstream("GitHub credential refresh failed".to_string()))?;
    let response = response
        .json::<AccessTokenResponse>()
        .await
        .map_err(|_| ApiError::Upstream("Invalid GitHub credential response".to_string()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    match interpret_poll(response, 5, now)? {
        PollOutcome::Authorized(credential) => Ok(credential),
        PollOutcome::Pending(_) => Err(ApiError::Unauthorized(
            "GitHub credential refresh failed. Authenticate again.".to_string(),
        )),
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
        .get(endpoint(
            &config.github_endpoints.api,
            "copilot_internal/v2/token",
        )?)
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get Copilot token: {e}")))?;

    crate::errors::check_upstream(resp)
        .await?
        .json::<CopilotTokenResponse>()
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
        .get(endpoint(&config.github_endpoints.api, "user")?)
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to fetch user: {e}")))?;

    crate::errors::check_upstream(resp)
        .await?
        .json::<GitHubUser>()
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
        .get(endpoint(
            &config.github_endpoints.api,
            "copilot_internal/user",
        )?)
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get Copilot usage: {e}")))?;

    crate::errors::check_upstream(resp)
        .await?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid usage response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_polling_handles_pending_slowdown_and_terminal_errors() {
        let pending: AccessTokenResponse =
            serde_json::from_value(serde_json::json!({"error": "authorization_pending"}))
                .expect("pending");
        assert!(matches!(
            interpret_poll(pending, 5, 1000),
            Ok(PollOutcome::Pending(5))
        ));
        let slowdown: AccessTokenResponse =
            serde_json::from_value(serde_json::json!({"error": "slow_down", "interval": 15}))
                .expect("slowdown");
        assert!(matches!(
            interpret_poll(slowdown, 5, 1000),
            Ok(PollOutcome::Pending(15))
        ));
        for error in [
            "expired_token",
            "access_denied",
            "incorrect_device_code",
            "device_flow_disabled",
            "unknown_error",
        ] {
            let response = serde_json::from_value(serde_json::json!({"error": error}))
                .expect("error response");
            assert!(interpret_poll(response, 5, 1000).is_err());
        }
        let empty = serde_json::from_value(serde_json::json!({})).expect("empty");
        assert!(interpret_poll(empty, 5, 1000).is_err());
    }

    #[test]
    fn device_polling_retains_expiring_oauth_credentials() {
        let response: AccessTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "unit-test-access", "token_type": "bearer",
            "refresh_token": "unit-test-refresh", "expires_in": 3600
        }))
        .expect("token response");
        let PollOutcome::Authorized(credential) =
            interpret_poll(response, 5, 1000).expect("authorized")
        else {
            panic!("expected token");
        };
        assert_eq!(credential.access_token, "unit-test-access");
        assert_eq!(
            credential.refresh_token.as_deref(),
            Some("unit-test-refresh")
        );
        assert_eq!(credential.expires_at, Some(4600));
    }
}
