use axum::{extract::State, response::{IntoResponse, Response}, Json};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    approval::check_manual_approval,
    auth_flow::ensure_copilot_token,
    errors::{ApiError, ApiResult},
    hooks::types::HookInput,
    rate_limit::check_rate_limit,
    routes::responses::{extract_instructions, messages_to_responses_input},
    services::{
        anthropic,
        copilot::{create_chat_completions, create_responses, ChatCompletionsPayload, Message, Tool},
    },
    state::AppState,
};

#[derive(Debug, Deserialize, Serialize)]
pub struct AnthropicMessagesPayload {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: u32,
    #[serde(default)]
    pub system: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub tools: Option<Vec<AnthropicTool>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "role")]
pub enum AnthropicMessage {
    #[serde(rename = "user")]
    User(AnthropicUserMessage),
    #[serde(rename = "assistant")]
    Assistant(AnthropicAssistantMessage),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnthropicUserMessage {
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnthropicAssistantMessage {
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct AnthropicResponse {
    pub id: String,
    pub r#type: String,
    pub role: String,
    pub content: Vec<serde_json::Value>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: serde_json::Value,
}

pub async fn handle(State(state): State<AppState>, Json(payload): Json<AnthropicMessagesPayload>) -> ApiResult<Response> {
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PreToolUse".to_string()),
            tool: Some("AnthropicMessages".to_string()),
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

    if provider == "anthropic" || (payload.model.to_lowercase().starts_with("claude") && std::env::var("ANTHROPIC_API_KEY").is_ok()) {
        let resp = anthropic::create_messages(&state.client, &serde_json::to_value(&payload).unwrap()).await?;
        if payload.stream.unwrap_or(false) {
            let stream = crate::services::copilot::response_body_stream(resp);
            if let Some(hooks) = &state.hooks {
                let input = HookInput {
                    hook_type: Some("PostToolUse".to_string()),
                    tool: Some("AnthropicMessages".to_string()),
                    tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                    tool_output: None,
                    session_id: None,
                };
                let _ = hooks.execute_event("PostToolUse", &input).await;
            }
            return Ok(crate::routes::streaming::sse_response(stream));
        }
        let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid Anthropic response: {e}")))?;
        if let Some(hooks) = &state.hooks {
            let input = HookInput {
                hook_type: Some("PostToolUse".to_string()),
                tool: Some("AnthropicMessages".to_string()),
                tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                tool_output: Some(json.clone()),
                session_id: None,
            };
            let _ = hooks.execute_event("PostToolUse", &input).await;
        }
        return Ok(Json(json).into_response());
    }
    let resolved_model = resolve_model_alias(&payload.model);
    let token = ensure_copilot_token(&state).await?;

    if requires_responses_api(&resolved_model) {
        return handle_responses_api(state, payload, resolved_model).await;
    }

    let openai_payload = translate_to_openai(&payload);
    let config = state.config.read().await.clone();
    let resp = create_chat_completions(&state.client, &config, &token, &openai_payload).await?;

    if payload.stream.unwrap_or(false) {
        if let Some(hooks) = &state.hooks {
            let input = HookInput {
                hook_type: Some("PostToolUse".to_string()),
                tool: Some("AnthropicMessages".to_string()),
                tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                tool_output: None,
                session_id: None,
            };
            let _ = hooks.execute_event("PostToolUse", &input).await;
        }
        return Ok(stream_anthropic(resp));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid response: {e}")))?;
    let anthropic = translate_to_anthropic(&json, &payload.model);
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PostToolUse".to_string()),
            tool: Some("AnthropicMessages".to_string()),
            tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
            tool_output: Some(anthropic.clone()),
            session_id: None,
        };
        let _ = hooks.execute_event("PostToolUse", &input).await;
    }
    Ok(Json(anthropic).into_response())
}

pub async fn count_tokens(
    State(state): State<AppState>,
    Json(payload): Json<AnthropicMessagesPayload>,
) -> ApiResult<Response> {
    let openai_payload = translate_to_openai(&payload);

    let base = serde_json::to_string(&openai_payload)
        .map(|s| (s.len() as f64 / 4.0).ceil() as u64)
        .unwrap_or(1);

    let mut token_count = base;

    if let Some(tools) = &payload.tools {
        if !tools.is_empty() {
            let model = payload.model.to_lowercase();
            if model.starts_with("claude") {
                token_count = token_count.saturating_add(346);
            } else if model.starts_with("grok") {
                token_count = token_count.saturating_add(480);
            }
        }
    }

    let model = payload.model.to_lowercase();
    if model.starts_with("claude") {
        token_count = ((token_count as f64) * 1.15).round() as u64;
    } else if model.starts_with("grok") {
        token_count = ((token_count as f64) * 1.03).round() as u64;
    }

    if state.config.read().await.show_token {
        tracing::info!("Token count (heuristic): {}", token_count);
    }

    Ok(Json(serde_json::json!({ "input_tokens": token_count })).into_response())
}

async fn handle_responses_api(
    state: AppState,
    payload: AnthropicMessagesPayload,
    resolved_model: String,
) -> ApiResult<Response> {
    let token = ensure_copilot_token(&state).await?;
    let openai_payload = translate_to_openai(&payload);
    let instructions = extract_instructions(&openai_payload.messages);
    let input = messages_to_responses_input(&openai_payload.messages);

    if input.is_empty() {
        return Err(ApiError::BadRequest("No valid input messages".to_string()));
    }

    let responses_payload = crate::services::copilot::ResponsesPayload {
        model: resolved_model,
        input: serde_json::to_value(input).unwrap_or(serde_json::json!([])),
        instructions,
        max_output_tokens: openai_payload.max_tokens,
        temperature: openai_payload.temperature,
        top_p: openai_payload.top_p,
        stream: payload.stream,
        tools: openai_payload.tools.as_ref().map(|tools| {
            serde_json::Value::Array(
                tools
                    .iter()
                    .map(|t| serde_json::json!({
                        "type": "function",
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters,
                    }))
                    .collect(),
            )
        }),
        tool_choice: openai_payload.tool_choice,
        previous_response_id: None,
    };

    let config = state.config.read().await.clone();
    let resp = create_responses(&state.client, &config, &token, &responses_payload).await?;

    if payload.stream.unwrap_or(false) {
        return Ok(stream_anthropic_from_responses(resp, &payload.model));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid responses payload: {e}")))?;
    let anthropic = translate_responses_to_anthropic(&json, &payload.model);
    Ok(Json(anthropic).into_response())
}

fn translate_to_openai(payload: &AnthropicMessagesPayload) -> ChatCompletionsPayload {
    let messages = translate_messages(&payload.messages, payload.system.clone());
    ChatCompletionsPayload {
        model: resolve_model_alias(&payload.model),
        messages,
        max_tokens: Some(payload.max_tokens),
        stop: payload.stop_sequences.as_ref().map(|s| serde_json::to_value(s).unwrap()),
        stream: payload.stream,
        temperature: payload.temperature,
        top_p: payload.top_p,
        n: None,
        frequency_penalty: None,
        presence_penalty: None,
        logit_bias: None,
        logprobs: None,
        response_format: None,
        seed: None,
        tools: payload.tools.as_ref().map(|t| translate_tools(t)),
        tool_choice: payload.tool_choice.clone(),
        user: payload.metadata.as_ref().and_then(|m| m.get("user_id").and_then(|v| v.as_str()).map(|s| s.to_string())),
    }
}

fn translate_tools(tools: &Vec<AnthropicTool>) -> Vec<Tool> {
    tools
        .iter()
        .map(|t| Tool {
            r#type: "function".to_string(),
            function: crate::services::copilot::ToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect()
}

fn translate_messages(messages: &[AnthropicMessage], system: Option<serde_json::Value>) -> Vec<Message> {
    let mut out = Vec::new();

    if let Some(system) = system {
        if system.is_string() {
            out.push(Message {
                role: "system".to_string(),
                content: system,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            });
        } else if let Some(arr) = system.as_array() {
            let text = arr
                .iter()
                .filter_map(|v| v.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n\n");
            out.push(Message {
                role: "system".to_string(),
                content: serde_json::Value::String(text),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }

    for msg in messages {
        match msg {
            AnthropicMessage::User(m) => out.extend(handle_user_message(m)),
            AnthropicMessage::Assistant(m) => out.extend(handle_assistant_message(m)),
        }
    }

    out
}

fn handle_user_message(message: &AnthropicUserMessage) -> Vec<Message> {
    if let Some(arr) = message.content.as_array() {
        let tool_results: Vec<&serde_json::Value> = arr.iter().filter(|b| b.get("type") == Some(&serde_json::Value::String("tool_result".to_string()))).collect();
        let other: Vec<&serde_json::Value> = arr.iter().filter(|b| b.get("type") != Some(&serde_json::Value::String("tool_result".to_string()))).collect();

        let mut out = Vec::new();
        for block in tool_results {
            out.push(Message {
                role: "tool".to_string(),
                content: block.get("content").cloned().unwrap_or(serde_json::Value::Null),
                name: None,
                tool_calls: None,
                tool_call_id: block.get("tool_use_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
            });
        }

        if !other.is_empty() {
            out.push(Message {
                role: "user".to_string(),
                content: map_content(other),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }

        return out;
    }

    vec![Message {
        role: "user".to_string(),
        content: message.content.clone(),
        name: None,
        tool_calls: None,
        tool_call_id: None,
    }]
}

fn handle_assistant_message(message: &AnthropicAssistantMessage) -> Vec<Message> {
    if let Some(arr) = message.content.as_array() {
        let tool_uses: Vec<&serde_json::Value> = arr.iter().filter(|b| b.get("type") == Some(&serde_json::Value::String("tool_use".to_string()))).collect();
        let text_blocks: Vec<&serde_json::Value> = arr.iter().filter(|b| b.get("type") == Some(&serde_json::Value::String("text".to_string()))).collect();
        let thinking_blocks: Vec<&serde_json::Value> = arr.iter().filter(|b| b.get("type") == Some(&serde_json::Value::String("thinking".to_string()))).collect();

        let all_text = text_blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .chain(thinking_blocks.iter().filter_map(|b| b.get("thinking").and_then(|t| t.as_str())))
            .collect::<Vec<_>>()
            .join("\n\n");

        if !tool_uses.is_empty() {
            let tool_calls = tool_uses
                .iter()
                .filter_map(|b| {
                    let id = b.get("id")?.as_str()?.to_string();
                    let name = b.get("name")?.as_str()?.to_string();
                    let input = b.get("input").cloned().unwrap_or(serde_json::Value::Null);
                    Some(crate::services::copilot::ToolCall {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: crate::services::copilot::ToolCallFunction {
                            name,
                            arguments: input.to_string(),
                        },
                    })
                })
                .collect();

            return vec![Message {
                role: "assistant".to_string(),
                content: if all_text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(all_text) },
                name: None,
                tool_calls: Some(tool_calls),
                tool_call_id: None,
            }];
        }
    }

    vec![Message {
        role: "assistant".to_string(),
        content: message.content.clone(),
        name: None,
        tool_calls: None,
        tool_call_id: None,
    }]
}

fn map_content(blocks: Vec<&serde_json::Value>) -> serde_json::Value {
    let has_image = blocks.iter().any(|b| b.get("type") == Some(&serde_json::Value::String("image".to_string())));
    if !has_image {
        let text = blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()).or_else(|| b.get("thinking").and_then(|t| t.as_str())))
            .collect::<Vec<_>>()
            .join("\n\n");
        return serde_json::Value::String(text);
    }

    let mut parts = Vec::new();
    for block in blocks {
        if let Some(kind) = block.get("type").and_then(|v| v.as_str()) {
            if kind == "text" {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    parts.push(serde_json::json!({"type": "text", "text": text}));
                }
            } else if kind == "image" {
                if let Some(source) = block.get("source") {
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", source.get("media_type").and_then(|v| v.as_str()).unwrap_or("image/png"), source.get("data").and_then(|v| v.as_str()).unwrap_or("")),
                        }
                    }));
                }
            }
        }
    }

    serde_json::Value::Array(parts)
}

fn translate_to_anthropic(openai: &serde_json::Value, model: &str) -> serde_json::Value {
    let mut all_text_blocks: Vec<serde_json::Value> = Vec::new();
    let mut all_tool_blocks: Vec<serde_json::Value> = Vec::new();

    let choices = openai.get("choices").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    let mut stop_reason: Option<String> = None;

    for choice in &choices {
        let message = choice.get("message");

        if let Some(content) = message.and_then(|m| m.get("content")) {
            if let Some(text) = content.as_str() {
                all_text_blocks.push(serde_json::json!({ "type": "text", "text": text }));
            } else if let Some(arr) = content.as_array() {
                for part in arr {
                    if part.get("type") == Some(&serde_json::Value::String("text".to_string())) {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            all_text_blocks.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                }
            }
        }

        if let Some(tool_calls) = message.and_then(|m| m.get("tool_calls")).and_then(|v| v.as_array()) {
            for tool_call in tool_calls {
                let id = tool_call.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = tool_call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = tool_call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");

                let input = serde_json::from_str::<serde_json::Value>(arguments).unwrap_or(serde_json::json!({}));
                all_tool_blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                }));
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            stop_reason = Some(reason.to_string());
        }
    }

    let usage = openai.get("usage");
    let prompt_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached_tokens = usage
        .and_then(|u| u.get("prompt_tokens_details"))
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64());
    let input_tokens = cached_tokens
        .map(|c| prompt_tokens.saturating_sub(c))
        .unwrap_or(prompt_tokens);

    let mut usage_json = serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": completion_tokens,
    });
    if let Some(cached) = cached_tokens {
        usage_json["cache_read_input_tokens"] = serde_json::Value::from(cached);
    }

    let stop_reason = stop_reason
        .as_deref()
        .map(map_openai_stop_reason)
        .unwrap_or("end_turn");

    let mut content = all_text_blocks;
    content.extend(all_tool_blocks);

    serde_json::json!({
        "id": format!("msg_{}", Uuid::new_v4()),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage_json,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        count_tokens, drain_sse_blocks, extract_sse_data, handle_user_message, map_content, resolve_model_alias,
        translate_chunk_to_anthropic_events, translate_messages, translate_responses_to_anthropic,
        translate_to_anthropic, translate_to_openai, AnthropicMessage, AnthropicMessagesPayload,
        AnthropicStreamState, AnthropicTool, AnthropicUserMessage,
    };
    use axum::{body::to_bytes, extract::State, response::IntoResponse, Json};

    fn test_state() -> crate::state::AppState {
        let client = reqwest::Client::builder()
            .user_agent("copilot-api-rs-test")
            .build()
            .expect("reqwest client");
        let config = crate::state::AppConfig::default();
        crate::state::AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            client,
            hooks: None,
        }
    }

    #[test]
    fn translates_tool_calls_and_usage() {
        let response = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-5.2-codex",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": "hello",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Seattle\"}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "prompt_tokens_details": { "cached_tokens": 2 }
            }
        });

        let out = translate_to_anthropic(&response, "claude-sonnet-4");
        let content = out.get("content").and_then(|v| v.as_array()).unwrap();

        assert!(content.iter().any(|c| c.get("type") == Some(&serde_json::Value::String("text".to_string()))));
        assert!(content.iter().any(|c| c.get("type") == Some(&serde_json::Value::String("tool_use".to_string()))));

        let usage = out.get("usage").unwrap();
        assert_eq!(usage.get("input_tokens").and_then(|v| v.as_u64()), Some(8));
        assert_eq!(usage.get("output_tokens").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()), Some(2));
    }

    #[test]
    fn extracts_sse_data_blocks() {
        let mut buffer = b"data: {\"a\":1}\n\n".to_vec();
        let blocks = drain_sse_blocks(&mut buffer);
        assert_eq!(blocks.len(), 1);
        let data = extract_sse_data(&blocks[0]).unwrap();
        assert_eq!(data, "{\"a\":1}");
    }

    #[test]
    fn extracts_multiline_sse_data() {
        let mut buffer = b"data: {\"a\":1}\ndata: {\"b\":2}\n\n".to_vec();
        let blocks = drain_sse_blocks(&mut buffer);
        let data = extract_sse_data(&blocks[0]).unwrap();
        assert_eq!(data, "{\"a\":1}\n{\"b\":2}");
    }

    #[test]
    fn translates_stream_chunk_with_tool_calls() {
        let mut state = AnthropicStreamState::default();
        let chunk = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-5.2-codex",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"Seattle\"}" }
                    }]
                },
                "finish_reason": null
            }]
        });

        let events = translate_chunk_to_anthropic_events(&chunk, &mut state);
        assert!(events.iter().any(|e| e.get("type") == Some(&serde_json::Value::String("content_block_start".to_string()))));
        assert!(events.iter().any(|e| e.get("type") == Some(&serde_json::Value::String("content_block_delta".to_string()))));
    }

    #[test]
    fn converts_responses_to_anthropic_with_usage() {
        let response = serde_json::json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "ok" }]
            }],
            "usage": { "input_tokens": 4, "output_tokens": 7 }
        });

        let out = translate_responses_to_anthropic(&response, "claude-sonnet-4");
        assert_eq!(out.get("model").and_then(|v| v.as_str()), Some("claude-sonnet-4"));
        let usage = out.get("usage").unwrap();
        assert_eq!(usage.get("input_tokens").and_then(|v| v.as_u64()), Some(4));
        assert_eq!(usage.get("output_tokens").and_then(|v| v.as_u64()), Some(7));
    }

    #[test]
    fn resolves_versioned_claude_aliases() {
        assert_eq!(resolve_model_alias("claude-sonnet-4-20250514"), "gpt-5.1-codex");
        assert_eq!(resolve_model_alias("claude-opus-4.5-20250514"), "gpt-5.2-codex");
        assert_eq!(resolve_model_alias("claude-haiku-20240307"), "gpt-5-mini");
    }

    #[test]
    fn translate_messages_merges_system_array() {
        let system = serde_json::json!([
            {"type": "text", "text": "sys-1"},
            {"type": "text", "text": "sys-2"}
        ]);
        let out = translate_messages(&[], Some(system));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[0].content.as_str(), Some("sys-1\n\nsys-2"));
    }

    #[test]
    fn handle_user_message_splits_tool_result() {
        let message = AnthropicUserMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {"type": "tool_result", "tool_use_id": "call-1", "content": "ok"},
                {"type": "text", "text": "hello"}
            ]),
        };
        let out = handle_user_message(&message);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "tool");
        assert_eq!(out[0].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(out[1].role, "user");
        assert_eq!(out[1].content.as_str(), Some("hello"));
    }

    #[tokio::test]
    async fn count_tokens_applies_claude_overhead_and_multiplier() {
        let payload = AnthropicMessagesPayload {
            model: "claude-3.5-sonnet".to_string(),
            messages: vec![AnthropicMessage::User(AnthropicUserMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            })],
            max_tokens: 16,
            system: None,
            metadata: None,
            stop_sequences: None,
            stream: None,
            temperature: None,
            top_p: None,
            top_k: None,
            tools: Some(vec![AnthropicTool {
                name: "doit".to_string(),
                description: None,
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            tool_choice: None,
        };

        let base_payload = translate_to_openai(&payload);
        let base = serde_json::to_string(&base_payload)
            .map(|s| (s.len() as f64 / 4.0).ceil() as u64)
            .unwrap_or(1);

        let mut expected = base.saturating_add(346);
        expected = ((expected as f64) * 1.15).round() as u64;

        let state = test_state();
        let resp = count_tokens(State(state), Json(payload))
            .await
            .expect("count_tokens ok")
            .into_response();
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        let tokens = json.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        assert_eq!(tokens, expected);
    }

    #[test]
    fn map_content_builds_image_data_url() {
        let blocks = vec![
            serde_json::json!({"type": "text", "text": "hi"}),
            serde_json::json!({
                "type": "image",
                "source": {"media_type": "image/png", "data": "abcd"}
            }),
        ];
        let refs: Vec<&serde_json::Value> = blocks.iter().collect();
        let out = map_content(refs);
        let arr = out.as_array().expect("array content");
        assert_eq!(arr.len(), 2);
        let image = arr.iter().find(|v| v.get("type") == Some(&serde_json::Value::String("image_url".to_string()))).expect("image_url part");
        let url = image
            .get("image_url")
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(url, "data:image/png;base64,abcd");
    }
}

#[derive(Debug, Default)]
struct AnthropicStreamState {
    message_start_sent: bool,
    content_block_index: u32,
    content_block_open: bool,
    tool_calls: std::collections::HashMap<u32, ToolCallState>,
}

#[derive(Debug, Clone)]
struct ToolCallState {
    anthropic_block_index: u32,
}

fn is_tool_block_open(state: &AnthropicStreamState) -> bool {
    if !state.content_block_open {
        return false;
    }
    state
        .tool_calls
        .values()
        .any(|tc| tc.anthropic_block_index == state.content_block_index)
}

fn map_openai_stop_reason(reason: &str) -> &str {
    match reason {
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        "content_filter" => "content_filter",
        _ => "end_turn",
    }
}

fn anthropic_error_event() -> serde_json::Value {
    serde_json::json!({
        "type": "error",
        "error": {
            "type": "api_error",
            "message": "An unexpected error occurred during streaming."
        }
    })
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

fn drain_sse_blocks(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut blocks = Vec::new();
    while let Some(pos) = find_double_newline(buffer) {
        let block = buffer.drain(..pos + 2).collect::<Vec<u8>>();
        blocks.push(String::from_utf8_lossy(&block).to_string());
    }
    blocks
}

fn extract_sse_data(block: &str) -> Option<String> {
    let lines: Vec<&str> = block
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn extract_usage(chunk: &serde_json::Value) -> (u64, u64, Option<u64>) {
    let usage = chunk.get("usage");
    let prompt_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached_tokens = usage
        .and_then(|u| u.get("prompt_tokens_details"))
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64());

    let input_tokens = cached_tokens
        .map(|c| prompt_tokens.saturating_sub(c))
        .unwrap_or(prompt_tokens);

    (input_tokens, completion_tokens, cached_tokens)
}

fn translate_chunk_to_anthropic_events(
    chunk: &serde_json::Value,
    state: &mut AnthropicStreamState,
) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    let choice = chunk.get("choices").and_then(|c| c.as_array()).and_then(|a| a.get(0));
    if choice.is_none() {
        return events;
    }

    let choice = choice.unwrap();
    let delta = choice.get("delta").cloned().unwrap_or(serde_json::json!({}));

    if !state.message_start_sent {
        let (input_tokens, _output_tokens, cached_tokens) = extract_usage(chunk);
        let mut usage = serde_json::json!({
            "input_tokens": input_tokens,
            "output_tokens": 0,
        });
        if let Some(cached) = cached_tokens {
            usage["cache_read_input_tokens"] = serde_json::Value::from(cached);
        }

        events.push(serde_json::json!({
            "type": "message_start",
            "message": {
                "id": chunk.get("id").and_then(|v| v.as_str()).unwrap_or_else(|| "msg_unknown").to_string(),
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": chunk.get("model").and_then(|v| v.as_str()).unwrap_or("unknown"),
                "stop_reason": serde_json::Value::Null,
                "stop_sequence": serde_json::Value::Null,
                "usage": usage,
            }
        }));
        state.message_start_sent = true;
    }

    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
        if is_tool_block_open(state) {
            events.push(serde_json::json!({
                "type": "content_block_stop",
                "index": state.content_block_index,
            }));
            state.content_block_index += 1;
            state.content_block_open = false;
        }

        if !state.content_block_open {
            events.push(serde_json::json!({
                "type": "content_block_start",
                "index": state.content_block_index,
                "content_block": { "type": "text", "text": "" },
            }));
            state.content_block_open = true;
        }

        events.push(serde_json::json!({
            "type": "content_block_delta",
            "index": state.content_block_index,
            "delta": { "type": "text_delta", "text": content },
        }));
    }

    if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        for tool_call in tool_calls {
            let index = tool_call.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let id = tool_call.get("id").and_then(|v| v.as_str());
            let name = tool_call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str());

            if let (Some(id), Some(name)) = (id, name) {
                if state.content_block_open {
                    events.push(serde_json::json!({
                        "type": "content_block_stop",
                        "index": state.content_block_index,
                    }));
                    state.content_block_index += 1;
                    state.content_block_open = false;
                }

                let anthropic_index = state.content_block_index;
                state.tool_calls.insert(index, ToolCallState {
                    anthropic_block_index: anthropic_index,
                });

                events.push(serde_json::json!({
                    "type": "content_block_start",
                    "index": anthropic_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": {},
                    }
                }));
                state.content_block_open = true;
            }

            if let Some(args) = tool_call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
            {
                if let Some(info) = state.tool_calls.get(&index) {
                    events.push(serde_json::json!({
                        "type": "content_block_delta",
                        "index": info.anthropic_block_index,
                        "delta": { "type": "input_json_delta", "partial_json": args },
                    }));
                }
            }
        }
    }

    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        if state.content_block_open {
            events.push(serde_json::json!({
                "type": "content_block_stop",
                "index": state.content_block_index,
            }));
            state.content_block_open = false;
        }

        let (input_tokens, output_tokens, cached_tokens) = extract_usage(chunk);
        let mut usage = serde_json::json!({
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        });
        if let Some(cached) = cached_tokens {
            usage["cache_read_input_tokens"] = serde_json::Value::from(cached);
        }

        events.push(serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": map_openai_stop_reason(reason), "stop_sequence": serde_json::Value::Null },
            "usage": usage,
        }));
        events.push(serde_json::json!({ "type": "message_stop" }));
    }

    events
}

fn stream_anthropic(resp: reqwest::Response) -> axum::response::Response {
    let stream = resp.bytes_stream();
    let out_stream = async_stream::stream! {
        let mut state = AnthropicStreamState::default();
        let mut buffer: Vec<u8> = Vec::new();
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            if let Ok(bytes) = chunk {
                buffer.extend_from_slice(&bytes);
                for block in drain_sse_blocks(&mut buffer) {
                    if let Some(data) = extract_sse_data(&block) {
                        if data.trim().is_empty() {
                            continue;
                        }
                        if data.trim() == "[DONE]" {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&data) {
                            Ok(json) => {
                                let events = translate_chunk_to_anthropic_events(&json, &mut state);
                                for ev in events {
                                    let payload = format!("event: {}\ndata: {}\n\n", ev["type"].as_str().unwrap_or("message_delta"), ev.to_string());
                                    yield Ok(Bytes::from(payload));
                                }
                            }
                            Err(_) => {
                                let ev = anthropic_error_event();
                                let payload = format!("event: {}\ndata: {}\n\n", ev["type"].as_str().unwrap_or("error"), ev.to_string());
                                yield Ok(Bytes::from(payload));
                            }
                        }
                    }
                }
            }
        }
    };

    crate::routes::streaming::sse_response(out_stream)
}

fn stream_anthropic_from_responses(resp: reqwest::Response, model: &str) -> axum::response::Response {
    let stream = resp.bytes_stream();
    let model = model.to_string();
    let out_stream = async_stream::stream! {
        futures::pin_mut!(stream);

        let mut output_tokens: u64 = 0;
        let mut buffer: Vec<u8> = Vec::new();

        let message_id = format!("msg_{}", Uuid::new_v4());
        let start = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": model,
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": 0, "output_tokens": 0 }
            }
        });
        yield Ok::<Bytes, std::io::Error>(Bytes::from(format!("event: message_start\ndata: {}\n\n", start)));

        let block_start = serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        });
        yield Ok(Bytes::from(format!("event: content_block_start\ndata: {}\n\n", block_start)));

        while let Some(chunk) = stream.next().await {
            if let Ok(bytes) = chunk {
                buffer.extend_from_slice(&bytes);
                for block in drain_sse_blocks(&mut buffer) {
                    if let Some(data) = extract_sse_data(&block) {
                        if data.trim().is_empty() {
                            continue;
                        }
                        if data.trim() == "[DONE]" {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&data) {
                            Ok(json) => {
                                if json.get("type") == Some(&serde_json::Value::String("response.output_text.delta".to_string())) {
                                    if let Some(delta) = json.get("delta").and_then(|v| v.as_str()) {
                                        let ev = serde_json::json!({
                                            "type": "content_block_delta",
                                            "index": 0,
                                            "delta": { "type": "text_delta", "text": delta }
                                        });
                                        yield Ok(Bytes::from(format!("event: content_block_delta\ndata: {}\n\n", ev)));
                                    }
                                }

                                if json.get("type") == Some(&serde_json::Value::String("response.completed".to_string())) {
                                    if let Some(tokens) = json
                                        .get("response")
                                        .and_then(|r| r.get("usage"))
                                        .and_then(|u| u.get("output_tokens"))
                                        .and_then(|v| v.as_u64())
                                    {
                                        output_tokens = tokens;
                                    }
                                }
                            }
                            Err(_) => {
                                let ev = anthropic_error_event();
                                let payload = format!("event: {}\ndata: {}\n\n", ev["type"].as_str().unwrap_or("error"), ev.to_string());
                                yield Ok(Bytes::from(payload));
                            }
                        }
                    }
                }
            }
        }

        let block_stop = serde_json::json!({ "type": "content_block_stop", "index": 0 });
        yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {}\n\n", block_stop)));

        let delta = serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "stop_sequence": null },
            "usage": { "output_tokens": output_tokens }
        });
        yield Ok(Bytes::from(format!("event: message_delta\ndata: {}\n\n", delta)));

        let stop = serde_json::json!({ "type": "message_stop" });
        yield Ok(Bytes::from(format!("event: message_stop\ndata: {}\n\n", stop)));
    };

    crate::routes::streaming::sse_response(out_stream)
}

fn translate_responses_to_anthropic(response: &serde_json::Value, model: &str) -> serde_json::Value {
    let output_text = response
        .get("output")
        .and_then(|o| o.as_array())
        .and_then(|arr| arr.iter().find(|x| x.get("type") == Some(&serde_json::Value::String("message".to_string()))))
        .and_then(|msg| msg.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.iter().find(|x| x.get("type") == Some(&serde_json::Value::String("output_text".to_string()))))
        .and_then(|t| t.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    serde_json::json!({
        "id": format!("msg_{}", Uuid::new_v4()),
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": output_text }],
        "model": model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": response.get("usage").cloned().unwrap_or(serde_json::json!({}))
    })
}

fn resolve_model_alias(model: &str) -> String {
    let aliases = [
        ("claude-opus-4.5", "gpt-5.2-codex"),
        ("claude-opus-4", "gpt-5.2-codex"),
        ("claude-4-opus", "gpt-5.2-codex"),
        ("claude-3-opus", "gpt-5.2-codex"),
        ("claude-3-opus-20240229", "gpt-5.2-codex"),
        ("claude-sonnet-4", "gpt-5.1-codex"),
        ("claude-4-sonnet", "gpt-5.1-codex"),
        ("claude-3.5-sonnet", "gpt-5.1-codex"),
        ("claude-3-5-sonnet-20241022", "gpt-5.1-codex"),
        ("claude-3-sonnet", "gpt-5.1-codex"),
        ("claude-3-sonnet-20240229", "gpt-5.1-codex"),
        ("claude-haiku-3.5", "gpt-5-mini"),
        ("claude-3.5-haiku", "gpt-5-mini"),
        ("claude-3-haiku", "gpt-5-mini"),
        ("claude-3-haiku-20240307", "gpt-5-mini"),
        ("claude-2.1", "gpt-5.1"),
        ("claude-2.0", "gpt-5.1"),
        ("claude-instant-1.2", "gpt-5-mini"),
        ("o3", "gpt-5.2-codex"),
        ("o3-mini", "gpt-5-mini"),
        ("o1", "gpt-5.1"),
        ("o1-preview", "gpt-5.1"),
        ("o1-mini", "gpt-5-mini"),
    ];

    if model.starts_with("claude-sonnet-4-") {
        return "gpt-5.1-codex".to_string();
    }
    if model.starts_with("claude-opus-4-") || model.starts_with("claude-opus-4.5-") {
        return "gpt-5.2-codex".to_string();
    }
    if model.starts_with("claude-haiku-") {
        return "gpt-5-mini".to_string();
    }

    for (from, to) in aliases {
        if model == from {
            return to.to_string();
        }
    }

    model.to_string()
}

fn requires_responses_api(model: &str) -> bool {
    matches!(model,
        "gpt-5.2-codex" | "gpt-5.1-codex" | "gpt-5.1-codex-mini" | "gpt-5.1-codex-max" | "gpt-5-codex" | "goldeneye" | "codex-5.2" | "codex-5.1"
    )
}
