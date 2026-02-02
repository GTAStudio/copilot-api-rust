use crate::errors::{ApiError, ApiResult};

fn anthropic_base_url() -> String {
    std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

fn anthropic_api_key() -> ApiResult<String> {
    std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| ApiError::BadRequest("Missing ANTHROPIC_API_KEY".to_string()))
}

fn anthropic_version() -> String {
    std::env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string())
}

pub async fn create_messages(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = anthropic_api_key()?;
    let url = format!("{}/v1/messages", anthropic_base_url());

    let resp = client
        .post(url)
        .header("x-api-key", key)
        .header("anthropic-version", anthropic_version())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Anthropic messages failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Anthropic messages failed: {text}")));
    }

    Ok(resp)
}
