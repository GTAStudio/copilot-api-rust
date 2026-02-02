use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub api_base_url: String,
    pub api_key: String,
    pub autostart: bool,
    pub claude_base_url: String,
    pub use_proxy: bool,
    pub proxy_url: String,
    pub proxy_scheme: String,
    pub proxy_username: String,
    pub proxy_password: String,
    pub server_port: u16,
    pub account_type: String,
    pub verbose: bool,
    pub manual: bool,
    pub wait: bool,
    pub rate_limit_seconds: u64,
    pub github_token: String,
    pub azure_enabled: bool,
    pub azure_endpoint: String,
    pub azure_deployment: String,
    pub azure_api_version: String,
    pub azure_api_key: String,
    pub show_copilot_section: bool,
    pub show_azure_section: bool,
    // Model selection
    pub main_model: String,
    pub fast_model: String,
    // Cached models from server
    #[serde(default)]
    pub cached_models: Vec<String>,
    #[serde(default)]
    pub hooks_enabled: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_base_url: String::new(),
            api_key: String::new(),
            autostart: false,
            claude_base_url: "http://localhost:4141".to_string(),
            use_proxy: false,
            proxy_url: String::new(),
            proxy_scheme: "http".to_string(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            server_port: 4141,
            account_type: "enterprise".to_string(),
            verbose: false,
            manual: false,
            wait: false,
            rate_limit_seconds: 0,
            github_token: String::new(),
            azure_enabled: false,
            azure_endpoint: String::new(),
            azure_deployment: String::new(),
            azure_api_version: "2024-10-21".to_string(),
            azure_api_key: String::new(),
            show_copilot_section: true,
            show_azure_section: false,
            main_model: "claude-sonnet-4".to_string(),
            fast_model: "gpt-5-mini".to_string(),
            cached_models: Vec::new(),
            hooks_enabled: true,
        }
    }
}

impl AppConfig {
    /// Returns the Claude base URL for clients to connect to.
    /// This is the copilot-api server address, NOT the proxy.
    /// Proxy is configured separately via environment variables.
    pub fn effective_claude_base_url(&self) -> String {
        let base = self.claude_base_url.trim();
        if base.is_empty() {
            format!("http://localhost:{}", self.server_port)
        } else {
            base.to_string()
        }
    }

    pub fn normalized_account_type(&self) -> String {
        let value = self.account_type.trim().to_lowercase();
        match value.as_str() {
            "enterprise" | "business" | "individual" => value,
            _ => "enterprise".to_string(),
        }
    }

    pub fn proxy_url_with_auth(&self) -> String {
        let raw = self.proxy_url.trim();
        if raw.is_empty() {
            return String::new();
        }

        let scheme = self.proxy_scheme.trim();
        let mut base = if raw.contains("://") {
            raw.to_string()
        } else if !scheme.is_empty() {
            format!("{}://{}", scheme, raw)
        } else {
            raw.to_string()
        };

        if base.contains('@') {
            return base;
        }

        let user = self.proxy_username.trim();
        let pass = self.proxy_password.trim();
        if user.is_empty() {
            return base;
        }

        if let Some((left, right)) = base.split_once("://") {
            let auth = if pass.is_empty() {
                user.to_string()
            } else {
                format!("{}:{}", user, pass)
            };
            base = format!("{}://{}@{}", left, auth, right);
        }

        base
    }
}

pub fn config_dir_path() -> io::Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("com", "gtastudio", "githubcopilot-api-gui")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No valid config dir"))?;
    let config_dir = proj_dirs.config_dir();
    fs::create_dir_all(config_dir)?;
    Ok(config_dir.to_path_buf())
}

pub fn config_file_path() -> io::Result<PathBuf> {
    Ok(config_dir_path()?.join("config.json"))
}

pub fn load_config() -> io::Result<AppConfig> {
    let path = config_file_path()?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let data = fs::read_to_string(path)?;
    let config = serde_json::from_str::<AppConfig>(&data)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(config)
}

pub fn save_config(config: &AppConfig) -> io::Result<()> {
    let path = config_file_path()?;
    let data = serde_json::to_string_pretty(config)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    write_atomic(&path, data.as_bytes())
}

fn write_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    fs::rename(tmp_path, path)?;
    Ok(())
}
