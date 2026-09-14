use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    approval::check_manual_approval,
    auth_flow::ensure_copilot_token,
    errors::{ApiError, ApiResult},
    hooks::types::HookInput,
    protocol::{ApiJson, relay_response, validate_anthropic, validate_translation},
    rate_limit::check_rate_limit,
    routes::responses::{extract_instructions, messages_to_responses_input},
    services::{
        anthropic,
        copilot::{
            ChatCompletionsPayload, Message, Tool, create_chat_completions, create_responses,
        },
    },
    state::AppState,
};

#[cfg(test)]
use crate::routes::streaming::{drain_sse_blocks, extract_sse_data};

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
    pub content: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AnthropicAssistantMessage {
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

pub async fn handle(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    ApiJson(mut raw): ApiJson<serde_json::Value>,
) -> ApiResult<Response> {
    validate_anthropic(&raw, false)?;
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PreToolUse".to_string()),
            tool: Some("AnthropicMessages".to_string()),
            tool_input: Some(raw.clone()),
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

    if provider == "anthropic" {
        let response = anthropic::create_messages(&state.client, &headers, &raw).await?;
        return relay_native_messages(&state, &raw, response).await;
    }
    if provider != "copilot" {
        return Err(ApiError::BadRequest(
            "Use an Anthropic-compatible provider or Copilot for /v1/messages".to_string(),
        ));
    }
    let token = ensure_copilot_token(&state).await?;
    crate::services::copilot::ensure_models(&state, &token).await?;
    let config = state.config.read().await.clone();
    let requested_model = raw["model"]
        .as_str()
        .ok_or_else(|| ApiError::BadRequest("Missing model".to_string()))?;
    let resolved_model = crate::config::resolve_model_id(&config, requested_model);
    raw["model"] = resolved_model.clone().into();
    if crate::config::supports_endpoint(&config, &resolved_model, "/v1/messages") {
        let response = crate::services::copilot::create_messages(
            &state.client,
            &config,
            &token,
            &raw,
            &headers,
        )
        .await?;
        return relay_native_messages(&state, &raw, response).await;
    }
    validate_translation(&raw)?;
    let payload: AnthropicMessagesPayload = serde_json::from_value(raw).map_err(|_| {
        ApiError::BadRequest("This request cannot be converted for the selected model".to_string())
    })?;

    if (crate::config::supports_endpoint(&config, &resolved_model, "/responses")
        && !crate::config::supports_endpoint(&config, &resolved_model, "/chat/completions"))
        || requires_responses_api(&resolved_model)
    {
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

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid response: {e}")))?;
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

async fn relay_native_messages(
    state: &AppState,
    payload: &serde_json::Value,
    upstream: reqwest::Response,
) -> ApiResult<Response> {
    let (response, output) = crate::protocol::relay_response_with_output(
        upstream,
        payload["stream"].as_bool().unwrap_or(false),
    )
    .await?;
    if let Some(hooks) = &state.hooks {
        hooks
            .post_tool_use("AnthropicMessages", payload.clone(), output)
            .await;
    }
    Ok(response)
}

pub async fn count_tokens(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    ApiJson(payload): ApiJson<serde_json::Value>,
) -> ApiResult<Response> {
    validate_anthropic(&payload, true)?;
    if std::env::var("COPILOT_PROVIDER").as_deref() == Ok("anthropic") {
        let response = anthropic::count_tokens(&state.client, &headers, &payload).await?;
        return relay_response(response, false).await;
    }
    let token_count = crate::utils::estimate_tokens_from_json(&payload).max(1);
    if state.config.read().await.show_token {
        tracing::info!("Token count (heuristic): {}", token_count);
    }
    let mut response = Json(serde_json::json!({ "input_tokens": token_count })).into_response();
    response.headers_mut().insert(
        "x-token-count-estimated",
        axum::http::HeaderValue::from_static("true"),
    );
    Ok(response)
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
        extra: crate::protocol::responses_options(&openai_payload.extra),
        model: resolved_model,
        input: serde_json::to_value(input).unwrap_or(serde_json::json!([])),
        instructions,
        max_output_tokens: openai_payload.max_tokens,
        temperature: openai_payload.temperature,
        top_p: openai_payload.top_p,
        stream: payload.stream,
        tools: openai_payload
            .tools
            .as_deref()
            .map(crate::protocol::responses_tools),
        tool_choice: openai_payload
            .tool_choice
            .as_ref()
            .map(crate::protocol::responses_tool_choice)
            .transpose()?,
        previous_response_id: None,
    };

    let config = state.config.read().await.clone();
    let resp = create_responses(&state.client, &config, &token, &responses_payload).await?;

    if payload.stream.unwrap_or(false) {
        if let Some(hooks) = &state.hooks {
            hooks
                .post_tool_use(
                    "AnthropicMessages",
                    serde_json::to_value(&payload).unwrap_or_default(),
                    None,
                )
                .await;
        }
        return Ok(stream_anthropic_from_responses(resp, &payload.model));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid responses payload: {e}")))?;
    let anthropic = translate_responses_to_anthropic(&json, &payload.model);
    if let Some(hooks) = &state.hooks {
        hooks
            .post_tool_use(
                "AnthropicMessages",
                serde_json::to_value(&payload).unwrap_or_default(),
                Some(anthropic.clone()),
            )
            .await;
    }
    Ok(Json(anthropic).into_response())
}

fn translate_to_openai(payload: &AnthropicMessagesPayload) -> ChatCompletionsPayload {
    let messages = translate_messages(&payload.messages, payload.system.clone());
    let mut extra = serde_json::Map::new();
    if let Some(disabled) = payload
        .tool_choice
        .as_ref()
        .and_then(|choice| choice.get("disable_parallel_tool_use"))
        .and_then(serde_json::Value::as_bool)
    {
        extra.insert("parallel_tool_calls".to_string(), (!disabled).into());
    }
    ChatCompletionsPayload {
        extra,
        model: resolve_model_alias(&payload.model),
        messages,
        max_tokens: Some(payload.max_tokens),
        stop: payload
            .stop_sequences
            .as_ref()
            .map(|s| serde_json::to_value(s).unwrap()),
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
        tools: payload.tools.as_deref().map(translate_tools),
        tool_choice: payload.tool_choice.as_ref().map(|choice| {
            match choice["type"].as_str().unwrap_or("") {
                "tool" => {
                    serde_json::json!({"type": "function", "function": {"name": choice["name"]}})
                }
                "any" => "required".into(),
                "none" => "none".into(),
                _ => "auto".into(),
            }
        }),
        user: payload.metadata.as_ref().and_then(|m| {
            m.get("user_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }),
    }
}

fn translate_tools(tools: &[AnthropicTool]) -> Vec<Tool> {
    tools
        .iter()
        .map(|t| Tool {
            r#type: "function".to_string(),
            function: crate::services::copilot::ToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
                strict: None,
            },
        })
        .collect()
}

fn translate_messages(
    messages: &[AnthropicMessage],
    system: Option<serde_json::Value>,
) -> Vec<Message> {
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
        let tool_results: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|b| {
                b.get("type") == Some(&serde_json::Value::String("tool_result".to_string()))
            })
            .collect();
        let other: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|b| {
                b.get("type") != Some(&serde_json::Value::String("tool_result".to_string()))
            })
            .collect();

        let mut out = Vec::new();
        for block in tool_results {
            out.push(Message {
                role: "tool".to_string(),
                content: block
                    .get("content")
                    .map(|content| {
                        if let Some(blocks) = content.as_array() {
                            map_content(blocks.iter().collect())
                        } else {
                            content.clone()
                        }
                    })
                    .unwrap_or_else(|| "".into()),
                name: None,
                tool_calls: None,
                tool_call_id: block
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
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
        let tool_uses: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|b| b.get("type") == Some(&serde_json::Value::String("tool_use".to_string())))
            .collect();
        let text_blocks: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|b| b.get("type") == Some(&serde_json::Value::String("text".to_string())))
            .collect();
        let thinking_blocks: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|b| b.get("type") == Some(&serde_json::Value::String("thinking".to_string())))
            .collect();

        let all_text = text_blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .chain(
                thinking_blocks
                    .iter()
                    .filter_map(|b| b.get("thinking").and_then(|t| t.as_str())),
            )
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
                content: if all_text.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(all_text)
                },
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
    let has_image = blocks
        .iter()
        .any(|b| b.get("type") == Some(&serde_json::Value::String("image".to_string())));
    if !has_image {
        let text = blocks
            .iter()
            .filter_map(|b| {
                b.get("text")
                    .and_then(|t| t.as_str())
                    .or_else(|| b.get("thinking").and_then(|t| t.as_str()))
            })
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
            } else if kind == "image"
                && let Some(source) = block.get("source")
            {
                parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": if source["type"] == "url" { source["url"].as_str().unwrap_or("").to_string() } else { format!("data:{};base64,{}", source.get("media_type").and_then(|v| v.as_str()).unwrap_or("image/png"), source.get("data").and_then(|v| v.as_str()).unwrap_or("")) },
                        }
                    }));
            }
        }
    }

    serde_json::Value::Array(parts)
}

fn translate_to_anthropic(openai: &serde_json::Value, model: &str) -> serde_json::Value {
    let mut all_text_blocks: Vec<serde_json::Value> = Vec::new();
    let mut all_tool_blocks: Vec<serde_json::Value> = Vec::new();

    let choices = openai
        .get("choices")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let mut stop_reason: Option<String> = None;

    for choice in &choices {
        let message = choice.get("message");

        if let Some(content) = message.and_then(|m| m.get("content")) {
            if let Some(text) = content.as_str() {
                all_text_blocks.push(serde_json::json!({ "type": "text", "text": text }));
            } else if let Some(arr) = content.as_array() {
                for part in arr {
                    if part.get("type") == Some(&serde_json::Value::String("text".to_string()))
                        && let Some(text) = part.get("text").and_then(|v| v.as_str())
                    {
                        all_text_blocks.push(serde_json::json!({ "type": "text", "text": text }));
                    }
                }
            }
        }

        if let Some(tool_calls) = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(|v| v.as_array())
        {
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

                let input = serde_json::from_str::<serde_json::Value>(arguments)
                    .unwrap_or(serde_json::json!({}));
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
        AnthropicMessage, AnthropicMessagesPayload, AnthropicStreamState, AnthropicTool,
        AnthropicUserMessage, count_tokens, drain_sse_blocks, extract_sse_data,
        handle_user_message, map_content, resolve_model_alias, translate_chunk_to_anthropic_events,
        translate_messages, translate_responses_to_anthropic, translate_to_anthropic,
    };
    use axum::{body::to_bytes, extract::State, response::IntoResponse};

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
    fn accepts_standard_claude_message_json() {
        let payload = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 32,
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}
            ]
        });
        let parsed: AnthropicMessagesPayload =
            serde_json::from_value(payload.clone()).expect("standard Claude request");
        let serialized = serde_json::to_value(&parsed).expect("serialize Claude request");
        assert_eq!(serialized["messages"], payload["messages"]);
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

        assert!(
            content
                .iter()
                .any(|c| c.get("type") == Some(&serde_json::Value::String("text".to_string())))
        );
        assert!(
            content
                .iter()
                .any(|c| c.get("type") == Some(&serde_json::Value::String("tool_use".to_string())))
        );

        let usage = out.get("usage").unwrap();
        assert_eq!(usage.get("input_tokens").and_then(|v| v.as_u64()), Some(8));
        assert_eq!(usage.get("output_tokens").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(
            usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64()),
            Some(2)
        );
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
    fn extracts_crlf_sse_data_blocks() {
        let mut buffer = b"event: message\r\ndata: {\"text\":\"ok\"}\r\n\r\n".to_vec();
        let blocks = drain_sse_blocks(&mut buffer);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            extract_sse_data(&blocks[0]).as_deref(),
            Some("{\"text\":\"ok\"}")
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn extracts_multiline_sse_data() {
        let mut buffer = b"data: {\"a\":1}\ndata: {\"b\":2}\n\n".to_vec();
        let blocks = drain_sse_blocks(&mut buffer);
        let data = extract_sse_data(&blocks[0]).unwrap();
        assert_eq!(data, "{\"a\":1}\n{\"b\":2}");
    }

    #[test]
    fn accepts_sse_optional_space_and_empty_data_fields() {
        assert_eq!(
            extract_sse_data("data:{\"ok\":true}\n\n").as_deref(),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            extract_sse_data("data:  indented\ndata\ndata:\n\n").as_deref(),
            Some(" indented\n\n")
        );
        assert_eq!(extract_sse_data(": heartbeat\nevent: ping\n\n"), None);
    }

    #[test]
    fn accepts_sse_line_endings_across_every_network_split() {
        for ending in ["\n", "\r\n", "\r"] {
            let input =
                format!("event: message{ending}data:one{ending}{ending}data:two{ending}{ending}");
            for split in 0..=input.len() {
                let mut buffer = input.as_bytes()[..split].to_vec();
                let mut blocks = drain_sse_blocks(&mut buffer);
                buffer.extend_from_slice(&input.as_bytes()[split..]);
                blocks.extend(drain_sse_blocks(&mut buffer));
                let data: Vec<String> = blocks
                    .iter()
                    .filter_map(|block| extract_sse_data(block))
                    .collect();
                assert_eq!(data, ["one", "two"], "ending={ending:?}, split={split}");
                assert!(
                    buffer.is_empty() || buffer == b"\n",
                    "incomplete frame: {buffer:?}"
                );
            }
        }
    }

    #[test]
    fn message_roles_are_validated_and_serialized_once() {
        for role in ["user", "assistant"] {
            let input = format!("{{\"role\":\"{role}\",\"content\":\"hello\"}}");
            let message: AnthropicMessage = serde_json::from_str(&input).expect("valid role");
            assert_eq!(
                serde_json::to_string(&message).expect("serialize message"),
                input
            );
        }
        assert!(
            serde_json::from_str::<AnthropicMessage>(r#"{"role":"invalid","content":"hello"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<AnthropicMessage>(
                r#"{"role":"user","role":"assistant","content":"hello"}"#
            )
            .is_err()
        );
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
        assert!(events.iter().any(|e| e.get("type")
            == Some(&serde_json::Value::String(
                "content_block_start".to_string()
            ))));
        assert!(events.iter().any(|e| e.get("type")
            == Some(&serde_json::Value::String(
                "content_block_delta".to_string()
            ))));
    }

    #[test]
    fn parallel_tool_streams_do_not_close_blocks_before_their_deltas() {
        let mut state = AnthropicStreamState::default();
        let first = serde_json::json!({"model": "claude-sonnet-4.6", "choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "call_one", "function": {"name": "first", "arguments": "{"}},
            {"index": 1, "id": "call_two", "function": {"name": "second", "arguments": "{"}}
        ]}}]});
        let second = serde_json::json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": "}"}}, {"index": 1, "function": {"arguments": "}"}}
        ]}, "finish_reason": "tool_calls"}]});
        let mut events = translate_chunk_to_anthropic_events(&first, &mut state);
        events.extend(translate_chunk_to_anthropic_events(&second, &mut state));
        let mut stopped = std::collections::HashSet::new();
        for event in events {
            if event["type"] == "content_block_stop" {
                stopped.insert(event["index"].as_u64().expect("index"));
            }
            if event["type"] == "content_block_delta" {
                assert!(!stopped.contains(&event["index"].as_u64().expect("index")));
            }
        }
    }

    #[test]
    fn repeated_tool_metadata_does_not_drop_argument_fragments() {
        let mut state = AnthropicStreamState::default();
        let mut events = Vec::new();
        for argument in ["{\"city\":", "\"Paris\"}"] {
            let chunk = serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": "call_fixture", "function": {"name": "weather", "arguments": argument}
            }]}}]});
            events.extend(translate_chunk_to_anthropic_events(&chunk, &mut state));
        }
        let arguments = events
            .iter()
            .filter_map(|event| event["delta"]["partial_json"].as_str())
            .collect::<String>();
        assert_eq!(arguments, "{\"city\":\"Paris\"}");
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "content_block_start")
                .count(),
            1
        );
    }

    #[test]
    fn translates_anthropic_tool_choice_to_openai() {
        for (choice, expected) in [
            (
                serde_json::json!({"type": "auto"}),
                serde_json::json!("auto"),
            ),
            (
                serde_json::json!({"type": "any"}),
                serde_json::json!("required"),
            ),
            (
                serde_json::json!({"type": "none"}),
                serde_json::json!("none"),
            ),
            (
                serde_json::json!({"type": "tool", "name": "weather"}),
                serde_json::json!({"type": "function", "function": {"name": "weather"}}),
            ),
        ] {
            let payload: AnthropicMessagesPayload = serde_json::from_value(serde_json::json!({
                "model": "claude-sonnet-4.6", "max_tokens": 32, "messages": [{"role": "user", "content": "hello"}],
                "tool_choice": choice
            })).expect("payload");
            assert_eq!(
                super::translate_to_openai(&payload).tool_choice,
                Some(expected)
            );
        }
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
        assert_eq!(
            out.get("model").and_then(|v| v.as_str()),
            Some("claude-sonnet-4")
        );
        let usage = out.get("usage").unwrap();
        assert_eq!(usage.get("input_tokens").and_then(|v| v.as_u64()), Some(4));
        assert_eq!(usage.get("output_tokens").and_then(|v| v.as_u64()), Some(7));
    }

    #[test]
    fn resolves_versioned_claude_aliases() {
        for model in [
            "claude-sonnet-4-20250514",
            "claude-opus-4.5-20250514",
            "claude-haiku-20240307",
            "claude-sonnet-4-6",
            "claude-opus-5",
        ] {
            assert_eq!(resolve_model_alias(model), model);
        }
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
    async fn count_tokens_labels_estimates() {
        let payload = AnthropicMessagesPayload {
            model: "claude-3.5-sonnet".to_string(),
            messages: vec![AnthropicMessage::User(AnthropicUserMessage {
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

        let payload = serde_json::to_value(&payload).expect("request");
        let expected = crate::utils::estimate_tokens_from_json(&payload).max(1);
        let state = test_state();
        let resp = count_tokens(
            State(state),
            axum::http::HeaderMap::new(),
            crate::protocol::ApiJson(payload),
        )
        .await
        .expect("count_tokens ok")
        .into_response();
        assert_eq!(resp.headers()["x-token-count-estimated"], "true");
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        let tokens = json
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(tokens, expected);
    }

    #[test]
    fn map_content_builds_image_data_url() {
        let blocks = [
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
        let image = arr
            .iter()
            .find(|v| v.get("type") == Some(&serde_json::Value::String("image_url".to_string())))
            .expect("image_url part");
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
    text_block: Option<u32>,
    open_blocks: std::collections::BTreeSet<u32>,
    tool_calls: std::collections::HashMap<u32, ToolCallState>,
    stop_reason: Option<String>,
    usage: serde_json::Value,
}

#[derive(Debug, Clone)]
struct ToolCallState {
    anthropic_block_index: u32,
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
    if chunk.get("usage").is_some_and(|usage| !usage.is_null()) {
        let (input, output, cached) = extract_usage(chunk);
        state.usage = serde_json::json!({"input_tokens": input, "output_tokens": output});
        if let Some(cached) = cached {
            state.usage["cache_read_input_tokens"] = cached.into();
        }
    }
    let choice = chunk
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());
    if choice.is_none() {
        return events;
    }

    let choice = choice.unwrap();
    let delta = choice
        .get("delta")
        .cloned()
        .unwrap_or(serde_json::json!({}));

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
                "id": chunk.get("id").and_then(|v| v.as_str()).unwrap_or("msg_unknown").to_string(),
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
        if state.text_block.is_none() {
            let index = state.content_block_index;
            state.content_block_index += 1;
            state.text_block = Some(index);
            state.open_blocks.insert(index);
            events.push(serde_json::json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "text", "text": "" },
            }));
        }

        events.push(serde_json::json!({
            "type": "content_block_delta",
            "index": state.text_block,
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

            if let (Some(id), Some(name)) = (id, name)
                && !state.tool_calls.contains_key(&index)
            {
                let anthropic_index = state.content_block_index;
                state.content_block_index += 1;
                state.open_blocks.insert(anthropic_index);
                state.tool_calls.insert(
                    index,
                    ToolCallState {
                        anthropic_block_index: anthropic_index,
                    },
                );

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
            }

            if let Some(args) = tool_call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                && let Some(info) = state.tool_calls.get(&index)
            {
                events.push(serde_json::json!({
                    "type": "content_block_delta",
                    "index": info.anthropic_block_index,
                    "delta": { "type": "input_json_delta", "partial_json": args },
                }));
            }
        }
    }

    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        state.stop_reason = Some(map_openai_stop_reason(reason).to_string());
    }

    events
}

fn stream_anthropic(resp: reqwest::Response) -> axum::response::Response {
    stream_chat_as_anthropic(crate::routes::streaming::json_events(resp))
}

fn stream_chat_as_anthropic(
    stream: impl futures::Stream<Item = Result<serde_json::Value, std::io::Error>> + Send + 'static,
) -> Response {
    let out_stream = async_stream::stream! {
        let mut state = AnthropicStreamState::default();
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => for event in translate_chunk_to_anthropic_events(&chunk, &mut state) {
                    yield Ok(Bytes::from(format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap_or("error"))));
                },
                Err(_) => {
                    yield Ok(Bytes::from(format!("event: error\ndata: {}\n\n", anthropic_error_event())));
                    return;
                }
            }
        }
        let Some(reason) = state.stop_reason else {
            yield Ok(Bytes::from(format!("event: error\ndata: {}\n\n", anthropic_error_event())));
            return;
        };
        for index in state.open_blocks {
            let event = serde_json::json!({"type": "content_block_stop", "index": index});
            yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {event}\n\n")));
        }
        let usage = if state.usage.is_null() { serde_json::json!({"input_tokens": 0, "output_tokens": 0}) } else { state.usage };
        let event = serde_json::json!({"type": "message_delta", "delta": {"stop_reason": reason, "stop_sequence": null}, "usage": usage});
        yield Ok(Bytes::from(format!("event: message_delta\ndata: {event}\n\n")));
        yield Ok::<Bytes, std::io::Error>(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
    };
    crate::routes::streaming::sse_response(out_stream)
}

fn stream_anthropic_from_responses(
    resp: reqwest::Response,
    model: &str,
) -> axum::response::Response {
    stream_chat_as_anthropic(crate::routes::chat_completions::responses_chat_events(
        resp,
        model.to_string(),
    ))
}

fn translate_responses_to_anthropic(
    response: &serde_json::Value,
    model: &str,
) -> serde_json::Value {
    let chat = crate::routes::chat_completions::convert_responses_to_chat(
        response.clone(),
        model.to_string(),
    );
    translate_to_anthropic(&chat, model)
}

fn resolve_model_alias(model: &str) -> String {
    model.to_string()
}

fn requires_responses_api(model: &str) -> bool {
    matches!(
        model,
        "gpt-5.2-codex"
            | "gpt-5.1-codex"
            | "gpt-5.1-codex-mini"
            | "gpt-5.1-codex-max"
            | "gpt-5-codex"
            | "goldeneye"
            | "codex-5.2"
            | "codex-5.1"
    )
}
