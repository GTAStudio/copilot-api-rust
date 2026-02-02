use crate::errors::{ApiError, ApiResult};

fn openai_base_url() -> String {
    std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
}

fn openai_api_key() -> ApiResult<String> {
    std::env::var("OPENAI_API_KEY")
        .map_err(|_| ApiError::BadRequest("Missing OPENAI_API_KEY".to_string()))
}

pub async fn create_chat_completions(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = openai_api_key()?;
    let url = format!("{}/chat/completions", openai_base_url());
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI chat completions failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("OpenAI chat completions failed: {text}")));
    }

    Ok(resp)
}

pub async fn create_responses(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = openai_api_key()?;
    let url = format!("{}/responses", openai_base_url());
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI responses failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("OpenAI responses failed: {text}")));
    }

    Ok(resp)
}

pub async fn create_embeddings(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = openai_api_key()?;
    let url = format!("{}/embeddings", openai_base_url());
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI embeddings failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("OpenAI embeddings failed: {text}")));
    }

    Ok(resp)
}

pub async fn list_models(client: &reqwest::Client) -> ApiResult<serde_json::Value> {
    let key = openai_api_key()?;
    let url = format!("{}/models", openai_base_url());
    let resp = client
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI models failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("OpenAI models failed: {text}")));
    }

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid OpenAI models response: {e}")))
}
