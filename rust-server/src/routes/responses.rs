use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
    approval::check_manual_approval,
    auth_flow::ensure_copilot_token,
    errors::{ApiError, ApiResult},
    hooks::types::HookInput,
    rate_limit::check_rate_limit,
    services::{
        azure,
        copilot::{ResponsesPayload, create_responses},
        openai,
    },
    state::AppState,
};

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ResponsesInputItem {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
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

pub async fn handle(
    State(state): State<AppState>,
    crate::protocol::ApiJson(payload): crate::protocol::ApiJson<ResponsesPayload>,
) -> ApiResult<Response> {
    crate::protocol::validate_responses(&payload)?;
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PreToolUse".to_string()),
            tool: Some("Responses".to_string()),
            tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
            tool_output: None,
            session_id: None,
        };
        let results = hooks.execute_event("PreToolUse", &input).await?;
        if results.iter().any(|r| r.exit_code != 0) {
            return Err(ApiError::BadRequest("Hook blocked request".to_string()));
        }
    }
    check_manual_approval(&state).await?;
    check_rate_limit(&state).await?;
    let provider = std::env::var("COPILOT_PROVIDER").unwrap_or_else(|_| "copilot".to_string());
    if (provider == "azure" || payload.model.starts_with("azure:"))
        && let Some(cfg) = azure::load_azure_config(&payload.model)
    {
        let mut azure_payload = payload.clone();
        if azure_payload.model.starts_with("azure:") {
            azure_payload.model = cfg.deployment.clone();
        }
        let resp = azure::create_responses(
            &state.client,
            &cfg,
            &serde_json::to_value(&azure_payload).unwrap(),
        )
        .await?;
        if payload.stream.unwrap_or(false) {
            let stream = crate::services::copilot::response_body_stream(resp);
            if let Some(hooks) = &state.hooks {
                let input = HookInput {
                    hook_type: Some("PostToolUse".to_string()),
                    tool: Some("Responses".to_string()),
                    tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                    tool_output: None,
                    session_id: None,
                };
                let _ = hooks.execute_event("PostToolUse", &input).await;
            }
            return Ok(crate::routes::streaming::sse_response(stream));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ApiError::Upstream(format!("Invalid Azure responses payload: {e}")))?;
        if let Some(hooks) = &state.hooks {
            let input = HookInput {
                hook_type: Some("PostToolUse".to_string()),
                tool: Some("Responses".to_string()),
                tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                tool_output: Some(json.clone()),
                session_id: None,
            };
            let _ = hooks.execute_event("PostToolUse", &input).await;
        }
        return Ok(Json(json).into_response());
    }
    if provider == "openai" || payload.model.starts_with("openai:") {
        let mut payload = payload;
        if payload.model.starts_with("openai:") {
            payload.model = payload.model.trim_start_matches("openai:").to_string();
        }
        let resp =
            openai::create_responses(&state.client, &serde_json::to_value(&payload).unwrap())
                .await?;
        if payload.stream.unwrap_or(false) {
            let stream = crate::services::copilot::response_body_stream(resp);
            if let Some(hooks) = &state.hooks {
                let input = HookInput {
                    hook_type: Some("PostToolUse".to_string()),
                    tool: Some("Responses".to_string()),
                    tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                    tool_output: None,
                    session_id: None,
                };
                let _ = hooks.execute_event("PostToolUse", &input).await;
            }
            return Ok(crate::routes::streaming::sse_response(stream));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ApiError::Upstream(format!("Invalid OpenAI responses payload: {e}")))?;
        if let Some(hooks) = &state.hooks {
            let input = HookInput {
                hook_type: Some("PostToolUse".to_string()),
                tool: Some("Responses".to_string()),
                tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                tool_output: Some(json.clone()),
                session_id: None,
            };
            let _ = hooks.execute_event("PostToolUse", &input).await;
        }
        return Ok(Json(json).into_response());
    }

    if provider != "copilot" {
        return Err(ApiError::BadRequest(
            "The configured provider does not support this Responses request".to_string(),
        ));
    }
    let token = ensure_copilot_token(&state).await?;
    let config = state.config.read().await.clone();

    let resp = create_responses(&state.client, &config, &token, &payload).await?;

    if payload.stream.unwrap_or(false) {
        let stream = crate::services::copilot::response_body_stream(resp);
        if let Some(hooks) = &state.hooks {
            let input = HookInput {
                hook_type: Some("PostToolUse".to_string()),
                tool: Some("Responses".to_string()),
                tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                tool_output: None,
                session_id: None,
            };
            let _ = hooks.execute_event("PostToolUse", &input).await;
        }
        return Ok(crate::routes::streaming::sse_response(stream));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid responses payload: {e}")))?;
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PostToolUse".to_string()),
            tool: Some("Responses".to_string()),
            tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
            tool_output: Some(json.clone()),
            session_id: None,
        };
        let _ = hooks.execute_event("PostToolUse", &input).await;
    }
    Ok(Json(json).into_response())
}

pub fn messages_to_responses_input(
    messages: &[crate::services::copilot::Message],
) -> Vec<ResponsesInputItem> {
    let mut input = Vec::new();
    for msg in messages {
        if matches!(msg.role.as_str(), "system" | "developer") {
            continue;
        }
        if msg.role == "tool" {
            input.push(ResponsesInputItem {
                r#type: "function_call_output".to_string(),
                call_id: msg.tool_call_id.clone(),
                output: Some(if msg.content.is_string() || msg.content.is_array() {
                    msg.content.clone()
                } else {
                    msg.content.to_string().into()
                }),
                ..Default::default()
            });
            continue;
        }
        if !msg.content.is_null() {
            let content = if let Some(parts) = msg.content.as_array() {
                serde_json::Value::Array(parts.iter().map(|part| {
                    match part.get("type").and_then(serde_json::Value::as_str) {
                        Some("text") => serde_json::json!({
                            "type": if msg.role == "assistant" { "output_text" } else { "input_text" },
                            "text": part["text"]
                        }),
                        Some("image_url") => {
                            let mut image = serde_json::json!({"type": "input_image", "image_url": part["image_url"]["url"]});
                            if let Some(detail) = part["image_url"].get("detail") { image["detail"] = detail.clone(); }
                            image
                        }
                        _ => part.clone(),
                    }
                }).collect())
            } else {
                msg.content.clone()
            };
            input.push(ResponsesInputItem {
                r#type: "message".to_string(),
                role: Some(msg.role.clone()),
                content: Some(content),
                ..Default::default()
            });
        }
        for call in msg.tool_calls.iter().flatten() {
            input.push(ResponsesInputItem {
                r#type: "function_call".to_string(),
                call_id: Some(call.id.clone()),
                name: Some(call.function.name.clone()),
                arguments: Some(call.function.arguments.clone()),
                ..Default::default()
            });
        }
    }
    input
}

pub fn extract_instructions(messages: &[crate::services::copilot::Message]) -> Option<String> {
    let system: Vec<String> = messages
        .iter()
        .filter(|message| matches!(message.role.as_str(), "system" | "developer"))
        .filter_map(|message| {
            message.content.as_str().map(str::to_string).or_else(|| {
                message.content.as_array().map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            })
        })
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
        assert_eq!(
            out[0].content.as_ref().and_then(|v| v.as_str()),
            Some("hello")
        );

        assert_eq!(out[1].role.as_deref(), Some("user"));
        let content = out[1].content.as_ref().expect("multimodal content");
        assert_eq!(
            content[0],
            serde_json::json!({"type": "input_text", "text": "world"})
        );
        assert_eq!(
            content[1],
            serde_json::json!({"type": "input_image", "image_url": "data:image/png;base64,xx"})
        );

        assert_eq!(out[2].role.as_deref(), Some("assistant"));
        assert_eq!(
            out[2].content.as_ref().and_then(|v| v.as_str()),
            Some("assistant")
        );

        assert_eq!(out[3].r#type, "function_call");
        assert_eq!(out[3].name.as_deref(), Some("doit"));
        assert_eq!(out[3].call_id.as_deref(), Some("call-1"));
        let function_call = serde_json::to_value(&out[3]).expect("function call");
        assert_eq!(function_call["arguments"], "{\"a\":1}");
        assert!(function_call.get("output").is_none());

        assert_eq!(out[4].r#type, "function_call_output");
        assert_eq!(out[4].call_id.as_deref(), Some("call-1"));
        assert_eq!(
            out[4].output.as_ref().and_then(serde_json::Value::as_str),
            Some("{\"ok\":true}")
        );
    }
}
