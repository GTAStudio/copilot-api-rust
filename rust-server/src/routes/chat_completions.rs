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
        azure,
        copilot::{create_chat_completions, create_responses, ChatCompletionsPayload, ResponsesPayload},
        openai,
    },
    state::AppState,
};

const RESPONSES_API_MODELS: &[&str] = &[
    "gpt-5.2-codex",
    "gpt-5.1-codex",
    "gpt-5.1-codex-mini",
    "gpt-5.1-codex-max",
    "gpt-5-codex",
    "goldeneye",
];

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
        ("codex-5.2", "gpt-5.2-codex"),
        ("codex-5.1", "gpt-5.1-codex"),
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
    RESPONSES_API_MODELS.contains(&model) || matches!(model, "codex-5.2" | "codex-5.1")
}

pub async fn handle(State(state): State<AppState>, Json(mut payload): Json<ChatCompletionsPayload>) -> ApiResult<Response> {
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PreToolUse".to_string()),
            tool: Some("ChatCompletions".to_string()),
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

    if provider == "azure" || payload.model.starts_with("azure:") {
        if let Some(cfg) = azure::load_azure_config(&payload.model) {
            let mut azure_payload = payload.clone();
            if azure_payload.model.starts_with("azure:") {
                azure_payload.model = cfg.deployment.clone();
            }
            let resp = azure::create_chat_completions(&state.client, &cfg, &serde_json::to_value(&azure_payload).unwrap())
                .await?;
            if payload.stream.unwrap_or(false) {
                let stream = crate::services::copilot::response_body_stream(resp);
                return Ok(crate::routes::streaming::sse_response(stream));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid Azure response: {e}")))?;
            return Ok(Json(json).into_response());
        }
    }

    if provider == "openai" || payload.model.starts_with("openai:") {
        if payload.model.starts_with("openai:") {
            payload.model = payload.model.trim_start_matches("openai:").to_string();
        }

        if requires_responses_api(&payload.model) {
            return Err(ApiError::BadRequest("Model requires /v1/responses when using OpenAI provider".to_string()));
        }

        let resp = openai::create_chat_completions(&state.client, &serde_json::to_value(&payload).unwrap()).await?;
        if payload.stream.unwrap_or(false) {
            let stream = crate::services::copilot::response_body_stream(resp);
            return Ok(crate::routes::streaming::sse_response(stream));
        }
        let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid OpenAI response: {e}")))?;
        return Ok(Json(json).into_response());
    }

    let token = ensure_copilot_token(&state).await?;

    let original_model = payload.model.clone();
    payload.model = resolve_model_alias(&payload.model);

    if requires_responses_api(&payload.model) {
        return handle_responses_api(state, payload, original_model).await;
    }

    if state.config.read().await.show_token {
        if crate::tokenizer::use_precise_tokenizer() {
            let tokenizer = state
                .config
                .read()
                .await
                .models
                .as_ref()
                .and_then(|models| models.data.iter().find(|m| m.id == payload.model))
                .map(|m| m.capabilities.tokenizer.clone())
                .unwrap_or_else(|| "o200k_base".to_string());
            let estimate = crate::tokenizer::estimate_chat_tokens(&payload, &tokenizer);
            tracing::info!("Token count (tiktoken): {}", estimate);
        } else {
            let estimate = crate::utils::estimate_tokens_from_json(&serde_json::to_value(&payload).unwrap_or_default());
            tracing::info!("Token count (heuristic): {}", estimate);
        }
    }

    let config = state.config.read().await.clone();

    if payload.max_tokens.is_none() {
        if let Some(models) = &config.models {
            if let Some(model) = models.data.iter().find(|m| m.id == payload.model) {
                payload.max_tokens = model.capabilities.limits.max_output_tokens;
            }
        }
    }
    let resp = create_chat_completions(&state.client, &config, &token, &payload).await?;

    if payload.stream.unwrap_or(false) {
        let stream = crate::services::copilot::response_body_stream(resp);
        if let Some(hooks) = &state.hooks {
            let input = HookInput {
                hook_type: Some("PostToolUse".to_string()),
                tool: Some("ChatCompletions".to_string()),
                tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
                tool_output: None,
                session_id: None,
            };
            let _ = hooks.execute_event("PostToolUse", &input).await;
        }
        return Ok(crate::routes::streaming::sse_response(stream));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid response: {e}")))?;
    if let Some(hooks) = &state.hooks {
        let input = HookInput {
            hook_type: Some("PostToolUse".to_string()),
            tool: Some("ChatCompletions".to_string()),
            tool_input: Some(serde_json::to_value(&payload).unwrap_or_default()),
            tool_output: Some(json.clone()),
            session_id: None,
        };
        let _ = hooks.execute_event("PostToolUse", &input).await;
    }
    Ok(Json(json).into_response())
}

async fn handle_responses_api(
    state: AppState,
    payload: ChatCompletionsPayload,
    _original_model: String,
) -> ApiResult<Response> {
    let token = ensure_copilot_token(&state).await?;
    let config = state.config.read().await.clone();

    let instructions = extract_instructions(&payload.messages);
    let input = messages_to_responses_input(&payload.messages);

    if input.is_empty() {
        return Err(ApiError::BadRequest("No valid input messages".to_string()));
    }

    let responses_payload = ResponsesPayload {
        model: payload.model.clone(),
        input: serde_json::to_value(input).unwrap_or(serde_json::json!([])),
        instructions,
        max_output_tokens: payload.max_tokens,
        temperature: payload.temperature,
        top_p: payload.top_p,
        stream: payload.stream,
        tools: payload.tools.as_ref().map(|tools| {
            serde_json::Value::Array(
                tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "name": t.function.name,
                            "description": t.function.description,
                            "parameters": t.function.parameters,
                        })
                    })
                    .collect(),
            )
        }),
        tool_choice: payload.tool_choice,
        previous_response_id: None,
    };

    let resp = create_responses(&state.client, &config, &token, &responses_payload).await?;

    if payload.stream.unwrap_or(false) {
        return Ok(stream_responses_as_chat_completion(resp, payload.model.clone()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| ApiError::Upstream(format!("Invalid responses payload: {e}")))?;
    let converted = convert_responses_to_chat(json, payload.model);
    Ok(Json(converted).into_response())
}

fn stream_responses_as_chat_completion(resp: reqwest::Response, model: String) -> axum::response::Response {
    let stream = resp.bytes_stream();
    let out_stream = async_stream::stream! {
        let mut buffer = Vec::<u8>::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;
        let mut saw_completed = false;
        let chat_id = format!("chatcmpl-{}", Uuid::new_v4());
        futures::pin_mut!(stream);

        while let Some(chunk) = stream.next().await {
            if let Ok(bytes) = chunk {
                buffer.extend_from_slice(&bytes);
                while let Some(pos) = find_double_newline(&buffer) {
                    let line = buffer.drain(..pos + 2).collect::<Vec<u8>>();
                    let text = String::from_utf8_lossy(&line);
                    for raw in text.split("\n") {
                        if let Some(data) = raw.strip_prefix("data: ") {
                            if data.trim() == "[DONE]" {
                                continue;
                            }
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(delta) = json.get("delta") {
                                    let chunk = build_chat_chunk(&chat_id, delta, json.get("response"));
                                    let payload = format!("data: {}\n\n", serde_json::to_string(&chunk).unwrap());
                                    yield Ok(Bytes::from(payload));
                                }

                                if json.get("type") == Some(&serde_json::Value::String("response.completed".to_string())) {
                                    if let Some(usage) = json.get("response").and_then(|r| r.get("usage")) {
                                        input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                        output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                    }
                                    saw_completed = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        if saw_completed {
            let final_chunk = serde_json::json!({
                "id": chat_id,
                "object": "chat.completion.chunk",
                "created": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": input_tokens,
                    "completion_tokens": output_tokens,
                    "total_tokens": input_tokens + output_tokens,
                }
            });
            let payload = format!("data: {}\n\n", final_chunk.to_string());
            yield Ok(Bytes::from(payload));
            yield Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"));
        }
    };

    crate::routes::streaming::sse_response(out_stream)
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

#[derive(Serialize, Deserialize)]
struct ChatChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<ChatChoice>,
}

#[derive(Serialize, Deserialize)]
struct ChatChoice {
    index: u32,
    delta: serde_json::Value,
    finish_reason: Option<String>,
    logprobs: Option<serde_json::Value>,
}

fn build_chat_chunk(id: &str, delta: &serde_json::Value, response: Option<&serde_json::Value>) -> ChatChunk {
    let model = response
        .and_then(|r| r.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("gpt-5.2-codex")
        .to_string();

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    ChatChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model,
        choices: vec![ChatChoice {
            index: 0,
            delta: delta.clone(),
            finish_reason: None,
            logprobs: None,
        }],
    }
}

fn convert_responses_to_chat(response: serde_json::Value, model: String) -> serde_json::Value {
    let output_text = response
        .get("output")
        .and_then(|o| o.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|x| x.get("type") == Some(&serde_json::Value::String("message".to_string())))
        })
        .and_then(|msg| msg.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.iter().find(|x| x.get("type") == Some(&serde_json::Value::String("output_text".to_string()))))
        .and_then(|t| t.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    serde_json::json!({
        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        "object": "chat.completion",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": output_text,
                },
                "logprobs": null,
                "finish_reason": "stop",
            }
        ],
        "usage": response.get("usage").cloned().unwrap_or(serde_json::json!({})),
    })
}

#[cfg(test)]
mod tests {
    use super::{build_chat_chunk, convert_responses_to_chat, find_double_newline, resolve_model_alias, requires_responses_api};

    #[test]
    fn resolves_claude_aliases() {
        assert_eq!(resolve_model_alias("claude-opus-4.5"), "gpt-5.2-codex");
        assert_eq!(resolve_model_alias("claude-3.5-haiku"), "gpt-5-mini");
        assert_eq!(resolve_model_alias("claude-2.1"), "gpt-5.1");
    }

    #[test]
    fn responses_api_required_models() {
        assert!(requires_responses_api("gpt-5.2-codex"));
        assert!(requires_responses_api("codex-5.2"));
        assert!(!requires_responses_api("gpt-4o"));
    }

    #[test]
    fn converts_responses_to_chat_with_usage() {
        let response = serde_json::json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "hello" }]
            }],
            "usage": { "input_tokens": 3, "output_tokens": 2, "total_tokens": 5 }
        });

        let converted = convert_responses_to_chat(response, "gpt-5.2-codex".to_string());
        let text = converted
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        assert_eq!(text, "hello");
        assert!(converted.get("usage").is_some());
    }

    #[test]
    fn finds_double_newline_in_buffer() {
        let buf = b"data: {\"a\":1}\n\nrest";
        assert_eq!(find_double_newline(buf), Some(13));
    }

    #[test]
    fn build_chat_chunk_defaults_model_when_missing() {
        let delta = serde_json::json!({"role": "assistant"});
        let chunk = build_chat_chunk("chatcmpl-1", &delta, None);
        assert_eq!(chunk.id, "chatcmpl-1");
        assert_eq!(chunk.model, "gpt-5.2-codex");
        assert_eq!(chunk.choices.len(), 1);
    }
}
