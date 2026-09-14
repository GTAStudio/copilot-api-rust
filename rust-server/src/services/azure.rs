use crate::errors::{ApiError, ApiResult};

#[derive(Debug, Clone)]
pub struct AzureConfig {
    pub endpoint: String,
    pub api_key: String,
    pub api_version: String,
    pub deployment: String,
}

pub fn load_azure_config(model: &str) -> Option<AzureConfig> {
    let endpoint = std::env::var("AZURE_OPENAI_ENDPOINT").ok()?;
    let api_key = crate::utils::required_api_key("AZURE_OPENAI_KEY").ok()?;
    let api_version =
        std::env::var("AZURE_OPENAI_API_VERSION").unwrap_or_else(|_| "2024-10-21".to_string());

    let deployment = model
        .strip_prefix("azure:")
        .filter(|deployment| !deployment.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("AZURE_OPENAI_DEPLOYMENT").ok())?;
    if endpoint.trim().is_empty() || api_key.trim().is_empty() || deployment.is_empty() {
        return None;
    }

    Some(AzureConfig {
        endpoint: endpoint.trim_end_matches('/').to_string(),
        api_key,
        api_version,
        deployment,
    })
}

fn request_url(config: &AzureConfig, operation: &str) -> ApiResult<url::Url> {
    if config.deployment.is_empty()
        || config.deployment.len() > 128
        || !config
            .deployment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApiError::BadRequest(
            "Invalid Azure deployment name".to_string(),
        ));
    }
    crate::utils::api_url(&config.endpoint, operation)?;
    let mut url = url::Url::parse(&config.endpoint)
        .map_err(|_| ApiError::BadRequest("Invalid Azure endpoint".to_string()))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| ApiError::BadRequest("Invalid Azure endpoint".to_string()))?;
        path.clear().push("openai");
        if operation == "responses" {
            path.push("v1").push("responses");
        } else {
            path.push("deployments")
                .push(&config.deployment)
                .extend(operation.split('/'));
        }
    }
    if operation != "responses" {
        url.query_pairs_mut()
            .append_pair("api-version", &config.api_version);
    }
    Ok(url)
}

pub async fn create_chat_completions(
    client: &reqwest::Client,
    config: &AzureConfig,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let url = request_url(config, "chat/completions")?;

    let resp = client
        .post(url)
        .header("api-key", &config.api_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Azure chat completions failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}

pub async fn create_embeddings(
    client: &reqwest::Client,
    config: &AzureConfig,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let url = request_url(config, "embeddings")?;

    let resp = client
        .post(url)
        .header("api-key", &config.api_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Azure embeddings failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}

pub async fn create_responses(
    client: &reqwest::Client,
    config: &AzureConfig,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let url = request_url(config, "responses")?;
    let mut payload = payload.clone();
    payload["model"] = config.deployment.clone().into();

    let resp = client
        .post(url)
        .header("api-key", &config.api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Azure responses failed: {e}")))?;

    crate::errors::check_upstream(resp).await
}

#[cfg(test)]
mod tests {
    use super::{AzureConfig, load_azure_config};
    use once_cell::sync::Lazy;
    use std::sync::Mutex;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_env(key: &str, value: &str) {
        unsafe {
            std::env::set_var(key, value);
        }
    }

    fn clear_env(key: &str) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn load_azure_config_uses_model_prefix_deployment() {
        let _lock = lock_env();
        set_env("AZURE_OPENAI_ENDPOINT", "https://example.openai.azure.com/");
        set_env("AZURE_OPENAI_KEY", "key");
        set_env("AZURE_OPENAI_API_VERSION", "2024-10-01-preview");
        set_env("AZURE_OPENAI_DEPLOYMENT", "ignored");

        let cfg = load_azure_config("azure:my-deployment").expect("config");
        assert_eq!(cfg.endpoint, "https://example.openai.azure.com");
        assert_eq!(cfg.deployment, "my-deployment");
        assert_eq!(cfg.api_version, "2024-10-01-preview");

        clear_env("AZURE_OPENAI_ENDPOINT");
        clear_env("AZURE_OPENAI_KEY");
        clear_env("AZURE_OPENAI_API_VERSION");
        clear_env("AZURE_OPENAI_DEPLOYMENT");
    }

    #[test]
    fn load_azure_config_falls_back_to_env_deployment() {
        let _lock = lock_env();
        set_env("AZURE_OPENAI_ENDPOINT", "https://example.openai.azure.com/");
        set_env("AZURE_OPENAI_KEY", "key");
        set_env("AZURE_OPENAI_DEPLOYMENT", "env-deployment");
        clear_env("AZURE_OPENAI_API_VERSION");

        let cfg = load_azure_config("azure").expect("config");
        assert_eq!(cfg.deployment, "env-deployment");
        assert_eq!(cfg.api_version, "2024-10-21");

        clear_env("AZURE_OPENAI_ENDPOINT");
        clear_env("AZURE_OPENAI_KEY");
        clear_env("AZURE_OPENAI_DEPLOYMENT");
    }

    #[test]
    fn azure_urls_use_stable_responses_and_encoded_deployment_paths() {
        let config = AzureConfig {
            endpoint: "https://fixture.openai.azure.com/".to_string(),
            api_key: "unit-test-key".to_string(),
            api_version: "2024-10-21".to_string(),
            deployment: "model-fixture".to_string(),
        };
        assert_eq!(
            super::request_url(&config, "responses")
                .expect("Responses URL")
                .as_str(),
            "https://fixture.openai.azure.com/openai/v1/responses"
        );
        assert_eq!(
            super::request_url(&config, "chat/completions")
                .expect("chat URL")
                .as_str(),
            "https://fixture.openai.azure.com/openai/deployments/model-fixture/chat/completions?api-version=2024-10-21"
        );
        let unsafe_config = AzureConfig {
            deployment: "../escape".to_string(),
            ..config
        };
        assert!(super::request_url(&unsafe_config, "chat/completions").is_err());
    }
}
