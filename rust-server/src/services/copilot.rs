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
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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
    #[serde(default)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
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
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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
#[serde(deny_unknown_fields)]
pub struct EmbeddingRequest {
    pub input: serde_json::Value,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
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

    crate::errors::check_upstream(resp).await
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

    crate::errors::check_upstream(resp)
        .await?
        .json::<ModelsResponse>()
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
            .map(|arr| {
                arr.iter().any(|v| {
                    v.get("type") == Some(&serde_json::Value::String("image_url".to_string()))
                })
            })
            .unwrap_or(false)
    });

    let mut headers = reqwest::header::HeaderMap::new();
    apply_headers(
        &mut headers,
        copilot_headers(config, copilot_token, enable_vision),
    );

    let is_agent_call = payload
        .messages
        .iter()
        .any(|m| m.role == "assistant" || m.role == "tool");
    headers.insert(
        "X-Initiator",
        if is_agent_call { "agent" } else { "user" }
            .parse()
            .unwrap(),
    );

    let resp = client
        .post(format!("{}/chat/completions", copilot_base_url(config)))
        .headers(headers)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Failed to create chat completions: {e}")))?;

    crate::errors::check_upstream(resp).await
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

    crate::errors::check_upstream(resp).await
}

pub async fn ensure_models(state: &crate::state::AppState, token: &str) -> ApiResult<()> {
    let config = state.config.read().await.clone();
    if config.models.is_none() {
        let models = get_models(&state.client, &config, token).await?;
        state.config.write().await.models = Some(models);
    }
    Ok(())
}

pub async fn create_messages(
    client: &reqwest::Client,
    config: &AppConfig,
    token: &str,
    payload: &serde_json::Value,
    incoming: &reqwest::header::HeaderMap,
) -> ApiResult<reqwest::Response> {
    let mut headers = crate::protocol::anthropic_headers(incoming)?;
    apply_headers(&mut headers, copilot_headers(config, token, false));
    let response = client
        .post(format!("{}/v1/messages", copilot_base_url(config)))
        .headers(headers)
        .json(payload)
        .send()
        .await
        .map_err(|_| ApiError::Upstream("Copilot Messages request failed".to_string()))?;
    crate::errors::check_upstream(response).await
}

pub fn response_body_stream(
    resp: reqwest::Response,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    resp.bytes_stream().map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_chat_options_survive_serialization() {
        let raw = serde_json::json!({
            "model": "gpt-5.4", "messages": [{"role": "user", "content": "hello"}],
            "max_completion_tokens": 100, "reasoning_effort": "high", "parallel_tool_calls": false,
            "stream_options": {"include_usage": true}
        });
        let payload: ChatCompletionsPayload =
            serde_json::from_value(raw.clone()).expect("chat payload");
        let result = serde_json::to_value(payload).expect("serialized chat");
        for field in [
            "max_completion_tokens",
            "reasoning_effort",
            "parallel_tool_calls",
            "stream_options",
        ] {
            assert_eq!(result[field], raw[field], "{field}");
        }
    }

    #[test]
    fn modern_responses_options_survive_serialization() {
        let raw = serde_json::json!({
            "model": "gpt-5.4", "input": "hello", "reasoning": {"effort": "high"},
            "text": {"format": {"type": "json_object"}}, "store": false, "include": ["reasoning.encrypted_content"]
        });
        let payload: ResponsesPayload =
            serde_json::from_value(raw.clone()).expect("responses payload");
        let result = serde_json::to_value(payload).expect("serialized responses");
        assert_eq!(result, raw);
    }

    #[test]
    fn assistant_tool_calls_allow_omitted_content() {
        let raw = serde_json::json!({"role": "assistant", "tool_calls": [{
            "id": "call_fixture", "type": "function", "function": {"name": "example", "arguments": "{}"}
        }]});
        let message: Message = serde_json::from_value(raw).expect("assistant tool call");
        assert!(message.content.is_null());
    }
}
