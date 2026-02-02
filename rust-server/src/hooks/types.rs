use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookInput {
    #[serde(default, alias = "event", alias = "hook")]
    pub hook_type: Option<String>,
    #[serde(default, alias = "tool_name", alias = "tool")]
    pub tool: Option<String>,
    #[serde(default, alias = "tool_input")]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default, alias = "tool_output", alias = "output")]
    pub tool_output: Option<serde_json::Value>,
    #[serde(default, alias = "session_id", alias = "session")]
    pub session_id: Option<String>,
}

impl HookInput {
    pub fn resolved_session_id(&self) -> Option<String> {
        self.session_id.clone().or_else(|| std::env::var("CLAUDE_SESSION_ID").ok())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    #[serde(rename = "type")]
    pub hook_type: String,
    pub command: Option<String>,
    pub name: Option<String>,
    pub timeout: Option<u64>,
    #[serde(default, rename = "async")]
    pub is_async: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for HookEntry {
    fn default() -> Self {
        Self {
            hook_type: "builtin".to_string(),
            command: None,
            name: None,
            timeout: None,
            is_async: false,
            enabled: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    pub matcher: String,
    pub hooks: Vec<HookEntry>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HooksJson {
    #[serde(default)]
    pub hooks: std::collections::HashMap<String, Vec<HookConfig>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}
