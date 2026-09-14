use crate::config::{config_dir_path, AppConfig};
use serde_json::json;
use std::io;

pub fn ensure_azure_openai_config(config: &AppConfig) -> io::Result<String> {
    if config.effective_provider() != "azure" {
        return Ok("Azure OpenAI disabled".to_string());
    }
    config.validate()?;

    let endpoint = config.azure_endpoint.trim();
    let deployment = config.azure_deployment.trim();
    let api_version = config.azure_api_version.trim();
    let api_key = &config.azure_api_key;

    let endpoint = endpoint.trim_end_matches('/');
    let invalid_url = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid Azure endpoint");
    let mut base_url = url::Url::parse(endpoint).map_err(|_| invalid_url())?;
    let mut chat_completions_url = base_url.clone();
    base_url
        .path_segments_mut()
        .map_err(|_| invalid_url())?
        .pop_if_empty()
        .extend(["openai", "v1", ""]);
    chat_completions_url
        .path_segments_mut()
        .map_err(|_| invalid_url())?
        .pop_if_empty()
        .extend(["openai", "deployments", deployment, "chat", "completions"]);
    chat_completions_url
        .query_pairs_mut()
        .append_pair("api-version", api_version);

    let payload = json!({
        "endpoint": endpoint,
        "deployment": deployment,
        "api_version": api_version,
        "api_key": api_key,
        "base_url": base_url.as_str(),
        "chat_completions_url": chat_completions_url.as_str(),
        "auth_header": "api-key"
    });

    let path = config_dir_path()?.join("azure-openai.json");
    let data = serde_json::to_vec_pretty(&payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    crate::config::write_atomic(&path, &data)?;

    Ok("Azure OpenAI config updated".to_string())
}
