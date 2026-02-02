use once_cell::sync::Lazy;
use tiktoken_rs::CoreBPE;

use crate::services::copilot::{ChatCompletionsPayload, Message, ToolCall};

static O200K: Lazy<CoreBPE> = Lazy::new(|| tiktoken_rs::o200k_base().expect("o200k_base"));
static CL100K: Lazy<CoreBPE> = Lazy::new(|| tiktoken_rs::cl100k_base().expect("cl100k_base"));
static P50K: Lazy<CoreBPE> = Lazy::new(|| tiktoken_rs::p50k_base().expect("p50k_base"));
static P50K_EDIT: Lazy<CoreBPE> = Lazy::new(|| tiktoken_rs::p50k_edit().expect("p50k_edit"));
static R50K: Lazy<CoreBPE> = Lazy::new(|| tiktoken_rs::r50k_base().expect("r50k_base"));

#[derive(Debug, Clone, Copy)]
struct TokenConstants {
    func_init: usize,
    func_end: usize,
    tokens_per_message: usize,
    tokens_per_name: usize,
}

fn constants_for_model(model: &str) -> TokenConstants {
    if model == "gpt-3.5-turbo" || model == "gpt-4" {
        TokenConstants {
            func_init: 10,
            func_end: 12,
            tokens_per_message: 3,
            tokens_per_name: 1,
        }
    } else {
        TokenConstants {
            func_init: 7,
            func_end: 12,
            tokens_per_message: 3,
            tokens_per_name: 1,
        }
    }
}

fn encoder_from_tokenizer(name: &str) -> &CoreBPE {
    match name {
        "cl100k_base" => &CL100K,
        "p50k_base" => &P50K,
        "p50k_edit" => &P50K_EDIT,
        "r50k_base" => &R50K,
        _ => &O200K,
    }
}

pub fn estimate_chat_tokens(payload: &ChatCompletionsPayload, tokenizer: &str) -> u64 {
    let encoder = encoder_from_tokenizer(tokenizer);
    let constants = constants_for_model(&payload.model);

    let mut tokens: usize = 0;
    for message in &payload.messages {
        tokens += constants.tokens_per_message;
        tokens += message_tokens(message, encoder, constants);
    }

    // every reply is primed with <|start|>assistant<|message|>
    tokens += 3;
    tokens as u64
}

fn message_tokens(message: &Message, encoder: &CoreBPE, constants: TokenConstants) -> usize {
    let mut tokens = 0;
    if let Some(name) = &message.name {
        tokens += constants.tokens_per_name + encoder.encode_ordinary(name).len();
    }

    match &message.content {
        serde_json::Value::String(text) => {
            tokens += encoder.encode_ordinary(text).len();
        }
        serde_json::Value::Array(arr) => {
            for part in arr {
                if let Some(kind) = part.get("type").and_then(|v| v.as_str()) {
                    if kind == "text" {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            tokens += encoder.encode_ordinary(text).len();
                        }
                    } else if kind == "image_url" {
                        if let Some(url) = part.get("image_url").and_then(|v| v.get("url")).and_then(|v| v.as_str()) {
                            tokens += encoder.encode_ordinary(url).len() + 85;
                        }
                    }
                }
            }
        }
        _ => {}
    }

    if let Some(tool_calls) = &message.tool_calls {
        tokens += tool_calls_tokens(tool_calls, encoder, constants);
    }

    tokens
}

fn tool_calls_tokens(tool_calls: &Vec<ToolCall>, encoder: &CoreBPE, constants: TokenConstants) -> usize {
    let mut tokens = 0;
    for tool_call in tool_calls {
        tokens += constants.func_init;
        let json = serde_json::to_string(tool_call).unwrap_or_default();
        tokens += encoder.encode_ordinary(&json).len();
    }
    tokens += constants.func_end;
    tokens
}

pub fn use_precise_tokenizer() -> bool {
    std::env::var("COPILOT_USE_TIKTOKEN")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{estimate_chat_tokens, encoder_from_tokenizer};
    use crate::services::copilot::{ChatCompletionsPayload, Message};

    #[test]
    fn encoder_exists_for_o200k() {
        let _ = encoder_from_tokenizer("o200k_base");
    }

    #[test]
    fn estimates_tokens_for_simple_payload() {
        let payload = ChatCompletionsPayload {
            model: "gpt-5.2-codex".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: serde_json::Value::String("hello world".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop: None,
            n: None,
            stream: None,
            frequency_penalty: None,
            presence_penalty: None,
            logit_bias: None,
            logprobs: None,
            response_format: None,
            seed: None,
            tools: None,
            tool_choice: None,
            user: None,
        };

        let count = estimate_chat_tokens(&payload, "o200k_base");
        assert!(count > 0);
    }
}
