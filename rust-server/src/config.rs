use uuid::Uuid;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::state::AppConfig;

pub const COPILOT_VERSION: &str = "0.26.7";
pub const API_VERSION: &str = "2025-04-01";

pub const GITHUB_API_BASE_URL: &str = "https://api.github.com";
pub const GITHUB_BASE_URL: &str = "https://github.com";
pub const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
pub const GITHUB_APP_SCOPES: &str = "read:user";

pub fn standard_headers() -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
    ]
}

pub fn copilot_base_url(config: &AppConfig) -> String {
    if config.account_type == "individual" {
        "https://api.githubcopilot.com".to_string()
    } else {
        format!("https://api.{}.githubcopilot.com", config.account_type)
    }
}

pub fn copilot_headers(config: &AppConfig, token: &str, vision: bool) -> Vec<(String, String)> {
    let editor_plugin_version = format!("copilot-chat/{}", COPILOT_VERSION);
    let user_agent = format!("GitHubCopilotChat/{}", COPILOT_VERSION);

    let mut headers = vec![
        ("authorization".to_string(), format!("Bearer {}", token)),
        ("content-type".to_string(), "application/json".to_string()),
        ("copilot-integration-id".to_string(), "vscode-chat".to_string()),
        ("editor-version".to_string(), format!("vscode/{}", config.vscode_version)),
        ("editor-plugin-version".to_string(), editor_plugin_version),
        ("user-agent".to_string(), user_agent),
        ("openai-intent".to_string(), "conversation-panel".to_string()),
        ("x-github-api-version".to_string(), API_VERSION.to_string()),
        ("x-request-id".to_string(), Uuid::new_v4().to_string()),
        (
            "x-vscode-user-agent-library-version".to_string(),
            "electron-fetch".to_string(),
        ),
    ];

    if vision {
        headers.push(("copilot-vision-request".to_string(), "true".to_string()));
    }

    headers
}

pub fn github_headers(config: &AppConfig, token: &str) -> Vec<(String, String)> {
    let editor_plugin_version = format!("copilot-chat/{}", COPILOT_VERSION);
    let user_agent = format!("GitHubCopilotChat/{}", COPILOT_VERSION);

    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("token {}", token)),
        ("editor-version".to_string(), format!("vscode/{}", config.vscode_version)),
        ("editor-plugin-version".to_string(), editor_plugin_version),
        ("user-agent".to_string(), user_agent),
        ("x-github-api-version".to_string(), API_VERSION.to_string()),
        (
            "x-vscode-user-agent-library-version".to_string(),
            "electron-fetch".to_string(),
        ),
    ]
}

pub fn apply_headers(map: &mut HeaderMap, headers: Vec<(String, String)>) {
    for (k, v) in headers {
        if let Ok(name) = HeaderName::from_bytes(k.as_bytes()) {
            if let Ok(value) = HeaderValue::from_str(&v) {
                map.insert(name, value);
            }
        }
    }
}
