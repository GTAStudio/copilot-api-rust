//! Model fetching and caching logic
//! Fetches available models from the copilot-api server and caches them locally

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Response from /v1/models endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<Model>,
    pub object: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub owned_by: String,
    #[serde(default)]
    pub display_name: String,
}

/// Fallback models when server is not available
pub fn fallback_models() -> Vec<String> {
    vec![
        "claude-sonnet-4".to_string(),
        "claude-opus-4.5".to_string(),
        "gpt-5.2-codex".to_string(),
        "gpt-5.1-codex".to_string(),
        "gpt-5-mini".to_string(),
        "gpt-5".to_string(),
        "gpt-4o".to_string(),
        "gemini-2.5-pro".to_string(),
    ]
}

/// Fetch models from the running copilot-api server
/// Returns None if server is not reachable
pub fn fetch_models_from_server(port: u16) -> Option<Vec<String>> {
    let url = format!("http://localhost:{}/v1/models", port);
    
    let client = match ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .get(&url)
        .call()
    {
        Ok(response) => response,
        Err(_) => {
            // Server not running or unreachable - this is expected at startup
            return None;
        }
    };
    
    match client.into_json::<ModelsResponse>() {
        Ok(models_response) => {
            let model_ids: Vec<String> = models_response
                .data
                .into_iter()
                .map(|m| m.id)
                .collect();
            
            if model_ids.is_empty() {
                None
            } else {
                Some(model_ids)
            }
        }
        Err(_) => {
            // Parse error - server returned unexpected format
            None
        }
    }
}

/// Get models from cache or fallback (for startup, when server is not running)
pub fn get_cached_or_fallback(cached: &[String]) -> Vec<String> {
    if !cached.is_empty() {
        cached.to_vec()
    } else {
        fallback_models()
    }
}
