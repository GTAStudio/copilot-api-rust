use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use serde::{Deserialize, Serialize};

use crate::{
    config::{apply_headers, copilot_base_url, copilot_headers},
    errors::{ApiError, ApiResult},
    state::{AppConfig, ModelsResponse},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionsPayload {
    pub messages: Vec<Message>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Tool {
    pub r#type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponsesPayload {
    pub model: String,
    pub input: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingRequest {
    pub input: serde_json::Value,
    pub model: String,
}

pub async fn create_embeddings(
    client: &reqwest::Client,
    config: &AppConfig,
    copilot_token: &str,
    payload: &EmbeddingRequest,
) -> ApiResult<reqwest::Response> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, copilot_headers(config, copilot_token, false));

    let resp = client
        .post(format!("{}/embeddings", copilot_base_url(config)))
        .headers(headers)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to create embeddings: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to create embeddings: {text}")));
    }

    Ok(resp)
}

pub async fn get_models(
    client: &reqwest::Client,
    config: &AppConfig,
    copilot_token: &str,
) -> ApiResult<ModelsResponse> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, copilot_headers(config, copilot_token, false));

    let resp = client
        .get(format!("{}/models", copilot_base_url(config)))
        .headers(headers)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to get models: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to get models: {text}")));
    }

    resp.json::<ModelsResponse>()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid models response: {e}")))
}

pub async fn create_chat_completions(
    client: &reqwest::Client,
    config: &AppConfig,
    copilot_token: &str,
    payload: &ChatCompletionsPayload,
) -> ApiResult<reqwest::Response> {
    let enable_vision = payload.messages.iter().any(|msg| {
        msg.content
            .as_array()
            .map(|arr| arr.iter().any(|v| v.get("type") == Some(&serde_json::Value::String("image_url".to_string()))))
            .unwrap_or(false)
    });

    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, copilot_headers(config, copilot_token, enable_vision));

    let is_agent_call = payload
        .messages
        .iter()
        .any(|m| m.role == "assistant" || m.role == "tool");
    headers.insert(
        "X-Initiator",
        if is_agent_call { "agent" } else { "user" }.parse().unwrap(),
    );

    let resp = client
        .post(format!("{}/chat/completions", copilot_base_url(config)))
        .headers(headers)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to create chat completions: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to create chat completions: {text}")));
    }

    Ok(resp)
}

pub async fn create_responses(
    client: &reqwest::Client,
    config: &AppConfig,
    copilot_token: &str,
    payload: &ResponsesPayload,
) -> ApiResult<reqwest::Response> {
    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(&mut headers, copilot_headers(config, copilot_token, false));

    let resp = client
        .post(format!("{}/responses", copilot_base_url(config)))
        .headers(headers)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to create responses: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Failed to create responses: {text}")));
    }

    Ok(resp)
}

pub fn response_body_stream(resp: reqwest::Response) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    resp.bytes_stream().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}
