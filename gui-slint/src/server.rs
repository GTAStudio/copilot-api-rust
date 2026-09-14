use crate::config::AppConfig;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(embedded_server)]
static EMBEDDED_SERVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/server_embedded.gz"));

pub fn start_server(config: &AppConfig) -> Result<Child, String> {
    config.validate().map_err(|error| error.to_string())?;
    let server_exe = get_server_exe()?;
    server_command(config, &server_exe)?
        .spawn()
        .map_err(|error| format!("Failed to start server: {error}"))
}

fn server_command(config: &AppConfig, server_exe: &std::path::Path) -> Result<Command, String> {
    config.validate().map_err(|error| error.to_string())?;
    let mut cmd = Command::new(server_exe);
    for name in [
        "COPILOT_GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "AZURE_OPENAI_KEY",
        "AZURE_OPENAI_ENDPOINT",
        "AZURE_OPENAI_DEPLOYMENT",
        "AZURE_OPENAI_API_VERSION",
    ] {
        cmd.env_remove(name);
    }

    cmd.arg("start")
        .arg("--port")
        .arg(config.server_port.to_string())
        .arg("--account-type")
        .arg(config.normalized_account_type())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Hide console window on Windows
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    if config.verbose {
        cmd.arg("--verbose");
    }
    if config.manual {
        cmd.arg("--manual");
    }
    if config.wait {
        cmd.arg("--wait");
    }
    if config.rate_limit_seconds > 0 {
        cmd.arg("--rate-limit")
            .arg(config.rate_limit_seconds.to_string());
    }
    if config.effective_provider() == "copilot" && !config.github_token.trim().is_empty() {
        cmd.env("COPILOT_GITHUB_TOKEN", config.github_token.trim());
    }

    configure_proxy(&mut cmd, config);

    cmd.env(
        "COPILOT_HOOKS_ENABLED",
        if config.hooks_enabled { "1" } else { "0" },
    );

    cmd.env("COPILOT_PROVIDER", config.effective_provider());
    match config.effective_provider() {
        "azure" => {
            cmd.env("AZURE_OPENAI_ENDPOINT", config.azure_endpoint.trim())
                .env("AZURE_OPENAI_DEPLOYMENT", config.azure_deployment.trim())
                .env("AZURE_OPENAI_API_VERSION", config.azure_api_version.trim())
                .env("AZURE_OPENAI_KEY", &config.azure_api_key);
        }
        "anthropic" => {
            cmd.env("ANTHROPIC_BASE_URL", config.api_base_url.trim())
                .env("ANTHROPIC_API_KEY", &config.api_key);
        }
        "openai" => {
            cmd.env("OPENAI_BASE_URL", config.api_base_url.trim())
                .env("OPENAI_API_KEY", &config.api_key);
        }
        _ => {}
    }

    Ok(cmd)
}

pub fn configure_proxy(command: &mut Command, config: &AppConfig) {
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        if config.use_proxy {
            command.env(name, config.proxy_url_with_auth());
        } else {
            command.env_remove(name);
        }
    }
    command.env("NO_PROXY", "localhost,127.0.0.1,::1");
    command.env(
        "COPILOT_DISABLE_PROXY",
        if config.use_proxy { "0" } else { "1" },
    );
}

/// Public version for auth command
pub fn get_server_exe_path() -> Result<PathBuf, String> {
    get_server_exe()
}

#[cfg(any(embedded_server, test))]
pub(crate) fn cache_directory() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("COPILOT_GUI_CACHE_DIR") {
        let directory = PathBuf::from(value);
        if !directory.is_absolute() {
            return Err("COPILOT_GUI_CACHE_DIR must be an absolute directory path".to_string());
        }
        return Ok(directory);
    }
    directories::ProjectDirs::from("com", "gtastudio", "githubcopilot-api-gui")
        .map(|directories| directories.cache_dir().to_path_buf())
        .ok_or_else(|| "No application cache directory".to_string())
}

fn get_server_exe() -> Result<PathBuf, String> {
    #[cfg(embedded_server)]
    {
        use std::io::Read;
        let mut data = Vec::new();
        flate2::read::GzDecoder::new(EMBEDDED_SERVER)
            .take(256 * 1024 * 1024)
            .read_to_end(&mut data)
            .map_err(|error| format!("Cannot decompress server: {error}"))?;
        ensure_cached_server(&cache_directory()?, &data)
    }

    #[cfg(not(embedded_server))]
    {
        let directory = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .ok_or_else(|| "No executable directory".to_string())?
            .to_path_buf();
        let path = directory.join(if cfg!(windows) {
            "copilot-api-server.exe"
        } else {
            "copilot-api-server"
        });
        if path.is_file() {
            Ok(path)
        } else {
            Err("Server not embedded and no adjacent server found".to_string())
        }
    }
}

#[cfg(any(embedded_server, test))]
fn ensure_cached_server(root: &std::path::Path, payload: &[u8]) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let digest: String = Sha256::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let directory = root.join("server").join(digest);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(if cfg!(windows) {
        "copilot-api-server.exe"
    } else {
        "copilot-api-server"
    });
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("Refusing a symlinked server cache".to_string());
    }
    if std::fs::read(&path).is_ok_and(|existing| existing == payload) {
        return Ok(path);
    }
    let mut file =
        tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
    file.write_all(payload).map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    file.persist(&path)
        .map_err(|error| error.error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_command_never_places_credentials_in_arguments() {
        let config = AppConfig {
            github_token: "unit-test-github-token".to_string(),
            verbose: true,
            wait: true,
            rate_limit_seconds: 2,
            use_proxy: true,
            proxy_url: "127.0.0.1:2080".to_string(),
            ..Default::default()
        };
        let command =
            server_command(&config, std::path::Path::new("fixture-server.exe")).expect("command");
        let arguments: Vec<_> = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert!(arguments.contains(&"--verbose".to_string()));
        assert!(arguments.contains(&"--wait".to_string()));
        assert!(!arguments
            .iter()
            .any(|value| value.contains("unit-test-github-token")));
        let environment: std::collections::BTreeMap<_, _> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            environment["COPILOT_GITHUB_TOKEN"].as_deref(),
            Some("unit-test-github-token")
        );
        assert_eq!(environment["COPILOT_PROVIDER"].as_deref(), Some("copilot"));
        assert_eq!(
            environment["HTTP_PROXY"].as_deref(),
            Some("http://127.0.0.1:2080/")
        );
        assert_eq!(environment["ANTHROPIC_API_KEY"], None);
        assert_eq!(environment["OPENAI_API_KEY"], None);
    }

    #[test]
    fn server_command_validates_provider_settings_and_clears_stale_credentials() {
        for (base, provider) in [
            ("https://api.anthropic.com", "anthropic"),
            ("https://api.openai.com", "openai"),
        ] {
            let config = AppConfig {
                api_base_url: base.to_string(),
                api_key: "unit-test-provider-key".to_string(),
                hooks_enabled: false,
                ..Default::default()
            };
            let command = server_command(&config, std::path::Path::new("fixture-server.exe"))
                .expect("provider command");
            let environment: std::collections::BTreeMap<_, _> = command
                .get_envs()
                .map(|(name, value)| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.map(|value| value.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            assert_eq!(environment["COPILOT_PROVIDER"].as_deref(), Some(provider));
            assert_eq!(environment["COPILOT_HOOKS_ENABLED"].as_deref(), Some("0"));
            assert_eq!(environment["COPILOT_GITHUB_TOKEN"], None);
            assert_eq!(environment["COPILOT_DISABLE_PROXY"].as_deref(), Some("1"));
        }
        for config in [
            AppConfig {
                api_key: "unit-test-provider-key".to_string(),
                ..Default::default()
            },
            AppConfig {
                azure_enabled: true,
                ..Default::default()
            },
            AppConfig {
                api_base_url: "http://untrusted.example".to_string(),
                api_key: "unit-test-provider-key".to_string(),
                ..Default::default()
            },
            AppConfig {
                server_port: 0,
                ..Default::default()
            },
        ] {
            assert!(server_command(&config, std::path::Path::new("fixture-server.exe")).is_err());
        }
    }

    #[test]
    fn explicit_provider_selection_is_independent_of_gateway_host() {
        for (provider, base, credential) in [
            (
                "anthropic",
                "https://gateway.example.com",
                "ANTHROPIC_API_KEY",
            ),
            ("openai", "https://anthropic.example.com", "OPENAI_API_KEY"),
        ] {
            let config: AppConfig = serde_json::from_value(serde_json::json!({
                "provider": provider, "api_base_url": base, "api_key": "unit-test-explicit-key"
            }))
            .expect("saved provider config");
            let command = server_command(&config, std::path::Path::new("fixture-server.exe"))
                .expect("explicit provider");
            let environment: std::collections::BTreeMap<_, _> = command
                .get_envs()
                .map(|(name, value)| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.map(|value| value.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            assert_eq!(environment["COPILOT_PROVIDER"].as_deref(), Some(provider));
            assert_eq!(
                environment[credential].as_deref(),
                Some("unit-test-explicit-key")
            );
        }
        let invalid: AppConfig =
            serde_json::from_value(serde_json::json!({"provider": "invalid-provider"}))
                .expect("config shape");
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn gui_audit_cached_server_checks_content_and_versions() {
        let directory = tempfile::tempdir().expect("test directory");
        let original = b"unit-test-server-one";
        let path = ensure_cached_server(directory.path(), original).expect("extract");
        std::fs::write(&path, b"unit-test-server-bad").expect("corrupt cache");
        let repaired = ensure_cached_server(directory.path(), original).expect("repair");
        assert_eq!(std::fs::read(&repaired).expect("cache"), original);
        let updated =
            ensure_cached_server(directory.path(), b"unit-test-server-two").expect("new version");
        assert_ne!(path, updated);
        assert_eq!(
            std::fs::read(&path).expect("old version preserved"),
            original
        );
    }
}
