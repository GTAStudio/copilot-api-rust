use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::hooks::HookExecutor;

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub client: reqwest::Client,
    pub hooks: Option<Arc<HookExecutor>>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub account_type: String,
    pub github_token: Option<String>,
    pub copilot_token: Option<String>,
    pub show_token: bool,
    pub vscode_version: String,
    pub models: Option<ModelsResponse>,
    pub manual_approve: bool,
    pub rate_limit_seconds: Option<u64>,
    pub rate_limit_wait: bool,
    pub last_request_timestamp: Option<std::time::Instant>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            account_type: std::env::var("COPILOT_ACCOUNT_TYPE").unwrap_or_else(|_| "individual".to_string()),
            github_token: std::env::var("COPILOT_GITHUB_TOKEN").ok(),
            copilot_token: None,
            show_token: std::env::var("COPILOT_SHOW_TOKEN").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false),
            vscode_version: "1.104.3".to_string(),
            models: None,
            manual_approve: std::env::var("COPILOT_MANUAL_APPROVE").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false),
            rate_limit_seconds: std::env::var("COPILOT_RATE_LIMIT").ok().and_then(|v| v.parse::<u64>().ok()),
            rate_limit_wait: std::env::var("COPILOT_RATE_LIMIT_WAIT").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false),
            last_request_timestamp: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<Model>,
    pub object: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub capabilities: ModelCapabilities,
    pub id: String,
    pub model_picker_enabled: bool,
    pub name: String,
    pub object: String,
    pub preview: bool,
    pub vendor: String,
    pub version: String,
    #[serde(default)]
    pub policy: Option<ModelPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPolicy {
    pub state: String,
    pub terms: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub family: String,
    pub limits: ModelLimits,
    pub object: String,
    pub supports: ModelSupports,
    pub tokenizer: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLimits {
    pub max_context_window_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub max_prompt_tokens: Option<u32>,
    pub max_inputs: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSupports {
    pub tool_calls: Option<bool>,
    pub parallel_tool_calls: Option<bool>,
    pub dimensions: Option<bool>,
}
