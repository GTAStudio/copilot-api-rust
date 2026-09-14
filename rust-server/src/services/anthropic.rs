use crate::errors::{ApiError, ApiResult};

fn anthropic_base_url() -> String {
    std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

fn anthropic_api_key() -> ApiResult<String> {
    crate::utils::required_api_key("ANTHROPIC_API_KEY")
}

fn anthropic_version() -> String {
    std::env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string())
}

fn configured_headers(
    incoming: &reqwest::header::HeaderMap,
) -> ApiResult<reqwest::header::HeaderMap> {
    let mut headers = crate::protocol::anthropic_headers(incoming)?;
    if !incoming.contains_key("anthropic-version") {
        headers.insert(
            "anthropic-version",
            anthropic_version()
                .parse()
                .map_err(|_| ApiError::BadRequest("Invalid ANTHROPIC_VERSION".to_string()))?,
        );
    }
    Ok(headers)
}

pub async fn create_messages(
    client: &reqwest::Client,
    incoming: &reqwest::header::HeaderMap,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    send(client, incoming, payload, "messages").await
}

pub async fn count_tokens(
    client: &reqwest::Client,
    incoming: &reqwest::header::HeaderMap,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    send(client, incoming, payload, "messages/count_tokens").await
}

pub async fn list_models(
    client: &reqwest::Client,
    incoming: &reqwest::header::HeaderMap,
) -> ApiResult<serde_json::Value> {
    let response = client
        .get(crate::utils::api_url(&anthropic_base_url(), "models")?)
        .headers(configured_headers(incoming)?)
        .header("x-api-key", anthropic_api_key()?)
        .send()
        .await
        .map_err(|_| ApiError::Upstream("Anthropic model discovery failed".to_string()))?;
    crate::errors::check_upstream(response)
        .await?
        .json()
        .await
        .map_err(|_| ApiError::Upstream("Invalid Anthropic model catalogue".to_string()))
}

async fn send(
    client: &reqwest::Client,
    incoming: &reqwest::header::HeaderMap,
    payload: &serde_json::Value,
    endpoint: &str,
) -> ApiResult<reqwest::Response> {
    let key = anthropic_api_key()?;
    let url = crate::utils::api_url(&anthropic_base_url(), endpoint)?;
    let headers = configured_headers(incoming)?;

    let resp = client
        .post(url)
        .headers(headers)
        .header("x-api-key", key)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Anthropic messages failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}
