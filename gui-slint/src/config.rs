use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub is_chinese: bool,
    pub provider: String,
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
            is_chinese: true,
            provider: "auto".to_string(),
            api_base_url: String::new(),
            api_key: String::new(),
            autostart: false,
            claude_base_url: String::new(),
            use_proxy: false,
            proxy_url: String::new(),
            proxy_scheme: "http".to_string(),
            proxy_username: String::new(),
            proxy_password: String::new(),
            server_port: 4141,
            account_type: "individual".to_string(),
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
            main_model: "claude-sonnet-4-6".to_string(),
            fast_model: "claude-haiku-4-5".to_string(),
            cached_models: Vec::new(),
            hooks_enabled: true,
        }
    }
}

impl AppConfig {
    pub fn effective_provider(&self) -> &str {
        if self.provider != "auto" {
            return &self.provider;
        }
        if self.azure_enabled {
            return "azure";
        }
        if self.api_base_url.is_empty() && self.api_key.is_empty() {
            return "copilot";
        }
        if url::Url::parse(&self.api_base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .as_deref()
            == Some("api.anthropic.com")
        {
            "anthropic"
        } else {
            "openai"
        }
    }

    /// Returns the Claude base URL for clients to connect to.
    /// This is the copilot-api server address, NOT the proxy.
    /// Proxy is configured separately via environment variables.
    pub fn effective_claude_base_url(&self) -> String {
        let base = self.claude_base_url.trim();
        if base.is_empty() {
            format!("http://127.0.0.1:{}", self.server_port)
        } else {
            base.to_string()
        }
    }

    pub fn normalized_account_type(&self) -> String {
        let value = self.account_type.trim().to_lowercase();
        match value.as_str() {
            "enterprise" | "business" | "individual" => value,
            _ => "individual".to_string(),
        }
    }

    pub fn proxy_url_with_auth(&self) -> String {
        let raw = self.proxy_url.trim();
        if raw.is_empty() {
            return String::new();
        }
        let base = if raw.contains("://") {
            raw.to_string()
        } else {
            format!("{}://{}", self.proxy_scheme.trim(), raw)
        };
        let Ok(mut url) = url::Url::parse(&base) else {
            return String::new();
        };
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
            || url.host_str().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return String::new();
        }
        if url.username().is_empty()
            && !self.proxy_username.is_empty()
            && (url.set_username(&self.proxy_username).is_err()
                || url.set_password(Some(&self.proxy_password)).is_err())
        {
            return String::new();
        }
        url.to_string()
    }

    pub fn validate(&self) -> io::Result<()> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
        if !["auto", "copilot", "anthropic", "openai", "azure"].contains(&self.provider.as_str()) {
            return Err(invalid("Invalid provider selection"));
        }
        if self.server_port == 0 {
            return Err(invalid("Port must be between 1 and 65535"));
        }
        if !["individual", "business", "enterprise"]
            .contains(&self.account_type.trim().to_lowercase().as_str())
        {
            return Err(invalid("Invalid account type"));
        }
        if self.rate_limit_seconds > 86_400 {
            return Err(invalid("Rate limit must not exceed 86400 seconds"));
        }
        if self.use_proxy && self.proxy_url_with_auth().is_empty() {
            return Err(invalid("Invalid proxy URL"));
        }
        if self.manual {
            return Err(invalid(
                "Manual approval requires the interactive server CLI",
            ));
        }
        let valid_key = |key: &str| {
            !key.is_empty() && key.len() <= 8192 && key.bytes().all(|byte| byte.is_ascii_graphic())
        };
        if self.effective_provider() == "azure" {
            validate_upstream_base(&self.azure_endpoint)?;
            if !valid_key(&self.azure_api_key)
                || self.azure_deployment.is_empty()
                || self.azure_deployment.len() > 128
                || !self
                    .azure_deployment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || self.azure_api_version.trim().is_empty()
            {
                return Err(invalid("Invalid or incomplete Azure configuration"));
            }
        } else if matches!(self.effective_provider(), "anthropic" | "openai") {
            validate_upstream_base(&self.api_base_url)?;
            if !valid_key(&self.api_key) {
                return Err(invalid("Invalid upstream API key"));
            }
        }
        if !self.github_token.is_empty() && !valid_key(&self.github_token) {
            return Err(invalid("Invalid GitHub token"));
        }
        Ok(())
    }
}

fn validate_upstream_base(base: &str) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid upstream base URL");
    let url = url::Url::parse(base).map_err(|_| invalid())?;
    let local = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.host_str().is_none()
        || (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(())
}

pub fn config_dir_path() -> io::Result<PathBuf> {
    let directory = match std::env::var_os("COPILOT_GUI_CONFIG_DIR") {
        Some(value) => {
            let directory = PathBuf::from(value);
            if !directory.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "COPILOT_GUI_CONFIG_DIR must be an absolute directory path",
                ));
            }
            directory
        }
        None => directories::ProjectDirs::from("com", "gtastudio", "githubcopilot-api-gui")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No valid config dir"))?
            .config_dir()
            .to_path_buf(),
    };
    fs::create_dir_all(&directory)?;
    Ok(directory)
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
    config.validate()?;
    let path = config_file_path()?;
    let data = serde_json::to_string_pretty(config)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    write_atomic(&path, data.as_bytes())
}

pub fn save_language_preference(is_chinese: bool) -> io::Result<()> {
    save_language_preference_at(&config_file_path()?, is_chinese)
}

fn save_language_preference_at(path: &Path, is_chinese: bool) -> io::Result<()> {
    let mut document: serde_json::Value = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error),
    };
    let object = document.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "GUI configuration must be a JSON object",
        )
    })?;
    object.insert("is_chinese".to_string(), is_chinese.into());
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_atomic(path, &bytes)
}

pub(crate) fn write_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing parent directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(content)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_preference_is_backward_compatible() {
        let legacy: AppConfig = serde_json::from_str("{}").expect("legacy config");
        assert_eq!(
            serde_json::to_value(legacy).expect("config JSON")["is_chinese"],
            true
        );
        let english: AppConfig =
            serde_json::from_str(r#"{"is_chinese":false}"#).expect("English config");
        assert_eq!(
            serde_json::to_value(english).expect("config JSON")["is_chinese"],
            false
        );
        assert!(serde_json::from_str::<AppConfig>(r#"{"is_chinese":"false"}"#).is_err());
    }

    #[test]
    fn language_preference_save_preserves_other_fields() {
        let directory = tempfile::tempdir().expect("isolated language config");
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"server_port":5050,"api_key":"fixture-key","future":{"keep":true}}"#,
        )
        .expect("existing config");
        save_language_preference_at(&path, false).expect("save English preference");
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("saved config"))
                .expect("saved JSON");
        assert_eq!(
            document,
            serde_json::json!({"is_chinese":false,"server_port":5050,"api_key":"fixture-key","future":{"keep":true}})
        );
        for invalid in ["{broken", "[]", "null"] {
            std::fs::write(&path, invalid).expect("invalid config fixture");
            assert!(save_language_preference_at(&path, true).is_err());
            assert_eq!(
                std::fs::read_to_string(&path).expect("preserved config"),
                invalid
            );
        }
        let fresh = directory.path().join("fresh.json");
        save_language_preference_at(&fresh, false).expect("new language preference");
        let config: AppConfig = serde_json::from_slice(&std::fs::read(fresh).expect("new config"))
            .expect("default settings with language");
        assert_eq!(
            serde_json::to_value(config).expect("config JSON")["is_chinese"],
            false
        );
    }

    #[test]
    fn gui_audit_proxy_credentials_are_url_encoded() {
        let config = AppConfig {
            proxy_url: "127.0.0.1:2080".to_string(),
            proxy_username: "user@example".to_string(),
            proxy_password: "pass:word/@#".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.proxy_url_with_auth(),
            "http://user%40example:pass%3Aword%2F%40%23@127.0.0.1:2080/"
        );
    }

    #[test]
    fn gui_audit_default_client_url_tracks_the_port() {
        let config = AppConfig {
            server_port: 5050,
            ..Default::default()
        };
        assert_eq!(config.effective_claude_base_url(), "http://127.0.0.1:5050");
    }
}
