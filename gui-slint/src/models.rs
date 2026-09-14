//! Model fetching and caching logic
//! Fetches available models from the copilot-api server and caches them locally

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Response from /v1/models endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<Model>,
    #[serde(default)]
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

/// Fetch models from the running copilot-api server
/// Returns None if server is not reachable
pub fn fetch_models_from_server(port: u16) -> Option<Vec<String>> {
    let url = format!("http://127.0.0.1:{}/v1/models", port);
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .max_redirects(0)
        .proxy(None)
        .build();
    let client: ureq::Agent = config.into();
    let mut request = client.get(&url);
    if let Ok(key) = std::env::var("COPILOT_API_KEY") {
        request = request.header("x-api-key", key.trim());
    }
    let mut response = match request.call() {
        Ok(response) => response,
        Err(_) => {
            // Server not running or unreachable - this is expected at startup
            return None;
        }
    };

    match response.body_mut().read_json::<ModelsResponse>() {
        Ok(models_response) => {
            let mut model_ids = Vec::new();
            for model in models_response.data {
                if !model.id.trim().is_empty()
                    && model.id.len() <= 256
                    && !model.id.chars().any(char::is_control)
                    && !model_ids.contains(&model.id)
                {
                    model_ids.push(model.id);
                }
            }

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
    cached.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_accepts_anthropic_catalogues_and_rejects_invalid_ids() {
        use std::io::{Read, Write};
        for (body, expected) in [
            (
                r#"{"data":[{"id":"claude-sonnet-4-6"}]}"#,
                Some(vec!["claude-sonnet-4-6".to_string()]),
            ),
            (
                r#"{"object":"list","data":[{"id":""},{"id":"claude-sonnet-4-6"},{"id":"claude-sonnet-4-6"},{"id":"bad\nid"}]}"#,
                Some(vec!["claude-sonnet-4-6".to_string()]),
            ),
            (r#"{"data":[]}"#, None),
            ("not-json", None),
        ] {
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("model discovery listener");
            let port = listener.local_addr().expect("model port").port();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("model client");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("read deadline");
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).expect("request headers");
                    request.push(byte[0]);
                }
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream
                    .write_all(response.as_bytes())
                    .expect("discovery response");
            });
            let models = fetch_models_from_server(port);
            worker.join().expect("discovery server completed");
            assert_eq!(models, expected);
        }
    }

    #[test]
    fn empty_cache_does_not_advertise_unverified_models() {
        assert!(get_cached_or_fallback(&[]).is_empty());
        assert_eq!(
            get_cached_or_fallback(&["claude-sonnet-4-6".to_string()]),
            vec!["claude-sonnet-4-6"]
        );
    }
}
