#![cfg_attr(windows, windows_subsystem = "windows")]

slint::include_modules!();

mod autostart;
mod azure_config;
mod claude_config;
mod config;
mod env_check;
mod models;
mod server;
mod hooks_config;

use config::{AppConfig, load_config, save_config};
use arboard::Clipboard;
use std::sync::{Arc, Mutex};
use std::io::{BufRead, BufReader, Read};
use std::thread;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config().unwrap_or_default();

    let startup_base_url = config.effective_claude_base_url();
    let claude_startup_status = claude_config::ensure_claude_files(&startup_base_url)
        .unwrap_or_else(|err| format!("Claude file check failed: {}", err));
    let azure_startup_status = azure_config::ensure_azure_openai_config(&config)
        .unwrap_or_else(|err| format!("Azure OpenAI check failed: {}", err));

    let ui = AppWindow::new()?;
    ui.set_api_base_url(config.api_base_url.clone().into());
    ui.set_api_key(config.api_key.clone().into());
    ui.set_autostart(config.autostart);
    ui.set_claude_base_url(config.claude_base_url.clone().into());
    ui.set_use_proxy(config.use_proxy);
    ui.set_proxy_url(config.proxy_url.clone().into());
    ui.set_proxy_scheme(config.proxy_scheme.clone().into());
    ui.set_proxy_username(config.proxy_username.clone().into());
    ui.set_proxy_password(config.proxy_password.clone().into());
    ui.set_server_port(config.server_port.to_string().into());
    ui.set_account_type(config.account_type.clone().into());
    ui.set_verbose(config.verbose);
    ui.set_manual(config.manual);
    ui.set_wait_rate_limit(config.wait);
    ui.set_rate_limit_seconds(config.rate_limit_seconds.to_string().into());
    ui.set_github_token(config.github_token.clone().into());
    ui.set_azure_enabled(config.azure_enabled);
    ui.set_azure_endpoint(config.azure_endpoint.clone().into());
    ui.set_azure_deployment(config.azure_deployment.clone().into());
    ui.set_azure_api_version(config.azure_api_version.clone().into());
    ui.set_azure_api_key(config.azure_api_key.clone().into());
    ui.set_show_copilot_section(config.show_copilot_section);
    ui.set_show_azure_section(config.show_azure_section);
    ui.set_hooks_enabled(config.hooks_enabled);
    ui.set_hooks_config_path(hooks_config::hooks_config_path_string().into());
    
    // Initialize model selection
    setup_model_selection(&ui, &config);
    
    let startup_status = format!("{}. {}", claude_startup_status, azure_startup_status);
    set_status(&ui, &startup_status);
    ui.set_github_login_url("https://github.com/login/device".into());

    let report = env_check::check_all();
    set_deps(&ui, &report);

    let server_handle: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));

    let ui_handle = ui.as_weak();
    ui.on_save(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let new_config = config_from_ui(&ui);
            match save_config(&new_config) {
                Ok(_) => {
                    let effective = new_config.effective_claude_base_url();
                    let claude_message = claude_config::ensure_claude_files(&effective)
                        .unwrap_or_else(|err| format!("Claude check failed: {}", err));
                    let azure_message = azure_config::ensure_azure_openai_config(&new_config)
                        .unwrap_or_else(|err| format!("Azure OpenAI check failed: {}", err));
                    set_status(&ui, &format!("Saved. {}. {}", claude_message, azure_message));
                }
                Err(err) => set_status(&ui, &format!("Save failed: {}", err)),
            }
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_toggle_autostart(move |enable| {
        if let Some(ui) = ui_handle.upgrade() {
            match autostart::set_autostart(enable) {
                Ok(_) => {
                    let mut new_config = config_from_ui(&ui);
                    new_config.autostart = enable;
                    let _ = save_config(&new_config);
                    set_status(
                        &ui,
                        if enable { "Autostart enabled" } else { "Autostart disabled" },
                    );
                }
                Err(err) => {
                    ui.set_autostart(!enable);
                    set_status(&ui, &format!("Autostart update failed: {}", err));
                }
            }
        }
    });

    let ui_handle = ui.as_weak();
    let server_handle_start = server_handle.clone();
    ui.on_start_server(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let mut guard = server_handle_start.lock().unwrap();
            if guard.is_some() {
                set_status(&ui, "Server already running");
                return;
            }

            let config = config_from_ui(&ui);
            match server::start_server(&config) {
                Ok(mut child) => {
                    let effective = config.effective_claude_base_url();
                    let _ = save_config(&config);
                    let message = claude_config::ensure_claude_files(&effective)
                        .unwrap_or_else(|err| format!("Claude file check failed: {}", err));
                    ui.set_server_running(true);
                    let start_message = format!("Server started on port {}. {}", config.server_port, message);
                    set_status(&ui, &start_message);
                    append_log(&ui_handle, &start_message);
                    let stdout = child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
                    let stderr = child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
                    let ui_stream = ui_handle.clone();
                    spawn_log_watcher(stdout, ui_stream.clone());
                    spawn_log_watcher(stderr, ui_stream);
                    *guard = Some(child);
                    
                    // Refresh model list from server after it starts
                    refresh_models_from_server(ui_handle.clone(), config.server_port);
                }
                Err(err) => {
                    set_status(&ui, &err);
                    append_log(&ui_handle, &format!("Server start failed: {}", err));
                }
            }
        }
    });

    let ui_handle = ui.as_weak();
    let server_handle_stop = server_handle.clone();
    ui.on_stop_server(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let mut guard = server_handle_stop.lock().unwrap();
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
                // Clear device code and update state when server stops
                ui.set_github_device_code("".into());
                ui.set_server_running(false);
                set_status(&ui, "Server stopped");
                append_log(&ui_handle, "Server stopped");
            } else {
                set_status(&ui, "Server is not running");
            }
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_check_deps(move || {
        if let Some(ui) = ui_handle.upgrade() {
            set_status(&ui, "Checking dependencies...");
            let ui_weak = ui_handle.clone();
            thread::spawn(move || {
                let report = env_check::check_all();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        set_deps(&ui, &report);
                        set_status(&ui, &report.summary);
                    }
                });
            });
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_install_deps(move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.set_installing(true);
            set_status(&ui, "Installing dependencies... (this may take a few minutes)");
            let ui_weak = ui_handle.clone();
            thread::spawn(move || {
                let report = env_check::check_all();
                let message = env_check::install_missing(&report);
                let updated = env_check::check_all();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_installing(false);
                        set_deps(&ui, &updated);
                        set_status(&ui, &message);
                    }
                });
            });
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_copy_device_code(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let value = ui.get_github_device_code().to_string();
            if !value.trim().is_empty() {
                match set_clipboard_text(&value) {
                    Ok(_) => ui.set_status_text("Device code copied to clipboard".into()),
                    Err(err) => ui.set_status_text(format!("Clipboard error: {}", err).into()),
                }
            } else {
                ui.set_status_text("Device code is empty".into());
            }
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_copy_login_url(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let value = ui.get_github_login_url().to_string();
            if !value.trim().is_empty() {
                match set_clipboard_text(&value) {
                    Ok(_) => ui.set_status_text("Login URL copied to clipboard".into()),
                    Err(err) => ui.set_status_text(format!("Clipboard error: {}", err).into()),
                }
            } else {
                ui.set_status_text("Login URL is empty".into());
            }
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_open_copilot_auth(move || {
        if let Some(ui) = ui_handle.upgrade() {
            set_status(&ui, "Starting Copilot auth flow...");
            
            // Run auth command from embedded server
            let ui_weak = ui.as_weak();
            std::thread::spawn(move || {
                match run_auth_command() {
                    Ok((code, url)) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                if !code.is_empty() {
                                    ui.set_github_device_code(code.into());
                                }
                                if !url.is_empty() {
                                    ui.set_github_login_url(url.clone().into());
                                    let _ = open_url(&url);
                                }
                                set_status(&ui, "Device code ready - enter it on the opened page");
                            }
                        });
                    }
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                set_status(&ui, &format!("Auth error: {}", e));
                                // Fallback: just open the page
                                let _ = open_url("https://github.com/login/device");
                            }
                        });
                    }
                }
            });
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_copy_log(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let log_text = get_log_text();
            if !log_text.is_empty() {
                match set_clipboard_text(&log_text) {
                    Ok(_) => set_status(&ui, "Log copied to clipboard"),
                    Err(err) => set_status(&ui, &format!("Clipboard error: {}", err)),
                }
            } else {
                set_status(&ui, "Log is empty");
            }
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_clear_log(move || {
        if let Some(ui) = ui_handle.upgrade() {
            clear_log_buffer(&ui);
            set_status(&ui, "Log cleared");
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_open_hooks_config(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let path = hooks_config::hooks_config_path_string();
            if let Err(err) = open_url(&path) {
                set_status(&ui, &format!("Open hooks config failed: {}", err));
            } else {
                set_status(&ui, "Hooks config opened");
            }
        }
    });

    ui.run()?;
    Ok(())
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/c", "start", "", url]);
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        cmd.spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

/// Run the auth command from the embedded server to get device code
fn run_auth_command() -> Result<(String, String), String> {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    
    let server_exe = server::get_server_exe_path()?;
    
    let mut cmd = std::process::Command::new(&server_exe);
    cmd.arg("auth")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    
    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn auth: {e}"))?;

    let (tx, rx) = mpsc::channel::<String>();

    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().flatten() {
                let _ = tx.send(line);
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                let _ = tx.send(line);
            }
        });
    }

    let mut code = String::new();
    let mut url = String::new();
    let timeout = Duration::from_secs(20);
    let start = Instant::now();

    while start.elapsed() < timeout {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if let Some((c, u)) = parse_device_code_line(&line) {
                    if !c.is_empty() && code.is_empty() {
                        code = c;
                    }
                    if !u.is_empty() && url.is_empty() {
                        url = u;
                    }
                    if !code.is_empty() && !url.is_empty() {
                        break;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    if !code.is_empty() {
        if url.is_empty() {
            url = "https://github.com/login/device".to_string();
        }
        Ok((code, url))
    } else {
        Err("No device code found in auth output. You may already be logged in.".to_string())
    }
}

fn set_clipboard_text(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|err| err.to_string())?;
    clipboard.set_text(text.to_string()).map_err(|err| err.to_string())
}

/// Global log storage for copying
static LOG_BUFFER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            // Skip ESC sequences like \x1b[31m
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == 'm' {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn append_log(ui: &slint::Weak<AppWindow>, line: &str) {
    let line = strip_ansi(line);
    if line.trim().is_empty() {
        return;
    }
    let ui = ui.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            // Append to global buffer
            if let Ok(mut buffer) = LOG_BUFFER.lock() {
                buffer.push_str(&line);
                buffer.push('\n');
                // Limit buffer size to ~100KB
                if buffer.len() > 100_000 {
                    let new_start = buffer.len() - 80_000;
                    *buffer = buffer[new_start..].to_string();
                }
                ui.set_log_text(buffer.clone().into());
            }
        }
    });
}

fn clear_log_buffer(ui: &AppWindow) {
    if let Ok(mut buffer) = LOG_BUFFER.lock() {
        buffer.clear();
        ui.set_log_text("".into());
    }
}

fn get_log_text() -> String {
    LOG_BUFFER.lock().map(|b| b.clone()).unwrap_or_default()
}

fn spawn_log_watcher(stream: Option<Box<dyn Read + Send>>, ui: slint::Weak<AppWindow>) {
    if let Some(out) = stream {
        thread::spawn(move || {
            let reader = BufReader::new(out);
            for line in reader.lines().flatten() {
                // Append to GUI log
                append_log(&ui, &line);
                
                // Also check for device code
                if let Some((code, url)) = parse_device_code_line(&line) {
                    let ui_clone = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_clone.upgrade() {
                            if !code.is_empty() {
                                ui.set_github_device_code(code.into());
                            }
                            if !url.is_empty() {
                                ui.set_github_login_url(url.into());
                            }
                            set_status(&ui, "Device code received. Open login URL to authorize.");
                        }
                    });
                }
            }
        });
    }
}

fn set_status(ui: &AppWindow, text: &str) {
    ui.set_status_text(text.into());
    ui.set_status_short(short_status(text).into());
}

fn short_status(text: &str) -> String {
    let trimmed = text.trim();
    let max = 48usize;
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out = String::new();
    for (i, ch) in trimmed.chars().enumerate() {
        if i >= max - 1 {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn set_deps(ui: &AppWindow, report: &env_check::DependencyReport) {
    ui.set_deps_summary(report.summary.clone().into());
    ui.set_deps_text(report.details.clone().into());
    let lines: Vec<&str> = report.details.lines().collect();
    set_line(ui, 1, lines.get(0));
    set_line(ui, 2, lines.get(1));
    set_line(ui, 3, lines.get(2));
    set_line(ui, 4, lines.get(3));
    set_line(ui, 5, lines.get(4));
    set_line(ui, 6, lines.get(5));
    set_line(ui, 7, lines.get(6));
    set_line(ui, 8, lines.get(7));
}

fn set_line(ui: &AppWindow, index: usize, value: Option<&&str>) {
    let text = value.copied().unwrap_or("");
    match index {
        1 => ui.set_deps_line1(text.into()),
        2 => ui.set_deps_line2(text.into()),
        3 => ui.set_deps_line3(text.into()),
        4 => ui.set_deps_line4(text.into()),
        5 => ui.set_deps_line5(text.into()),
        6 => ui.set_deps_line6(text.into()),
        7 => ui.set_deps_line7(text.into()),
        8 => ui.set_deps_line8(text.into()),
        _ => {}
    }
}

fn parse_device_code_line(line: &str) -> Option<(String, String)> {
    let lower = line.to_lowercase();
    // Match various log formats mentioning device code
    if !lower.contains("login/device") && !lower.contains("device code") && !lower.contains("user code") {
        return None;
    }

    let url = if let Some(start) = line.find("https://") {
        let tail = &line[start..];
        tail.split_whitespace().next().unwrap_or("").to_string()
    } else {
        "https://github.com/login/device".to_string()
    };

    let mut code = String::new();
    
    // Try to find code in quotes first
    if let Some(first) = line.find('"') {
        if let Some(second) = line[first + 1..].find('"') {
            code = line[first + 1..first + 1 + second].to_string();
        }
    }
    
    // Also try pattern like "code: XXXX-XXXX" or "Code: XXXX-XXXX"
    if code.is_empty() {
        for pattern in ["code: ", "Code: ", "code:", "Code:"] {
            if let Some(pos) = line.find(pattern) {
                let after = &line[pos + pattern.len()..];
                if let Some(found) = after.split_whitespace().next() {
                    // Device codes are typically XXXX-XXXX format
                    if found.contains('-') && found.len() >= 8 {
                        code = found.to_string();
                        break;
                    }
                }
            }
        }
    }

    if code.is_empty() {
        None
    } else {
        Some((code, url))
    }
}

fn config_from_ui(ui: &AppWindow) -> AppConfig {
    let server_port = ui
        .get_server_port()
        .trim()
        .parse::<u16>()
        .unwrap_or(4141);
    let rate_limit_seconds = ui
        .get_rate_limit_seconds()
        .trim()
        .parse::<u64>()
        .unwrap_or(0);

    AppConfig {
        api_base_url: ui.get_api_base_url().to_string(),
        api_key: ui.get_api_key().to_string(),
        autostart: ui.get_autostart(),
        claude_base_url: ui.get_claude_base_url().to_string(),
        use_proxy: ui.get_use_proxy(),
        proxy_url: ui.get_proxy_url().to_string(),
        proxy_scheme: ui.get_proxy_scheme().to_string(),
        proxy_username: ui.get_proxy_username().to_string(),
        proxy_password: ui.get_proxy_password().to_string(),
        server_port,
        account_type: ui.get_account_type().to_string(),
        verbose: ui.get_verbose(),
        manual: ui.get_manual(),
        wait: ui.get_wait_rate_limit(),
        rate_limit_seconds,
        github_token: ui.get_github_token().to_string(),
        azure_enabled: ui.get_azure_enabled(),
        azure_endpoint: ui.get_azure_endpoint().to_string(),
        azure_deployment: ui.get_azure_deployment().to_string(),
        azure_api_version: ui.get_azure_api_version().to_string(),
        azure_api_key: ui.get_azure_api_key().to_string(),
        show_copilot_section: ui.get_show_copilot_section(),
        show_azure_section: ui.get_show_azure_section(),
        main_model: ui.get_main_model().to_string(),
        fast_model: ui.get_fast_model().to_string(),
        // Preserve cached models from existing config
        cached_models: load_config().map(|c| c.cached_models).unwrap_or_default(),
        hooks_enabled: ui.get_hooks_enabled(),
    }
}

fn setup_model_selection(ui: &AppWindow, config: &AppConfig) {
    // At startup, only use cached models or fallback (server not running yet)
    let model_list = models::get_cached_or_fallback(&config.cached_models);
    
    // Convert to Slint model
    let model_vec: Vec<slint::SharedString> = model_list.iter().map(|s| s.as_str().into()).collect();
    let slint_model = std::rc::Rc::new(slint::VecModel::from(model_vec));
    ui.set_available_models(slint_model.into());
    
    // Restore selection values
    ui.set_main_model(config.main_model.clone().into());
    ui.set_fast_model(config.fast_model.clone().into());
}

/// Refresh model list from server after it starts
fn refresh_models_from_server(ui_weak: slint::Weak<AppWindow>, port: u16) {
    std::thread::spawn(move || {
        // Wait a bit for server to be ready
        std::thread::sleep(std::time::Duration::from_secs(3));
        
        if let Some(mut model_list) = models::fetch_models_from_server(port) {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    // Get current selections before updating
                    let current_main = ui.get_main_model().to_string();
                    let current_fast = ui.get_fast_model().to_string();
                    
                    // Ensure current selections are in the list
                    // (user may have selected a model that's not from server, like claude-opus-4.5)
                    if !current_main.is_empty() && !model_list.contains(&current_main) {
                        model_list.insert(0, current_main.clone());
                    }
                    if !current_fast.is_empty() && !model_list.contains(&current_fast) {
                        model_list.push(current_fast.clone());
                    }
                    
                    // Update cached models in config
                    let mut config = config_from_ui(&ui);
                    config.cached_models = model_list.clone();
                    let _ = save_config(&config);
                    
                    // Update UI model list
                    let model_vec: Vec<slint::SharedString> = model_list.iter().map(|s| s.as_str().into()).collect();
                    let slint_model = std::rc::Rc::new(slint::VecModel::from(model_vec));
                    ui.set_available_models(slint_model.into());

                    // Restore selection values explicitly (ensure no unexpected reset)
                    if !current_main.is_empty() {
                        ui.set_main_model(current_main.clone().into());
                    }
                    if !current_fast.is_empty() {
                        ui.set_fast_model(current_fast.clone().into());
                    }
                    
                    // Re-apply selection values (ComboBox will keep if present)
                    if !current_main.is_empty() {
                        ui.set_main_model(current_main.into());
                    }
                    if !current_fast.is_empty() {
                        ui.set_fast_model(current_fast.into());
                    }
                    
                    set_status(&ui, "Model list refreshed from server");
                    append_log(&ui_weak, "Model list refreshed from server");
                }
            });
        }
    });
}
