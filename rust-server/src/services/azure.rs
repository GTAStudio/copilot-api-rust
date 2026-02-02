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
    let api_key = std::env::var("AZURE_OPENAI_KEY").ok()?;
    let api_version = std::env::var("AZURE_OPENAI_API_VERSION").unwrap_or_else(|_| "2024-10-01-preview".to_string());

    let deployment = if let Some(dep) = model.strip_prefix("azure:") {
        dep.to_string()
    } else {
        std::env::var("AZURE_OPENAI_DEPLOYMENT").ok()?
    };

    Some(AzureConfig {
        endpoint: endpoint.trim_end_matches('/').to_string(),
        api_key,
        api_version,
        deployment,
    })
}

pub async fn create_chat_completions(
    client: &reqwest::Client,
    config: &AzureConfig,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let url = format!(
        "{}/openai/deployments/{}/chat/completions?api-version={}",
        config.endpoint, config.deployment, config.api_version
    );

    let resp = client
        .post(url)
        .header("api-key", &config.api_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Azure chat completions failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Azure chat completions failed: {text}")));
    }

    Ok(resp)
}

pub async fn create_embeddings(
    client: &reqwest::Client,
    config: &AzureConfig,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let url = format!(
        "{}/openai/deployments/{}/embeddings?api-version={}",
        config.endpoint, config.deployment, config.api_version
    );

    let resp = client
        .post(url)
        .header("api-key", &config.api_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Azure embeddings failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Azure embeddings failed: {text}")));
    }

    Ok(resp)
}

pub async fn create_responses(
    client: &reqwest::Client,
    config: &AzureConfig,
    payload: &serde_json::Value,
) -> ApiResult<reqwest::Response> {
    let url = format!(
        "{}/openai/deployments/{}/responses?api-version={}",
        config.endpoint, config.deployment, config.api_version
    );

    let resp = client
        .post(url)
        .header("api-key", &config.api_key)
        .json(payload)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(format!("Azure responses failed: {e}")))?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Upstream(format!("Azure responses failed: {text}")));
    }

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::load_azure_config;
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
        assert_eq!(cfg.api_version, "2024-10-01-preview");

        clear_env("AZURE_OPENAI_ENDPOINT");
        clear_env("AZURE_OPENAI_KEY");
        clear_env("AZURE_OPENAI_DEPLOYMENT");
    }
}
