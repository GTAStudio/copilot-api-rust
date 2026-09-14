use axum::{
    Json,
    extract::{FromRequest, Request},
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::errors::{ApiError, ApiResult};

pub struct ApiJson<T>(pub T);

fn validate_model(model: &str) -> ApiResult<()> {
    if model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("Invalid model identifier".to_string()));
    }
    Ok(())
}

fn validate_options(options: &serde_json::Map<String, Value>) -> ApiResult<()> {
    for name in [
        "base_url",
        "api_key",
        "authorization",
        "headers",
        "provider",
        "proxy",
    ] {
        if options.contains_key(name) {
            return Err(ApiError::BadRequest(
                "Transport settings are not accepted in API request bodies".to_string(),
            ));
        }
    }
    for name in ["store", "parallel_tool_calls", "background"] {
        if options
            .get(name)
            .is_some_and(|value| !value.is_null() && !value.is_boolean())
        {
            return Err(ApiError::BadRequest(format!("Invalid {name}")));
        }
    }
    if let Some(value) = options.get("max_completion_tokens")
        && !value
            .as_u64()
            .is_some_and(|value| value > 0 && value <= u32::MAX as u64)
    {
        return Err(ApiError::BadRequest(
            "Invalid max_completion_tokens".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_chat(payload: &crate::services::copilot::ChatCompletionsPayload) -> ApiResult<()> {
    validate_model(&payload.model)?;
    validate_options(&payload.extra)?;
    if payload.messages.is_empty()
        || payload.messages.len() > 100_000
        || payload.max_tokens == Some(0)
        || payload.n == Some(0)
        || payload
            .temperature
            .is_some_and(|value| !(0.0..=2.0).contains(&value))
        || payload
            .top_p
            .is_some_and(|value| !(0.0..=1.0).contains(&value))
    {
        return Err(ApiError::BadRequest(
            "Invalid chat messages or generation parameters".to_string(),
        ));
    }
    for message in &payload.messages {
        if !matches!(
            message.role.as_str(),
            "user" | "assistant" | "system" | "developer" | "tool"
        ) || (!message.content.is_string()
            && !message.content.is_array()
            && !(message.role == "assistant"
                && message.tool_calls.is_some()
                && message.content.is_null()))
            || (message.role == "tool"
                && message.tool_call_id.as_ref().is_none_or(|id| id.is_empty()))
        {
            return Err(ApiError::BadRequest(
                "Invalid chat message role or content".to_string(),
            ));
        }
        for call in message.tool_calls.iter().flatten() {
            if call.id.is_empty() || call.function.name.is_empty() || call.r#type != "function" {
                return Err(ApiError::BadRequest("Invalid function call".to_string()));
            }
        }
    }
    for tool in payload.tools.iter().flatten() {
        if tool.r#type != "function"
            || tool.function.name.is_empty()
            || !tool.function.parameters.is_object()
        {
            return Err(ApiError::BadRequest("Invalid tool definition".to_string()));
        }
    }
    Ok(())
}

pub fn validate_responses(payload: &crate::services::copilot::ResponsesPayload) -> ApiResult<()> {
    validate_model(&payload.model)?;
    validate_options(&payload.extra)?;
    let input_valid = payload.input.as_str().is_some_and(|text| !text.is_empty())
        || payload
            .input
            .as_array()
            .is_some_and(|items| !items.is_empty() && items.iter().all(Value::is_object));
    if !input_valid
        || payload.max_output_tokens == Some(0)
        || payload
            .temperature
            .is_some_and(|value| !(0.0..=2.0).contains(&value))
        || payload
            .top_p
            .is_some_and(|value| !(0.0..=1.0).contains(&value))
    {
        return Err(ApiError::BadRequest(
            "Invalid Responses input or generation parameters".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_embedding(payload: &crate::services::copilot::EmbeddingRequest) -> ApiResult<()> {
    validate_model(&payload.model)?;
    if payload.dimensions == Some(0)
        || payload
            .encoding_format
            .as_deref()
            .is_some_and(|format| !matches!(format, "float" | "base64"))
        || payload
            .user
            .as_ref()
            .is_some_and(|user| user.len() > 256 || user.chars().any(char::is_control))
    {
        return Err(ApiError::BadRequest(
            "Invalid embedding output parameters".to_string(),
        ));
    }
    let valid = payload.input.as_str().is_some_and(|text| !text.is_empty())
        || payload.input.as_array().is_some_and(|items| {
            !items.is_empty()
                && (items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|text| !text.is_empty()))
                    || items.iter().all(|item| item.as_u64().is_some())
                    || items.iter().all(|item| {
                        item.as_array().is_some_and(|tokens| {
                            !tokens.is_empty()
                                && tokens.iter().all(|token| token.as_u64().is_some())
                        })
                    }))
        });
    if !valid {
        return Err(ApiError::BadRequest("Invalid embedding input".to_string()));
    }
    Ok(())
}

impl<State, Payload> FromRequest<State> for ApiJson<Payload>
where
    State: Send + Sync,
    Payload: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &State) -> Result<Self, Self::Rejection> {
        Json::<Payload>::from_request(request, state)
            .await
            .map(|Json(payload)| Self(payload))
            .map_err(|error| {
                if error.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
                    ApiError::PayloadTooLarge
                } else {
                    ApiError::BadRequest("Invalid JSON request body".to_string())
                }
            })
    }
}

pub fn validate_anthropic(payload: &Value, count_only: bool) -> ApiResult<()> {
    let invalid = || ApiError::BadRequest("Invalid Anthropic message request".to_string());
    let object = payload.as_object().ok_or_else(invalid)?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    if model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err(invalid());
    }
    for field in [
        "base_url",
        "api_key",
        "authorization",
        "headers",
        "provider",
        "proxy",
    ] {
        if object.contains_key(field) {
            return Err(ApiError::BadRequest(
                "Transport settings cannot be supplied in a message request".to_string(),
            ));
        }
    }
    if !count_only || object.contains_key("max_tokens") {
        let tokens = payload
            .get("max_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        if tokens > u32::MAX as u64 {
            return Err(invalid());
        }
    }
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if messages.is_empty() || messages.len() > 100_000 {
        return Err(invalid());
    }
    for message in messages {
        if !matches!(
            message.get("role").and_then(Value::as_str),
            Some("user" | "assistant" | "system")
        ) {
            return Err(invalid());
        }
        validate_content(message.get("content").ok_or_else(invalid)?)?;
    }
    if let Some(system) = payload.get("system").filter(|value| !value.is_null()) {
        validate_content(system)?;
    }
    for field in ["temperature", "top_p"] {
        if let Some(value) = payload.get(field).filter(|value| !value.is_null()) {
            let value = value.as_f64().ok_or_else(invalid)?;
            if !(0.0..=1.0).contains(&value) {
                return Err(invalid());
            }
        }
    }
    if payload
        .get("stream")
        .is_some_and(|value| !value.is_null() && !value.is_boolean())
    {
        return Err(invalid());
    }
    if let Some(thinking) = payload.get("thinking").filter(|value| !value.is_null()) {
        match thinking.get("type").and_then(Value::as_str) {
            Some("adaptive" | "disabled") => {}
            Some("enabled") => {
                let budget = thinking
                    .get("budget_tokens")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                if budget < 1024
                    || payload
                        .get("max_tokens")
                        .and_then(Value::as_u64)
                        .is_some_and(|maximum| budget >= maximum)
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
    }
    for field in [
        "metadata",
        "output_config",
        "context_management",
        "cache_control",
        "tool_choice",
    ] {
        if payload
            .get(field)
            .is_some_and(|value| !value.is_null() && !value.is_object())
        {
            return Err(invalid());
        }
    }
    if let Some(choice) = payload.get("tool_choice").filter(|value| !value.is_null()) {
        match choice.get("type").and_then(Value::as_str) {
            Some("auto" | "any" | "none") => {}
            Some("tool") => {
                let name = choice
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        if choice
            .get("disable_parallel_tool_use")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(invalid());
        }
    }
    if let Some(tools) = payload.get("tools").filter(|value| !value.is_null()) {
        for tool in tools.as_array().ok_or_else(invalid)? {
            if !tool.is_object()
                || (!tool.get("name").is_some_and(Value::is_string)
                    && !tool.get("type").is_some_and(Value::is_string))
            {
                return Err(invalid());
            }
            if tool
                .get("input_schema")
                .is_some_and(|value| !value.is_object())
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn validate_content(content: &Value) -> ApiResult<()> {
    let invalid = || {
        ApiError::BadRequest(
            "Message content must be text or an array of typed content blocks".to_string(),
        )
    };
    if content.is_string() {
        return Ok(());
    }
    let blocks = content.as_array().ok_or_else(invalid)?;
    if blocks.is_empty() {
        return Err(invalid());
    }
    for block in blocks {
        let kind = block
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if kind.is_empty() || kind.len() > 128 {
            return Err(invalid());
        }
        match kind {
            "text" if !block.get("text").is_some_and(Value::is_string) => return Err(invalid()),
            "thinking"
                if !block.get("thinking").is_some_and(Value::is_string)
                    || !block.get("signature").is_some_and(Value::is_string) =>
            {
                return Err(invalid());
            }
            "tool_use"
                if !block.get("id").is_some_and(Value::is_string)
                    || !block.get("name").is_some_and(Value::is_string)
                    || !block.get("input").is_some_and(Value::is_object) =>
            {
                return Err(invalid());
            }
            "tool_result" => {
                if !block.get("tool_use_id").is_some_and(Value::is_string) {
                    return Err(invalid());
                }
                if let Some(result) = block.get("content")
                    && !result.as_array().is_some_and(Vec::is_empty)
                {
                    validate_content(result)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn validate_translation(payload: &Value) -> ApiResult<()> {
    let unsupported = || {
        ApiError::BadRequest(
            "This request requires a model with a native Anthropic Messages endpoint".to_string(),
        )
    };
    let allowed = [
        "model",
        "messages",
        "max_tokens",
        "system",
        "metadata",
        "stop_sequences",
        "stream",
        "temperature",
        "top_p",
        "top_k",
        "tools",
        "tool_choice",
    ];
    if payload.as_object().is_some_and(|object| {
        object
            .iter()
            .any(|(key, value)| !allowed.contains(&key.as_str()) && !value.is_null())
    }) {
        return Err(unsupported());
    }
    if payload.get("top_k").is_some_and(|value| !value.is_null())
        || payload.get("max_tokens").and_then(Value::as_u64) == Some(0)
    {
        return Err(unsupported());
    }
    for message in payload
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if message.as_object().is_some_and(|object| {
            object
                .keys()
                .any(|name| !["role", "content"].contains(&name.as_str()))
        }) {
            return Err(unsupported());
        }
    }
    for (name, allowed) in [
        ("metadata", &["user_id"][..]),
        (
            "tool_choice",
            &["type", "name", "disable_parallel_tool_use"][..],
        ),
    ] {
        if payload
            .get(name)
            .and_then(Value::as_object)
            .is_some_and(|object| object.keys().any(|key| !allowed.contains(&key.as_str())))
        {
            return Err(unsupported());
        }
    }
    let contents = payload
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content"))
        .chain(payload.get("system"));
    for content in contents {
        validate_translation_content(content)?;
    }
    if let Some(tools) = payload.get("tools").and_then(Value::as_array) {
        for tool in tools {
            if !tool.get("input_schema").is_some_and(Value::is_object)
                || tool.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .any(|key| !["name", "description", "input_schema"].contains(&key.as_str()))
                })
            {
                return Err(unsupported());
            }
        }
    }
    Ok(())
}

fn validate_translation_content(content: &Value) -> ApiResult<()> {
    for block in content.as_array().into_iter().flatten() {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        let fields: &[&str] = match kind {
            "text" => &["type", "text"],
            "image" => &["type", "source"],
            "tool_use" => &["type", "id", "name", "input"],
            "tool_result" => &["type", "tool_use_id", "content", "is_error"],
            _ => &[],
        };
        if fields.is_empty()
            || block
                .as_object()
                .is_some_and(|object| object.keys().any(|key| !fields.contains(&key.as_str())))
            || block
                .get("is_error")
                .is_some_and(|value| value != &Value::Bool(false))
        {
            return Err(ApiError::BadRequest(
                "Content requires a native Anthropic Messages endpoint".to_string(),
            ));
        }
        if kind == "image" && !matches!(block["source"]["type"].as_str(), Some("base64" | "url")) {
            return Err(ApiError::BadRequest(
                "Image requires a native Anthropic Messages endpoint".to_string(),
            ));
        }
        if kind == "image"
            && block["source"].as_object().is_some_and(|source| {
                source
                    .keys()
                    .any(|key| !["type", "media_type", "data", "url"].contains(&key.as_str()))
            })
        {
            return Err(ApiError::BadRequest(
                "Image requires a native Anthropic Messages endpoint".to_string(),
            ));
        }
        if let Some(result) = block.get("content") {
            validate_translation_content(result)?;
        }
    }
    Ok(())
}

pub fn anthropic_headers(incoming: &HeaderMap) -> ApiResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in incoming {
        if name.as_str().starts_with("anthropic-") || name.as_str().starts_with("x-claude-code-") {
            if value.as_bytes().len() > 8192 {
                return Err(ApiError::BadRequest(
                    "Protocol header is too large".to_string(),
                ));
            }
            headers.append(name.clone(), value.clone());
        }
    }
    headers
        .entry("anthropic-version")
        .or_insert(HeaderValue::from_static("2023-06-01"));
    Ok(headers)
}

pub fn responses_tools(tools: &[crate::services::copilot::Tool]) -> Value {
    Value::Array(tools.iter().map(|tool| {
        let mut definition = serde_json::json!({
            "type": "function", "name": tool.function.name, "parameters": tool.function.parameters
        });
        if let Some(description) = &tool.function.description {
            definition["description"] = description.clone().into();
        }
        if let Some(strict) = tool.function.strict {
            definition["strict"] = strict.into();
        }
        definition
    }).collect())
}

pub fn responses_tool_choice(choice: &Value) -> ApiResult<Value> {
    if matches!(choice.as_str(), Some("auto" | "none" | "required")) {
        return Ok(choice.clone());
    }
    if choice["type"] == "function"
        && let Some(name) = choice["function"]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
    {
        return Ok(serde_json::json!({"type": "function", "name": name}));
    }
    Err(ApiError::BadRequest(
        "Tool choice cannot be converted to the Responses API".to_string(),
    ))
}

pub fn responses_options(chat: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let mut options = serde_json::Map::new();
    for name in [
        "parallel_tool_calls",
        "metadata",
        "store",
        "service_tier",
        "prompt_cache_key",
        "safety_identifier",
    ] {
        if let Some(value) = chat.get(name) {
            options.insert(name.to_string(), value.clone());
        }
    }
    if let Some(effort) = chat.get("reasoning_effort") {
        options.insert(
            "reasoning".to_string(),
            serde_json::json!({"effort": effort}),
        );
    }
    options
}

pub async fn relay_response(upstream: reqwest::Response, streaming: bool) -> ApiResult<Response> {
    Ok(relay_response_with_output(upstream, streaming).await?.0)
}

pub async fn relay_response_with_output(
    mut upstream: reqwest::Response,
    streaming: bool,
) -> ApiResult<(Response, Option<Value>)> {
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("");
    if (streaming && content_type != "text/event-stream")
        || (!streaming && content_type != "application/json" && !content_type.ends_with("+json"))
    {
        return Err(ApiError::Upstream(
            "Invalid upstream response content type".to_string(),
        ));
    }
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if matches!(
            name.as_str(),
            "content-type" | "request-id" | "x-request-id" | "retry-after"
        ) || name.as_str().starts_with("anthropic-ratelimit-")
        {
            headers.append(name.clone(), value.clone());
        }
    }
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    let (body, output) = if streaming {
        (
            axum::body::Body::from_stream(crate::services::copilot::response_body_stream(upstream)),
            None,
        )
    } else {
        let mut bytes = Vec::new();
        while let Some(chunk) = upstream
            .chunk()
            .await
            .map_err(|_| ApiError::Upstream("Incomplete upstream response".to_string()))?
        {
            if bytes.len().saturating_add(chunk.len()) > 32 * 1024 * 1024 {
                return Err(ApiError::Upstream(
                    "Upstream JSON response exceeds the size limit".to_string(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::Upstream("Invalid upstream JSON response".to_string()))?;
        if !json.is_object() {
            return Err(ApiError::Upstream(
                "Invalid upstream response shape".to_string(),
            ));
        }
        (axum::body::Body::from(bytes), Some(json))
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok((response, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::copilot::{ChatCompletionsPayload, EmbeddingRequest, ResponsesPayload};

    #[test]
    fn fallback_rejects_message_fields_and_controls_it_cannot_preserve() {
        let base = serde_json::json!({"model": "fixture", "max_tokens": 4096, "messages": [{"role": "user", "content": "hello"}]});
        for change in [
            serde_json::json!({"messages": [{"role": "user", "content": "hello", "cache_control": {"type": "ephemeral"}}]}),
            serde_json::json!({"messages": [{"role": "assistant", "content": [{"type": "text", "text": "answer", "citations": [{"type": "char_location", "start_char_index": 0}]}]}]}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_fixture", "is_error": true, "content": "failed"}]}]}),
            serde_json::json!({"tool_choice": {"type": "auto", "future_control": true}}),
            serde_json::json!({"metadata": {"user_id": "fixture", "future_metadata": "preserve"}}),
        ] {
            let mut payload = base.clone();
            payload
                .as_object_mut()
                .expect("base")
                .extend(change.as_object().expect("change").clone());
            assert!(
                validate_anthropic(&payload, false).is_ok(),
                "native extension {change}"
            );
            assert!(
                validate_translation(&payload).is_err(),
                "must not discard {change}"
            );
        }
    }

    #[test]
    fn anthropic_tool_choice_is_validated_before_any_forwarding() {
        let base = serde_json::json!({"model": "fixture", "max_tokens": 4096, "messages": [{"role": "user", "content": "hello"}]});
        for choice in [
            serde_json::json!({"type": "auto"}),
            serde_json::json!({"type": "none"}),
            serde_json::json!({"type": "any", "disable_parallel_tool_use": true}),
            serde_json::json!({"type": "tool", "name": "lookup", "disable_parallel_tool_use": false}),
        ] {
            let mut payload = base.clone();
            payload["tool_choice"] = choice;
            assert!(validate_anthropic(&payload, false).is_ok());
        }
        for choice in [
            serde_json::json!({}),
            serde_json::json!({"type": "invalid"}),
            serde_json::json!({"type": "tool"}),
            serde_json::json!({"type": "tool", "name": ""}),
            serde_json::json!({"type": "tool", "name": 5}),
            serde_json::json!({"type": "auto", "disable_parallel_tool_use": "false"}),
        ] {
            let mut payload = base.clone();
            payload["tool_choice"] = choice.clone();
            assert!(
                validate_anthropic(&payload, false).is_err(),
                "invalid choice {choice}"
            );
        }
        let mut payload = base;
        payload["messages"][0]["role"] = "tool".into();
        assert!(validate_anthropic(&payload, false).is_err());
    }

    #[test]
    fn validates_chat_public_input_before_forwarding() {
        let base = serde_json::json!({"model": "gpt-5.4", "messages": [{"role": "user", "content": "hello"}]});
        let valid: ChatCompletionsPayload = serde_json::from_value(base.clone()).expect("chat");
        assert!(validate_chat(&valid).is_ok());
        for change in [
            serde_json::json!({"messages": []}),
            serde_json::json!({"model": ""}),
            serde_json::json!({"messages": [{"role": "invalid", "content": "hello"}]}),
            serde_json::json!({"max_completion_tokens": 0}),
            serde_json::json!({"base_url": "https://untrusted.example"}),
            serde_json::json!({"temperature": 3}),
            serde_json::json!({"messages": [{"role": "tool", "content": "result"}]}),
        ] {
            let mut raw = base.clone();
            raw.as_object_mut()
                .expect("base")
                .extend(change.as_object().expect("change").clone());
            let payload = serde_json::from_value(raw).expect("chat shape");
            assert!(validate_chat(&payload).is_err(), "{change}");
        }
    }

    #[test]
    fn validates_responses_and_embedding_public_input() {
        for input in [
            serde_json::json!("hello"),
            serde_json::json!([{ "role": "user", "content": "hello" }]),
        ] {
            let payload: ResponsesPayload =
                serde_json::from_value(serde_json::json!({"model": "gpt-5.4", "input": input}))
                    .expect("responses");
            assert!(validate_responses(&payload).is_ok());
        }
        for input in [
            serde_json::json!(null),
            serde_json::json!(42),
            serde_json::json!([]),
        ] {
            let payload =
                serde_json::from_value(serde_json::json!({"model": "gpt-5.4", "input": input}))
                    .expect("responses");
            assert!(validate_responses(&payload).is_err());
        }
        let valid: EmbeddingRequest = serde_json::from_value(
            serde_json::json!({"model": "text-embedding-3-small", "input": [1, 2, 3]}),
        )
        .expect("embedding");
        assert!(validate_embedding(&valid).is_ok());
        let invalid = serde_json::from_value(serde_json::json!({"model": "", "input": {}}))
            .expect("embedding");
        assert!(validate_embedding(&invalid).is_err());
    }

    #[test]
    fn translation_rejects_unrepresentable_blocks_instead_of_flattening_them() {
        for block in [
            serde_json::json!({"type": "thinking", "thinking": "fixture", "signature": "opaque"}),
            serde_json::json!({"type": "text", "text": "fixture", "cache_control": {"type": "ephemeral"}}),
            serde_json::json!({"type": "document", "source": {"type": "file", "file_id": "file_fixture"}}),
        ] {
            let payload = serde_json::json!({"model": "claude-sonnet-4.6", "max_tokens": 32, "messages": [{"role": "assistant", "content": [block]}]});
            assert!(validate_translation(&payload).is_err());
        }
    }

    #[test]
    fn anthropic_core_validation_accepts_current_blocks_and_rejects_invalid_controls() {
        let base = serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 4096, "messages": [{"role": "user", "content": "hello"}]});
        for update in [
            serde_json::json!({"thinking": {"type": "enabled", "budget_tokens": 1024}}),
            serde_json::json!({"thinking": {"type": "disabled"}, "temperature": 0.5, "top_p": 1.0}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_fixture", "content": []}]}]}),
            serde_json::json!({"messages": [{"role": "assistant", "content": [{"type": "thinking", "thinking": "fixture", "signature": "opaque"}, {"type": "tool_use", "id": "call_fixture", "name": "example", "input": {}}]}]}),
            serde_json::json!({"system": [{"type": "text", "text": "context"}], "tools": [{"type": "tool_search_tool_bm25", "name": "tool_search"}]}),
        ] {
            let mut payload = base.clone();
            payload
                .as_object_mut()
                .expect("base")
                .extend(update.as_object().expect("update").clone());
            assert!(validate_anthropic(&payload, false).is_ok(), "{update}");
        }
        for update in [
            serde_json::json!({"max_tokens": -1}),
            serde_json::json!({"stream": "true"}),
            serde_json::json!({"temperature": -0.1}),
            serde_json::json!({"top_p": 2}),
            serde_json::json!({"thinking": {"type": "enabled", "budget_tokens": 4096}}),
            serde_json::json!({"thinking": {"type": "enabled", "budget_tokens": 1}}),
            serde_json::json!({"thinking": {"type": "unknown"}}),
            serde_json::json!({"tools": [{"name": "example", "input_schema": []}]}),
            serde_json::json!({"tools": ["invalid"]}),
            serde_json::json!({"metadata": false}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "text", "text": 1}]}]}),
            serde_json::json!({"messages": [{"role": "assistant", "content": [{"type": "thinking", "thinking": "missing signature"}]}]}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "tool_result", "content": "missing ID"}]}]}),
            serde_json::json!({"headers": {"authorization": "must not override"}}),
        ] {
            let mut payload = base.clone();
            payload
                .as_object_mut()
                .expect("base")
                .extend(update.as_object().expect("update").clone());
            assert!(validate_anthropic(&payload, false).is_err(), "{update}");
        }
    }

    #[tokio::test]
    async fn json_extractor_preserves_request_too_large_status() {
        use tower::ServiceExt;
        let app = axum::Router::new()
            .route(
                "/",
                axum::routing::post(|ApiJson(_): ApiJson<Value>| async { "ok" }),
            )
            .layer(axum::extract::DefaultBodyLimit::max(32));
        let request = axum::http::Request::post("/")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({"large": "x".repeat(64)}).to_string(),
            ))
            .expect("request");
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("error body");
        let body: Value = serde_json::from_slice(&bytes).expect("error JSON");
        assert_eq!(body["error"]["type"], "request_too_large");
    }
}
