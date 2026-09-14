use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use uuid::Uuid;

use crate::state::AppConfig;

pub const COPILOT_VERSION: &str = "0.26.7";
pub const API_VERSION: &str = "2025-04-01";
pub const GITHUB_API_VERSION: &str = "2026-03-10";

fn copilot_version() -> String {
    std::env::var("COPILOT_EXTENSION_VERSION")
        .ok()
        .filter(|version| crate::utils::valid_version(version))
        .unwrap_or_else(|| COPILOT_VERSION.to_string())
}

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
    if let Some(base) = &config.copilot_base_url {
        return base.trim_end_matches('/').to_string();
    }
    if config.account_type == "individual" {
        "https://api.githubcopilot.com".to_string()
    } else {
        format!("https://api.{}.githubcopilot.com", config.account_type)
    }
}

pub fn resolve_model_id(config: &AppConfig, requested: &str) -> String {
    let Some(models) = &config.models else {
        return requested.to_string();
    };
    if models.data.iter().any(|model| model.id == requested) {
        return requested.to_string();
    }
    fn normalized(model: &str) -> String {
        let model = match model.rsplit_once('-') {
            Some((base, date))
                if date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                base
            }
            _ => model,
        };
        model.replace('.', "-")
    }
    let requested_key = normalized(requested);
    let mut matches = models
        .data
        .iter()
        .filter(|model| normalized(&model.id) == requested_key);
    match (matches.next(), matches.next()) {
        (Some(model), None) => model.id.clone(),
        _ => requested.to_string(),
    }
}

pub fn supports_endpoint(config: &AppConfig, model: &str, endpoint: &str) -> bool {
    config
        .models
        .as_ref()
        .and_then(|models| models.data.iter().find(|entry| entry.id == model))
        .is_some_and(|model| {
            model
                .supported_endpoints
                .iter()
                .any(|value| value == endpoint)
        })
}

pub fn copilot_headers(config: &AppConfig, token: &str, vision: bool) -> Vec<(String, String)> {
    let version = copilot_version();
    let editor_plugin_version = format!("copilot-chat/{version}");
    let user_agent = format!("GitHubCopilotChat/{version}");

    let mut headers = vec![
        ("authorization".to_string(), format!("Bearer {}", token)),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "copilot-integration-id".to_string(),
            "vscode-chat".to_string(),
        ),
        (
            "editor-version".to_string(),
            format!("vscode/{}", config.vscode_version),
        ),
        ("editor-plugin-version".to_string(), editor_plugin_version),
        ("user-agent".to_string(), user_agent),
        (
            "openai-intent".to_string(),
            "conversation-panel".to_string(),
        ),
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
    let version = copilot_version();
    let editor_plugin_version = format!("copilot-chat/{version}");
    let user_agent = format!("GitHubCopilotChat/{version}");

    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("Bearer {}", token)),
        (
            "editor-version".to_string(),
            format!("vscode/{}", config.vscode_version),
        ),
        ("editor-plugin-version".to_string(), editor_plugin_version),
        ("user-agent".to_string(), user_agent),
        (
            "x-github-api-version".to_string(),
            GITHUB_API_VERSION.to_string(),
        ),
        (
            "x-vscode-user-agent-library-version".to_string(),
            "electron-fetch".to_string(),
        ),
    ]
}

pub fn apply_headers(map: &mut HeaderMap, headers: Vec<(String, String)>) {
    for (k, v) in headers {
        if let Ok(name) = HeaderName::from_bytes(k.as_bytes())
            && let Ok(value) = HeaderValue::from_str(&v)
        {
            map.insert(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_rest_and_copilot_have_separate_version_headers() {
        let config = AppConfig::default();
        let github = github_headers(&config, "unit-test-token");
        assert!(
            github
                .iter()
                .any(|(name, value)| name == "x-github-api-version" && value == "2026-03-10")
        );
        let copilot = copilot_headers(&config, "unit-test-token", false);
        assert!(
            copilot
                .iter()
                .any(|(name, value)| name == "x-github-api-version" && value == "2025-04-01")
        );
    }
}
