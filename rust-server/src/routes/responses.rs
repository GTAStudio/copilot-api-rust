use axum::{extract::State, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};

use crate::{
    approval::check_manual_approval,
    auth_flow::ensure_copilot_token,
    errors::{ApiError, ApiResult},
    rate_limit::check_rate_limit,
    services::{copilot::{create_responses, ResponsesPayload}, openai, azure},
    state::AppState,
};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResponsesInputItem {
    pub r#type: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub status: String,
    pub model: String,
    pub output: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<serde_json::Value>,
}

pub async fn handle(State(state): State<AppState>, Json(payload): Json<ResponsesPayload>) -> ApiResult<Response> {
    check_manual_approval(&state).await?;
    check_rate_limit(&state).await?;
    let provider = std::env::var("COPILOT_PROVIDER").unwrap_or_else(|_| "copilot".to_string());
    if provider == "azure" || payload.model.starts_with("azure:") {
        if let Some(cfg) = azure::load_azure_config(&payload.model) {
            let mut azure_payload = payload.clone();
            if azure_payload.model.starts_with("azure:") {
                azure_payload.model = cfg.deployment.clone();
            }
            let resp = azure::create_responses(&state.client, &cfg, &serde_json::to_value(&azure_payload).unwrap()).await?;
            if payload.stream.unwrap_or(false) {
                let stream = crate::services::copilot::response_body_stream(resp);
                return Ok(crate::routes::streaming::sse_response(stream));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid Azure responses payload: {e}")))?;
            return Ok(Json(json).into_response());
        }
    }
    if provider == "openai" || payload.model.starts_with("openai:") {
        let mut payload = payload;
        if payload.model.starts_with("openai:") {
            payload.model = payload.model.trim_start_matches("openai:").to_string();
        }
        let resp = openai::create_responses(&state.client, &serde_json::to_value(&payload).unwrap()).await?;
        if payload.stream.unwrap_or(false) {
            let stream = crate::services::copilot::response_body_stream(resp);
            return Ok(crate::routes::streaming::sse_response(stream));
        }
        let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid OpenAI responses payload: {e}")))?;
        return Ok(Json(json).into_response());
    }

    let token = ensure_copilot_token(&state).await?;
    let config = state.config.read().await.clone();

    let resp = create_responses(&state.client, &config, &token, &payload).await?;

    if payload.stream.unwrap_or(false) {
        let stream = crate::services::copilot::response_body_stream(resp);
        return Ok(crate::routes::streaming::sse_response(stream));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid responses payload: {e}")))?;
    Ok(Json(json).into_response())
}

pub fn messages_to_responses_input(messages: &[crate::services::copilot::Message]) -> Vec<ResponsesInputItem> {
    let mut input = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => {}
            "user" => {
                if let Some(text) = msg.content.as_str() {
                    input.push(ResponsesInputItem {
                        r#type: "message".to_string(),
                        role: Some("user".to_string()),
                        content: Some(serde_json::Value::String(text.to_string())),
                        text: None,
                        id: None,
                        call_id: None,
                        name: None,
                        output: None,
                    });
                } else if msg.content.is_array() {
                    let text_parts: Vec<String> = msg
                        .content
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|p| {
                            if p.get("type") == Some(&serde_json::Value::String("text".to_string())) {
                                p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !text_parts.is_empty() {
                        input.push(ResponsesInputItem {
                            r#type: "message".to_string(),
                            role: Some("user".to_string()),
                            content: Some(serde_json::Value::String(text_parts.join("\n"))),
                            text: None,
                            id: None,
                            call_id: None,
                            name: None,
                            output: None,
                        });
                    }
                }
            }
            "assistant" => {
                if let Some(text) = msg.content.as_str() {
                    input.push(ResponsesInputItem {
                        r#type: "message".to_string(),
                        role: Some("assistant".to_string()),
                        content: Some(serde_json::Value::String(text.to_string())),
                        text: None,
                        id: None,
                        call_id: None,
                        name: None,
                        output: None,
                    });
                }

                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        input.push(ResponsesInputItem {
                            r#type: "function_call".to_string(),
                            id: Some(tc.id.clone()),
                            call_id: Some(tc.id.clone()),
                            name: Some(tc.function.name.clone()),
                            output: Some(tc.function.arguments.clone()),
                            role: None,
                            content: None,
                            text: None,
                        });
                    }
                }
            }
            "tool" => {
                let output = msg
                    .content
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| msg.content.to_string());
                input.push(ResponsesInputItem {
                    r#type: "function_call_output".to_string(),
                    call_id: msg.tool_call_id.clone(),
                    output: Some(output),
                    role: None,
                    content: None,
                    text: None,
                    id: None,
                    name: None,
                });
            }
            _ => {}
        }
    }

    input
}

pub fn extract_instructions(messages: &[crate::services::copilot::Message]) -> Option<String> {
    let system: Vec<String> = messages
        .iter()
        .filter(|m| m.role == "system")
        .filter_map(|m| m.content.as_str().map(|s| s.to_string()))
        .collect();

    if system.is_empty() {
        None
    } else {
        Some(system.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_instructions, messages_to_responses_input};
    use crate::services::copilot::{Message, ToolCall, ToolCallFunction};

    #[test]
    fn extracts_system_instructions_joined() {
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: serde_json::Value::String("one".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "system".to_string(),
                content: serde_json::Value::String("two".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let out = extract_instructions(&messages);
        assert_eq!(out.as_deref(), Some("one\n\ntwo"));
    }

    #[test]
    fn maps_messages_into_responses_input() {
        let messages = vec![
            Message {
                role: "system".to_string(),
                content: serde_json::Value::String("sys".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "user".to_string(),
                content: serde_json::Value::String("hello".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "user".to_string(),
                content: serde_json::json!([
                    {"type": "text", "text": "world"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,xx"}}
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "assistant".to_string(),
                content: serde_json::Value::String("assistant".to_string()),
                name: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    r#type: "function".to_string(),
                    function: ToolCallFunction {
                        name: "doit".to_string(),
                        arguments: "{\"a\":1}".to_string(),
                    },
                }]),
                tool_call_id: None,
            },
            Message {
                role: "tool".to_string(),
                content: serde_json::json!({"ok": true}),
                name: None,
                tool_calls: None,
                tool_call_id: Some("call-1".to_string()),
            },
        ];

        let out = messages_to_responses_input(&messages);
        assert_eq!(out.len(), 5);

        assert_eq!(out[0].role.as_deref(), Some("user"));
        assert_eq!(out[0].content.as_ref().and_then(|v| v.as_str()), Some("hello"));

        assert_eq!(out[1].role.as_deref(), Some("user"));
        assert_eq!(out[1].content.as_ref().and_then(|v| v.as_str()), Some("world"));

        assert_eq!(out[2].role.as_deref(), Some("assistant"));
        assert_eq!(out[2].content.as_ref().and_then(|v| v.as_str()), Some("assistant"));

        assert_eq!(out[3].r#type, "function_call");
        assert_eq!(out[3].name.as_deref(), Some("doit"));
        assert_eq!(out[3].call_id.as_deref(), Some("call-1"));

        assert_eq!(out[4].r#type, "function_call_output");
        assert_eq!(out[4].call_id.as_deref(), Some("call-1"));
        assert_eq!(out[4].output.as_deref(), Some("{\"ok\":true}"));
    }
}
