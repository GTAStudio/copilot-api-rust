use crate::config::{config_dir_path, AppConfig};
use serde_json::{json, Value};
use std::fs;
use std::io;
use std::path::Path;

pub fn ensure_azure_openai_config(config: &AppConfig) -> io::Result<String> {
    if !config.azure_enabled {
        return Ok("Azure OpenAI disabled".to_string());
    }

    let endpoint = config.azure_endpoint.trim();
    let deployment = config.azure_deployment.trim();
    let api_version = config.azure_api_version.trim();
    let api_key = config.azure_api_key.trim();

    if endpoint.is_empty() || deployment.is_empty() || api_version.is_empty() || api_key.is_empty() {
        return Ok("Azure OpenAI config incomplete".to_string());
    }

    let endpoint = endpoint.trim_end_matches('/');
    let base_url = format!("{}/openai/v1/", endpoint);
    let chat_completions_url = format!(
        "{}/openai/deployments/{}/chat/completions?api-version={}",
        endpoint, deployment, api_version
    );

    let payload = json!({
        "endpoint": endpoint,
        "deployment": deployment,
        "api_version": api_version,
        "api_key": api_key,
        "base_url": base_url,
        "chat_completions_url": chat_completions_url,
        "auth_header": "api-key"
    });

    let path = config_dir_path()?.join("azure-openai.json");
    write_json_atomic(&path, &payload)?;

    Ok("Azure OpenAI config updated".to_string())
}

fn write_json_atomic(path: &Path, value: &Value) -> io::Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    let data = serde_json::to_string_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(&tmp_path, data)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}
