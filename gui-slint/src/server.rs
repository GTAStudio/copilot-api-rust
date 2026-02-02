use crate::config::AppConfig;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::io::{Read, Write};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(embedded_server)]
static EMBEDDED_SERVER: &[u8] = include_bytes!("server_embedded.gz");

pub fn start_server(config: &AppConfig) -> Result<Child, String> {
    let server_exe = get_server_exe()?;
    
    let mut cmd = Command::new(&server_exe);
    
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
    if !config.github_token.trim().is_empty() {
        cmd.arg("--github-token").arg(config.github_token.trim());
    }

    if config.use_proxy {
        let proxy = config.proxy_url_with_auth();
        if !proxy.trim().is_empty() {
            cmd.env("HTTP_PROXY", &proxy)
                .env("HTTPS_PROXY", &proxy)
                .env("ALL_PROXY", &proxy)
                .env("NO_PROXY", "localhost,127.0.0.1");
        }
    }

    if !config.hooks_enabled {
        cmd.env("COPILOT_HOOKS_ENABLED", "0");
    }

    // Provider selection + credentials
    if config.azure_enabled {
        cmd.env("COPILOT_PROVIDER", "azure")
            .env("AZURE_OPENAI_ENDPOINT", config.azure_endpoint.trim())
            .env("AZURE_OPENAI_DEPLOYMENT", config.azure_deployment.trim())
            .env("AZURE_OPENAI_API_VERSION", config.azure_api_version.trim())
            .env("AZURE_OPENAI_KEY", config.azure_api_key.trim());
    } else if !config.api_key.trim().is_empty() {
        let base = config.api_base_url.trim();
        if !base.is_empty() {
            if base.contains("anthropic") {
                cmd.env("COPILOT_PROVIDER", "anthropic")
                    .env("ANTHROPIC_BASE_URL", base)
                    .env("ANTHROPIC_API_KEY", config.api_key.trim());
            } else {
                cmd.env("COPILOT_PROVIDER", "openai")
                    .env("OPENAI_BASE_URL", base)
                    .env("OPENAI_API_KEY", config.api_key.trim());
            }
        }
    } else {
        cmd.env("COPILOT_PROVIDER", "copilot");
    }

    cmd.spawn().map_err(|err| format!("Failed to start server: {err}"))
}

/// Public version for auth command
pub fn get_server_exe_path() -> Result<PathBuf, String> {
    get_server_exe()
}

fn get_server_exe() -> Result<PathBuf, String> {
    #[cfg(embedded_server)]
    {
        // Extract embedded server to temp directory
        let temp_dir = std::env::temp_dir().join("copilot-api-gui");
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| format!("Cannot create temp dir: {e}"))?;
        
        let server_path = temp_dir.join("copilot-api-server.exe");
        
        // Check if already extracted and has correct size
        let need_extract = if server_path.exists() {
            // Re-extract if file seems corrupted
            std::fs::metadata(&server_path)
                .map(|m| m.len() < 1000000) // Less than 1MB is probably wrong
                .unwrap_or(true)
        } else {
            true
        };
        
        if need_extract {
            use flate2::read::GzDecoder;
            let mut decoder = GzDecoder::new(EMBEDDED_SERVER);
            let mut data = Vec::new();
            decoder.read_to_end(&mut data)
                .map_err(|e| format!("Cannot decompress server: {e}"))?;
            
            let mut file = std::fs::File::create(&server_path)
                .map_err(|e| format!("Cannot create server exe: {e}"))?;
            file.write_all(&data)
                .map_err(|e| format!("Cannot write server exe: {e}"))?;
        }
        
        return Ok(server_path);
    }
    
    #[cfg(not(embedded_server))]
    {
        // Fallback: look for external server
        Err("Server not embedded and no external server found".to_string())
    }
}
