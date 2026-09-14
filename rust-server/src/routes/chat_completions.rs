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
    rate_limit::check_rate_limit,
    routes::responses::{extract_instructions, messages_to_responses_input},
    services::{
        azure,
        copilot::{
            ChatCompletionsPayload, ResponsesPayload, create_chat_completions, create_responses,
        },
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
    model.to_string()
}

fn requires_responses_api(model: &str) -> bool {
    RESPONSES_API_MODELS.contains(&model) || matches!(model, "codex-5.2" | "codex-5.1")
}

pub async fn handle(
    State(state): State<AppState>,
    crate::protocol::ApiJson(mut payload): crate::protocol::ApiJson<ChatCompletionsPayload>,
) -> ApiResult<Response> {
    crate::protocol::validate_chat(&payload)?;
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

    if (provider == "azure" || payload.model.starts_with("azure:"))
        && let Some(cfg) = azure::load_azure_config(&payload.model)
    {
        let mut azure_payload = payload.clone();
        if azure_payload.model.starts_with("azure:") {
            azure_payload.model = cfg.deployment.clone();
        }
        let resp = azure::create_chat_completions(
            &state.client,
            &cfg,
            &serde_json::to_value(&azure_payload).unwrap(),
        )
        .await?;
        return relay_chat_response(&state, &payload, resp).await;
    }

    if provider == "openai" || payload.model.starts_with("openai:") {
        if payload.model.starts_with("openai:") {
            payload.model = payload.model.trim_start_matches("openai:").to_string();
        }

        if requires_responses_api(&payload.model) {
            return Err(ApiError::BadRequest(
                "Model requires /v1/responses when using OpenAI provider".to_string(),
            ));
        }

        let resp = openai::create_chat_completions(
            &state.client,
            &serde_json::to_value(&payload).unwrap(),
        )
        .await?;
        return relay_chat_response(&state, &payload, resp).await;
    }

    if provider != "copilot" {
        return Err(ApiError::BadRequest(
            "The configured provider does not support this Chat request".to_string(),
        ));
    }
    let token = ensure_copilot_token(&state).await?;
    crate::services::copilot::ensure_models(&state, &token).await?;
    let catalogue = state.config.read().await.clone();

    let original_model = payload.model.clone();
    payload.model =
        crate::config::resolve_model_id(&catalogue, &resolve_model_alias(&payload.model));

    if crate::config::supports_endpoint(&catalogue, &payload.model, "/responses")
        && !crate::config::supports_endpoint(&catalogue, &payload.model, "/chat/completions")
        || requires_responses_api(&payload.model)
    {
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
            let estimate = crate::utils::estimate_tokens_from_json(
                &serde_json::to_value(&payload).unwrap_or_default(),
            );
            tracing::info!("Token count (heuristic): {}", estimate);
        }
    }

    let config = state.config.read().await.clone();

    if payload.max_tokens.is_none()
        && !payload.extra.contains_key("max_completion_tokens")
        && let Some(models) = &config.models
        && let Some(model) = models.data.iter().find(|m| m.id == payload.model)
    {
        payload.max_tokens = model.capabilities.limits.max_output_tokens;
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

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid response: {e}")))?;
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

async fn relay_chat_response(
    state: &AppState,
    payload: &ChatCompletionsPayload,
    upstream: reqwest::Response,
) -> ApiResult<Response> {
    let (response, output) =
        crate::protocol::relay_response_with_output(upstream, payload.stream.unwrap_or(false))
            .await?;
    if let Some(hooks) = &state.hooks {
        hooks
            .post_tool_use(
                "ChatCompletions",
                serde_json::to_value(payload).unwrap_or_default(),
                output,
            )
            .await;
    }
    Ok(response)
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
        extra: crate::protocol::responses_options(&payload.extra),
        model: payload.model.clone(),
        input: serde_json::to_value(input).unwrap_or(serde_json::json!([])),
        instructions,
        max_output_tokens: payload
            .extra
            .get("max_completion_tokens")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| value.try_into().ok())
            .or(payload.max_tokens),
        temperature: payload.temperature,
        top_p: payload.top_p,
        stream: payload.stream,
        tools: payload
            .tools
            .as_deref()
            .map(crate::protocol::responses_tools),
        tool_choice: payload
            .tool_choice
            .as_ref()
            .map(crate::protocol::responses_tool_choice)
            .transpose()?,
        previous_response_id: None,
    };

    let resp = create_responses(&state.client, &config, &token, &responses_payload).await?;

    if payload.stream.unwrap_or(false) {
        if let Some(hooks) = &state.hooks {
            hooks
                .post_tool_use(
                    "ChatCompletions",
                    serde_json::to_value(&payload).unwrap_or_default(),
                    None,
                )
                .await;
        }
        return Ok(stream_responses_as_chat_completion(
            resp,
            payload.model.clone(),
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Upstream(format!("Invalid responses payload: {e}")))?;
    let converted = convert_responses_to_chat(json, payload.model.clone());
    if let Some(hooks) = &state.hooks {
        hooks
            .post_tool_use(
                "ChatCompletions",
                serde_json::to_value(&payload).unwrap_or_default(),
                Some(converted.clone()),
            )
            .await;
    }
    Ok(Json(converted).into_response())
}

fn stream_responses_as_chat_completion(
    resp: reqwest::Response,
    model: String,
) -> axum::response::Response {
    let stream = responses_chat_events(resp, model);
    let out_stream = async_stream::stream! {
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => yield Ok(Bytes::from(format!("data: {chunk}\n\n"))),
                Err(error) => { yield Err(error); return; }
            }
        }
        yield Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"));
    };
    crate::routes::streaming::sse_response(out_stream)
}

pub(super) fn responses_chat_events(
    resp: reqwest::Response,
    model: String,
) -> impl futures::Stream<Item = Result<serde_json::Value, std::io::Error>> + Send {
    async_stream::stream! {
        let stream = crate::routes::streaming::json_events(resp);
        futures::pin_mut!(stream);
        let chat_id = format!("chatcmpl-{}", Uuid::new_v4());
        let model_info = serde_json::json!({"model": model});
        let make_chunk = |delta: serde_json::Value| serde_json::to_value(build_chat_chunk(&chat_id, &delta, Some(&model_info))).unwrap_or_default();
        let mut tools = std::collections::BTreeMap::<u64, (usize, bool)>::new();
        yield Ok(make_chunk(serde_json::json!({"role": "assistant"})));
        while let Some(event) = stream.next().await {
            let event = match event { Ok(event) => event, Err(error) => { yield Err(error); return; } };
            match event["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    if let Some(text) = event["delta"].as_str() {
                        yield Ok(make_chunk(serde_json::json!({"content": text})));
                    }
                }
                "response.output_item.added" | "response.output_item.done" if event["item"]["type"] == "function_call" => {
                    let output_index = event["output_index"].as_u64().unwrap_or(0);
                    let item = &event["item"];
                    if !tools.contains_key(&output_index) {
                        let index = tools.len();
                        tools.insert(output_index, (index, false));
                        yield Ok(make_chunk(serde_json::json!({"tool_calls": [{"index": index, "id": item["call_id"], "type": "function", "function": {"name": item["name"], "arguments": ""}}]})));
                    }
                    if let Some((index, arguments_sent)) = tools.get_mut(&output_index)
                        && !*arguments_sent
                            && let Some(arguments) = item["arguments"].as_str().filter(|arguments| !arguments.is_empty()) {
                                *arguments_sent = true;
                                yield Ok(make_chunk(serde_json::json!({"tool_calls": [{"index": index, "function": {"arguments": arguments}}]})));
                            }
                }
                "response.function_call_arguments.delta" => {
                    let output_index = event["output_index"].as_u64().unwrap_or(0);
                    let Some((index, sent)) = tools.get_mut(&output_index) else {
                        yield Err(std::io::Error::other("Tool argument delta has no tool definition")); return;
                    };
                    *sent = true;
                    yield Ok(make_chunk(serde_json::json!({"tool_calls": [{"index": index, "function": {"arguments": event["delta"]}}]})));
                }
                "response.completed" | "response.incomplete" => {
                    let mut final_chunk = make_chunk(serde_json::json!({}));
                    final_chunk["choices"][0]["finish_reason"] = if event["type"] == "response.incomplete" { "length" } else if tools.is_empty() { "stop" } else { "tool_calls" }.into();
                    final_chunk["usage"] = response_usage_to_chat(&event["response"]["usage"]);
                    yield Ok(final_chunk);
                    return;
                }
                "response.failed" | "response.cancelled" => {
                    yield Err(std::io::Error::other("Responses stream did not complete successfully")); return;
                }
                _ => {}
            }
        }
        yield Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Responses stream ended without a terminal event"));
    }
}

#[cfg(test)]
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

fn build_chat_chunk(
    id: &str,
    delta: &serde_json::Value,
    response: Option<&serde_json::Value>,
) -> ChatChunk {
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

fn response_usage_to_chat(usage: &serde_json::Value) -> serde_json::Value {
    let input = usage["input_tokens"].as_u64().unwrap_or(0);
    let output = usage["output_tokens"].as_u64().unwrap_or(0);
    serde_json::json!({
        "prompt_tokens": input, "completion_tokens": output, "total_tokens": input.saturating_add(output),
        "prompt_tokens_details": usage["input_tokens_details"], "completion_tokens_details": usage["output_tokens_details"]
    })
}

pub(super) fn convert_responses_to_chat(
    response: serde_json::Value,
    model: String,
) -> serde_json::Value {
    let mut output_text = String::new();
    let mut tool_calls = Vec::new();
    for item in response
        .get("output")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if item["type"] == "message" {
            for part in item["content"].as_array().into_iter().flatten() {
                if part["type"] == "output_text" {
                    output_text.push_str(part["text"].as_str().unwrap_or(""));
                }
            }
        } else if item["type"] == "function_call" {
            tool_calls.push(serde_json::json!({"id": item["call_id"], "type": "function", "function": {"name": item["name"], "arguments": item["arguments"]}}));
        }
    }
    let finish = if response["status"] == "incomplete" {
        "length"
    } else if tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let mut message = serde_json::json!({"role": "assistant", "content": output_text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = tool_calls.into();
    }

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
                "message": message,
                "logprobs": null,
                "finish_reason": finish,
            }
        ],
        "usage": response_usage_to_chat(&response["usage"]),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_chat_chunk, convert_responses_to_chat, find_double_newline, requires_responses_api,
        resolve_model_alias,
    };

    #[test]
    fn resolves_claude_aliases() {
        for model in [
            "claude-opus-4.5",
            "claude-sonnet-4-6",
            "claude-opus-4-8",
            "claude-sonnet-5",
            "o3",
            "o1",
            "gpt-5.4",
        ] {
            assert_eq!(resolve_model_alias(model), model);
        }
    }

    #[test]
    fn converts_responses_tool_calls_without_losing_arguments_or_usage() {
        let raw = serde_json::json!({
            "id": "resp_fixture", "status": "completed", "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "first"}, {"type": "output_text", "text": "second"}]},
                {"type": "function_call", "call_id": "call_fixture", "name": "weather", "arguments": "{\"city\":\"Paris\"}"}
            ], "usage": {"input_tokens": 12, "output_tokens": 4, "input_tokens_details": {"cached_tokens": 3}}
        });
        let converted = convert_responses_to_chat(raw, "gpt-5.4".to_string());
        assert_eq!(converted["choices"][0]["message"]["content"], "firstsecond");
        assert_eq!(
            converted["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"Paris\"}"
        );
        assert_eq!(converted["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(converted["usage"]["prompt_tokens"], 12);
        assert_eq!(converted["usage"]["completion_tokens"], 4);
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
