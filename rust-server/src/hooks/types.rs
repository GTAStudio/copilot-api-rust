use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookInput {
    #[serde(default, alias = "event", alias = "hook", alias = "hook_event_name")]
    pub hook_type: Option<String>,
    #[serde(default, alias = "tool_name", alias = "tool")]
    pub tool: Option<String>,
    #[serde(default, alias = "tool_input")]
    pub tool_input: Option<serde_json::Value>,
    #[serde(
        default,
        alias = "tool_output",
        alias = "output",
        alias = "tool_response"
    )]
    pub tool_output: Option<serde_json::Value>,
    #[serde(default, alias = "session_id", alias = "session")]
    pub session_id: Option<String>,
}

impl HookInput {
    pub fn resolved_session_id(&self) -> Option<String> {
        self.session_id
            .clone()
            .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok())
            .filter(|session| valid_session_id(session))
    }

    pub fn validate(&self) -> crate::errors::ApiResult<()> {
        if self
            .session_id
            .clone()
            .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok())
            .is_some_and(|session| !valid_session_id(&session))
        {
            return Err(crate::errors::ApiError::BadRequest(
                "Invalid hook session identifier".to_string(),
            ));
        }
        if self
            .tool
            .as_ref()
            .is_some_and(|tool| tool.len() > 256 || tool.chars().any(char::is_control))
        {
            return Err(crate::errors::ApiError::BadRequest(
                "Invalid hook tool name".to_string(),
            ));
        }
        Ok(())
    }
}

fn valid_session_id(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
    #[serde(default = "default_matcher")]
    pub matcher: String,
    pub hooks: Vec<HookEntry>,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_matcher() -> String {
    "*".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_claude_hook_event_fields() {
        let input: HookInput = serde_json::from_value(serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Read", "session_id": "fixture-session",
            "tool_response": {"result": "fixture"}, "cwd": "D:/fixture"
        }))
        .expect("hook payload");
        assert_eq!(input.hook_type.as_deref(), Some("PostToolUse"));
        assert_eq!(
            input.tool_output,
            Some(serde_json::json!({"result": "fixture"}))
        );
    }

    #[test]
    fn session_ids_cannot_escape_storage_directories() {
        for session in [
            "../../escape",
            "..\\escape",
            "C:\\escape",
            "\u{4e2d}\u{6587}",
            "",
            "a/b",
        ] {
            let input = HookInput {
                session_id: Some(session.to_string()),
                ..Default::default()
            };
            assert!(input.validate().is_err(), "{session}");
            assert!(input.resolved_session_id().is_none());
        }
        assert!(
            HookInput {
                session_id: Some("fixture-123_test".to_string()),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
}
