use crate::errors::{ApiError, ApiResult};

fn openai_base_url() -> String {
    std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string())
}

fn openai_api_key() -> ApiResult<String> {
    crate::utils::required_api_key("OPENAI_API_KEY")
}

pub async fn create_chat_completions(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = openai_api_key()?;
    let url = crate::utils::api_url(&openai_base_url(), "chat/completions")?;
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI chat completions failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}

pub async fn create_responses(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = openai_api_key()?;
    let url = crate::utils::api_url(&openai_base_url(), "responses")?;
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI responses failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}

pub async fn create_embeddings(
    client: &reqwest::Client,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let key = openai_api_key()?;
    let url = crate::utils::api_url(&openai_base_url(), "embeddings")?;
    let resp = client
        .post(url)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI embeddings failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}

pub async fn list_models(client: &reqwest::Client) -> ApiResult<serde_json::Value> {
    let key = openai_api_key()?;
    let url = crate::utils::api_url(&openai_base_url(), "models")?;
    let resp = client
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("OpenAI models failed: {e}")))?;

    crate::errors::check_upstream(resp)
        .await?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid OpenAI models response: {e}")))
}
