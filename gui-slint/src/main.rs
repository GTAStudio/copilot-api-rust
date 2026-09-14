#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

slint::include_modules!();

#[cfg(all(test, debug_assertions))]
mod ui_tests;

mod auth;
mod autostart;
mod azure_config;
mod claude_config;
mod config;
mod desktop;
mod env_check;
mod hooks_config;
mod localization;
mod models;
mod server;

use auth::parse_device_code_line;
use config::{load_config, save_config, AppConfig};
use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Mutex};
use std::thread;

#[cfg(test)]
use auth::watch_auth_command;

struct GuiApplication {
    ui: AppWindow,
    server_handle: auth::ProcessHandle,
    auth_handle: auth::ProcessHandle,
    auth_cancelled: Arc<std::sync::atomic::AtomicBool>,
    _server_status_timer: slint::Timer,
}

impl Drop for GuiApplication {
    fn drop(&mut self) {
        self.auth_cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        auth::stop_process(&self.auth_handle);
        auth::stop_process(&self.server_handle);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config()?;
    let services: Arc<dyn desktop::DesktopServices> = Arc::new(desktop::SystemDesktop);
    let report = services.dependencies();
    let application = initialize_application(config, report, services)?;
    application.ui.run()?;
    Ok(())
}

fn initialize_application(
    config: AppConfig,
    report: env_check::DependencyReport,
    services: Arc<dyn desktop::DesktopServices>,
) -> Result<GuiApplication, slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.on_translate_message(|message| localization::translate_message(&message).into());
    ui.set_is_chinese(config.is_chinese);
    ui.set_provider(config.provider.clone().into());
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

    set_status(&ui, "Ready");
    ui.set_github_login_url("https://github.com/login/device".into());

    set_deps(&ui, &report);

    let ui_handle = ui.as_weak();
    ui.on_language_changed(move |is_chinese| {
        if let Some(ui) = ui_handle.upgrade() {
            ui.set_is_chinese(is_chinese);
            if let Err(error) = config::save_language_preference(is_chinese) {
                set_status(&ui, &format!("Language preference save failed: {error}"));
            }
        }
    });

    let server_handle: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    let auth_handle = Arc::new(Mutex::new(None));
    let auth_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server_status_timer = slint::Timer::default();
    let monitored_server = server_handle.clone();
    let monitored_ui = ui.as_weak();
    server_status_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(500),
        move || {
            if let (Some(ui), Ok(mut guard)) = (monitored_ui.upgrade(), monitored_server.lock()) {
                if let Some(child) = guard.as_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        guard.take();
                        ui.set_server_running(false);
                        set_status(&ui, &format!("Server exited: {status}"));
                    }
                }
            }
        },
    );

    let ui_handle = ui.as_weak();
    ui.on_save(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let new_config = config_from_ui(&ui);
            match save_config(&new_config) {
                Ok(_) => {
                    let azure_message = azure_config::ensure_azure_openai_config(&new_config)
                        .unwrap_or_else(|err| format!("Azure OpenAI check failed: {}", err));
                    set_status(&ui, &format!("Saved. {}", azure_message));
                }
                Err(err) => set_status(&ui, &format!("Save failed: {}", err)),
            }
        }
    });

    let ui_handle = ui.as_weak();
    ui.on_configure_claude(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let result = claude_config::configure_claude_code(&config_from_ui(&ui));
            set_status(
                &ui,
                &result.unwrap_or_else(|error| format!("Claude Code settings failed: {error}")),
            );
        }
    });

    let ui_handle = ui.as_weak();
    let desktop = services.clone();
    ui.on_toggle_autostart(move |enable| {
        if let Some(ui) = ui_handle.upgrade() {
            match desktop.autostart(enable) {
                Ok(_) => {
                    ui.set_autostart(enable);
                    let mut new_config = config_from_ui(&ui);
                    new_config.autostart = enable;
                    let _ = save_config(&new_config);
                    set_status(
                        &ui,
                        if enable {
                            "Autostart enabled"
                        } else {
                            "Autostart disabled"
                        },
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
                    let _ = save_config(&config);
                    ui.set_server_running(true);
                    let start_message =
                        format!("Server process started on port {}", config.server_port);
                    set_status(&ui, &start_message);
                    append_log(&ui_handle, &start_message);
                    let stdout = child
                        .stdout
                        .take()
                        .map(|s| Box::new(s) as Box<dyn Read + Send>);
                    let stderr = child
                        .stderr
                        .take()
                        .map(|s| Box::new(s) as Box<dyn Read + Send>);
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
    let desktop = services.clone();
    ui.on_check_deps(move || {
        if let Some(ui) = ui_handle.upgrade() {
            set_status(&ui, "Checking dependencies...");
            let ui_weak = ui_handle.clone();
            let desktop = desktop.clone();
            thread::spawn(move || {
                let report = desktop.dependencies();
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
    let desktop = services.clone();
    ui.on_install_deps(move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.set_installing(true);
            set_status(
                &ui,
                "Installing dependencies... (this may take a few minutes)",
            );
            let ui_weak = ui_handle.clone();
            let desktop = desktop.clone();
            thread::spawn(move || {
                let report = desktop.dependencies();
                let message = env_check::install_missing(&report);
                let updated = desktop.dependencies();
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
    let desktop = services.clone();
    ui.on_copy_device_code(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let value = ui.get_github_device_code().to_string();
            if !value.trim().is_empty() {
                match desktop.clipboard(&value) {
                    Ok(_) => ui.set_status_text("Device code copied to clipboard".into()),
                    Err(err) => ui.set_status_text(format!("Clipboard error: {}", err).into()),
                }
            } else {
                ui.set_status_text("Device code is empty".into());
            }
        }
    });

    let ui_handle = ui.as_weak();
    let desktop = services.clone();
    ui.on_copy_login_url(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let value = ui.get_github_login_url().to_string();
            if !value.trim().is_empty() {
                match desktop.clipboard(&value) {
                    Ok(_) => ui.set_status_text("Login URL copied to clipboard".into()),
                    Err(err) => ui.set_status_text(format!("Clipboard error: {}", err).into()),
                }
            } else {
                ui.set_status_text("Login URL is empty".into());
            }
        }
    });

    let ui_handle = ui.as_weak();
    let auth_handle_start = auth_handle.clone();
    let cancelled = auth_cancelled.clone();
    let desktop = services.clone();
    ui.on_open_copilot_auth(move || {
        if let Some(ui) = ui_handle.upgrade() {
            if ui.get_authenticating() {
                return;
            }
            set_status(&ui, "Starting Copilot auth flow...");
            ui.set_authenticating(true);
            let config = config_from_ui(&ui);
            let process_handle = auth_handle_start.clone();
            let ui_weak = ui.as_weak();
            let desktop = desktop.clone();
            let cancelled = cancelled.clone();
            std::thread::spawn(move || {
                let device_ui = ui_weak.clone();
                let device_desktop = desktop.clone();
                let result = desktop.authenticate(
                    &config,
                    &process_handle,
                    &cancelled,
                    Box::new(move |code, url| {
                        let device_ui = device_ui.clone();
                        let desktop = device_desktop.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = device_ui.upgrade() {
                                ui.set_github_device_code(code.into());
                                ui.set_github_login_url(url.clone().into());
                                match desktop.open(&url) {
                                    Ok(()) => set_status(&ui, "Waiting for GitHub authorization"),
                                    Err(error) => set_status(
                                        &ui,
                                        &format!("Cannot open GitHub authorization URL: {error}"),
                                    ),
                                }
                            }
                        });
                    }),
                );
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_authenticating(false);
                        match result {
                            Ok(()) => set_status(
                                &ui,
                                "GitHub authorized. Server restart required for account changes.",
                            ),
                            Err(error) => set_status(&ui, &format!("Auth error: {error}")),
                        }
                    }
                });
            });
        }
    });

    let ui_handle = ui.as_weak();
    let desktop = services.clone();
    ui.on_copy_log(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let log_text = get_log_text();
            if !log_text.is_empty() {
                match desktop.clipboard(&log_text) {
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
    let desktop = services.clone();
    ui.on_open_hooks_config(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let path = hooks_config::hooks_config_path_string();
            if let Err(err) = desktop.open(&path) {
                set_status(&ui, &format!("Open hooks config failed: {}", err));
            } else {
                set_status(&ui, "Hooks config opened");
            }
        }
    });

    Ok(GuiApplication {
        ui,
        server_handle,
        auth_handle,
        auth_cancelled,
        _server_status_timer: server_status_timer,
    })
}

/// Global log storage for copying
static LOG_BUFFER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn trim_log_buffer(buffer: &mut String) {
    if buffer.len() > 100_000 {
        let start = buffer.ceil_char_boundary(buffer.len() - 80_000);
        buffer.drain(..start);
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            // Skip ESC sequences like \x1b[31m
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for c in chars.by_ref() {
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
                trim_log_buffer(&mut buffer);
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
            for line in reader.lines().map_while(Result::ok) {
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
    set_line(ui, 1, lines.first());
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

fn config_from_ui(ui: &AppWindow) -> AppConfig {
    let server_port = ui.get_server_port().trim().parse::<u16>().unwrap_or(0);
    let rate_limit_seconds = ui
        .get_rate_limit_seconds()
        .trim()
        .parse::<u64>()
        .unwrap_or(u64::MAX);

    AppConfig {
        is_chinese: ui.get_is_chinese(),
        provider: ui.get_provider().to_string(),
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
    let model_vec: Vec<slint::SharedString> =
        model_list.iter().map(|s| s.as_str().into()).collect();
    let slint_model = std::rc::Rc::new(slint::VecModel::from(model_vec));
    ui.set_available_models(slint_model.into());

    // Restore selection values
    ui.set_main_model(config.main_model.clone().into());
    ui.set_fast_model(config.fast_model.clone().into());
}

fn apply_model_catalogue(ui: &AppWindow, model_list: Vec<String>) -> std::io::Result<()> {
    let mut config = config_from_ui(ui);
    if !model_list.contains(&config.main_model) {
        config.main_model.clear();
    }
    if !model_list.contains(&config.fast_model) {
        config.fast_model.clear();
    }
    config.cached_models = model_list.clone();
    save_config(&config)?;
    let model_vec: Vec<slint::SharedString> = model_list
        .iter()
        .map(|model| model.as_str().into())
        .collect();
    ui.set_available_models(std::rc::Rc::new(slint::VecModel::from(model_vec)).into());
    ui.set_main_model(config.main_model.into());
    ui.set_fast_model(config.fast_model.into());
    Ok(())
}

fn refresh_models_from_server(ui_weak: slint::Weak<AppWindow>, port: u16) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));

        if let Some(model_list) = models::fetch_models_from_server(port) {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    if !ui.get_server_running()
                        || ui.get_server_port().trim().parse::<u16>().ok() != Some(port)
                    {
                        return;
                    }
                    match apply_model_catalogue(&ui, model_list) {
                        Ok(()) => {
                            set_status(&ui, "Model list refreshed from server");
                            append_log(&ui_weak, "Model list refreshed from server");
                        }
                        Err(error) => {
                            set_status(&ui, &format!("Model cache update failed: {error}"))
                        }
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingDesktop {
        calls: Mutex<Vec<String>>,
        fail: std::sync::atomic::AtomicBool,
    }

    impl desktop::DesktopServices for RecordingDesktop {
        fn clipboard(&self, value: &str) -> Result<(), String> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("clipboard:{value}"));
            if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                Err("fixture clipboard failure".to_string())
            } else {
                Ok(())
            }
        }
        fn open(&self, value: &str) -> Result<(), String> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("open:{value}"));
            if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                Err("fixture open failure".to_string())
            } else {
                Ok(())
            }
        }
        fn autostart(&self, enabled: bool) -> Result<(), String> {
            self.calls
                .lock()
                .expect("calls")
                .push(format!("autostart:{enabled}"));
            if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                Err("fixture autostart failure".to_string())
            } else {
                Ok(())
            }
        }
        fn dependencies(&self) -> env_check::DependencyReport {
            self.calls
                .lock()
                .expect("calls")
                .push("dependencies".to_string());
            env_check::DependencyReport {
                summary: "Fixture checked".to_string(),
                details: "Fixture dependency".to_string(),
                missing: Vec::new(),
            }
        }
        fn authenticate(
            &self,
            _config: &AppConfig,
            _process: &auth::ProcessHandle,
            cancelled: &std::sync::atomic::AtomicBool,
            mut on_device: Box<dyn FnMut(String, String) + Send>,
        ) -> Result<(), String> {
            assert!(!cancelled.load(std::sync::atomic::Ordering::Acquire));
            self.calls
                .lock()
                .expect("calls")
                .push("authenticate".to_string());
            on_device(
                "ABCD-EFGH".to_string(),
                "https://github.com/login/device".to_string(),
            );
            if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                Err("fixture auth failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn run_ui_until(condition: impl Fn() -> bool + 'static) {
        let completed = std::rc::Rc::new(std::cell::Cell::new(false));
        let observed = completed.clone();
        let timer = slint::Timer::default();
        let started = std::time::Instant::now();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(10),
            move || {
                if condition() {
                    observed.set(true);
                    slint::quit_event_loop().expect("test event loop");
                } else if started.elapsed() > std::time::Duration::from_secs(5) {
                    slint::quit_event_loop().expect("test deadline");
                }
            },
        );
        slint::run_event_loop().expect("headless event loop");
        assert!(
            completed.get(),
            "UI workflow did not complete before the test deadline"
        );
    }

    #[test]
    fn gui_branding_assets_are_valid_pngs() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/assets");
        for name in ["logo.png", "app-icon.png"] {
            let bytes = std::fs::read(assets.join(name))
                .expect("original GameCheater branding asset must be bundled");
            assert!(bytes.len() >= 24, "{name} must contain a PNG header");
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{name} must be PNG");
            let width = u32::from_be_bytes(bytes[16..20].try_into().expect("PNG width"));
            let height = u32::from_be_bytes(bytes[20..24].try_into().expect("PNG height"));
            assert!(
                width >= 32 && height >= 32,
                "{name} must have usable dimensions"
            );
        }
    }

    #[test]
    fn headless_gui_settings_workflow_is_isolated() {
        let directory =
            tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
                .expect("isolated GUI directory");
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::headless_gui_settings_fixture",
                "--nocapture",
            ])
            .env("COPILOT_GUI_TEST_FIXTURE", "1")
            .env("COPILOT_GUI_CONFIG_DIR", directory.path().join("gui"))
            .env("COPILOT_GUI_CACHE_DIR", directory.path().join("cache"))
            .env("CLAUDE_CONFIG_DIR", directory.path().join("claude"))
            .env("COPILOT_DATA_DIR", directory.path().join("server"))
            .env("COPILOT_API_KEY", "unit-test-gui-local-key")
            .env("COPILOT_EDITOR_VERSION", "1.0.0")
            .env("RUST_LOG", "info")
            .output()
            .expect("headless GUI fixture");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("HEADLESS_GUI_SETTINGS_PASSED"));
    }

    #[test]
    fn headless_gui_settings_fixture() {
        if std::env::var("COPILOT_GUI_TEST_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        use slint::ComponentHandle;
        let root = std::path::PathBuf::from(
            std::env::var_os("COPILOT_GUI_CONFIG_DIR").expect("GUI directory"),
        );
        assert_eq!(
            config::config_dir_path().expect("config directory"),
            root,
            "never write real GUI settings"
        );
        let cache_root = std::path::PathBuf::from(
            std::env::var_os("COPILOT_GUI_CACHE_DIR").expect("cache directory"),
        );
        assert_eq!(
            server::cache_directory().expect("cache directory"),
            cache_root,
            "never write real GUI cache"
        );
        i_slint_backend_testing::init_integration_test_with_system_time();
        let config = AppConfig {
            is_chinese: false,
            server_port: 5050,
            cached_models: vec![
                "claude-sonnet-4-6".to_string(),
                "claude-haiku-4-5".to_string(),
            ],
            ..Default::default()
        };
        let report = env_check::DependencyReport {
            summary: "Fixture ready".to_string(),
            details: "Server fixture\nClient fixture".to_string(),
            missing: Vec::new(),
        };
        let services = Arc::new(RecordingDesktop::default());
        let application =
            initialize_application(config, report, services.clone()).expect("headless application");
        let ui = &application.ui;
        assert!(
            !ui.get_is_chinese(),
            "restore the saved language at startup"
        );
        assert!(ui.global::<Branding>().get_logo().size().width > 0);
        assert!(ui.global::<Branding>().get_app_icon().size().height > 0);
        assert_eq!(
            ui.global::<Theme>().get_accent(),
            slint::Color::from_rgb_u8(255, 176, 0)
        );
        assert_eq!(ui.get_active_page(), 0);
        for page in 0..5 {
            ui.invoke_select_page(page);
            assert_eq!(ui.get_active_page(), page);
            assert_eq!(ui.get_server_port().as_str(), "5050");
        }
        ui.invoke_select_page(-1);
        assert_eq!(ui.get_active_page(), 4);
        ui.invoke_select_page(5);
        assert_eq!(ui.get_active_page(), 4);
        ui.invoke_select_page(0);
        ui.global::<Theme>().set_dark(true);
        assert_eq!(
            ui.global::<Theme>().get_window_bg(),
            slint::Color::from_rgb_u8(25, 25, 25)
        );
        ui.global::<Theme>().set_dark(false);
        assert_eq!(ui.get_server_port().as_str(), "5050");
        assert_eq!(ui.get_main_model().as_str(), "claude-sonnet-4-6");
        apply_model_catalogue(ui, vec!["claude-sonnet-4-6".to_string()])
            .expect("current catalogue");
        assert_eq!(config_from_ui(ui).cached_models, vec!["claude-sonnet-4-6"]);
        assert!(
            ui.get_fast_model().is_empty(),
            "unavailable model must not be inserted into the catalogue"
        );
        ui.set_server_port("6060".into());
        ui.set_main_model("claude-haiku-4-5".into());
        ui.invoke_save();
        let saved = load_config().expect("saved GUI config");
        assert_eq!(saved.server_port, 6060);
        assert_eq!(saved.main_model, "claude-haiku-4-5");
        let path = config::config_file_path().expect("settings path");
        let previous = std::fs::read(&path).expect("saved bytes");
        ui.set_server_port("invalid".into());
        ui.invoke_save();
        assert!(ui.get_status_text().contains("Save failed"));
        assert_eq!(std::fs::read(&path).expect("unchanged bytes"), previous);
        ui.invoke_language_changed(true);
        assert!(ui.get_is_chinese());
        assert!(ui
            .get_localized_status()
            .starts_with("\u{4fdd}\u{5b58}\u{5931}\u{8d25}"));
        assert_eq!(
            ui.get_server_port().as_str(),
            "invalid",
            "language must preserve unsaved edits"
        );
        let language_config = load_config().expect("saved language");
        assert!(language_config.is_chinese);
        assert_eq!(
            language_config.server_port, 6060,
            "language must not save other controls"
        );
        let reopened = initialize_application(
            language_config,
            env_check::DependencyReport {
                summary: "Fixture ready".to_string(),
                details: String::new(),
                missing: Vec::new(),
            },
            services.clone(),
        )
        .expect("reopened language fixture");
        assert!(reopened.ui.get_is_chinese());
        drop(reopened);
        ui.invoke_language_changed(false);
        assert!(!load_config().expect("English preference").is_chinese);
        ui.set_server_port("6060".into());
        let claude_root = std::path::PathBuf::from(
            std::env::var_os("CLAUDE_CONFIG_DIR").expect("isolated Claude directory"),
        );
        std::fs::create_dir_all(&claude_root).expect("Claude fixture directory");
        let claude_path = claude_root.join("settings.json");
        std::fs::write(
            &claude_path,
            r#"{"permissions":{"deny":["Read(private)"]},"env":{"KEEP":"value"}}"#,
        )
        .expect("existing Claude fixture");
        ui.invoke_configure_claude();
        let updated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&claude_path).expect("Claude settings"))
                .expect("Claude JSON");
        assert_eq!(
            updated["env"]["ANTHROPIC_BASE_URL"],
            "http://127.0.0.1:6060"
        );
        assert_eq!(
            updated["env"]["ANTHROPIC_AUTH_TOKEN"],
            "unit-test-gui-local-key"
        );
        assert_eq!(updated["env"]["KEEP"], "value");
        assert_eq!(updated["permissions"]["deny"][0], "Read(private)");
        ui.set_main_model("".into());
        ui.set_fast_model("".into());
        ui.invoke_configure_claude();
        let cleared: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&claude_path).expect("cleared Claude settings"))
                .expect("Claude JSON");
        assert!(
            cleared["env"].get("ANTHROPIC_MODEL").is_none(),
            "empty catalogue selection must remove stale model overrides"
        );
        assert!(cleared["env"]
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .is_none());
        assert_eq!(cleared["env"]["KEEP"], "value");
        for provider in ["anthropic", "openai"] {
            ui.set_provider(provider.into());
            ui.set_api_base_url("https://gateway.example.com/v1".into());
            ui.set_api_key("unit-test-gui-upstream-key".into());
            ui.set_rate_limit_seconds("3".into());
            ui.invoke_save();
            let saved = load_config().expect("explicit provider config");
            assert_eq!(saved.provider, provider);
            assert_eq!(saved.effective_provider(), provider);
            assert_eq!(saved.rate_limit_seconds, 3);
        }
        ui.set_provider("azure".into());
        ui.set_azure_enabled(false);
        ui.set_azure_endpoint("https://fixture.openai.azure.com".into());
        ui.set_azure_deployment("fixture-deployment".into());
        ui.set_azure_api_key("unit-test-azure-config-key".into());
        ui.set_azure_api_version("2024-10-21".into());
        ui.invoke_save();
        let azure_path = root.join("azure-openai.json");
        let azure: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&azure_path).expect("explicit Azure config"))
                .expect("Azure JSON");
        assert_eq!(azure["deployment"], "fixture-deployment");
        assert_eq!(
            azure["base_url"],
            "https://fixture.openai.azure.com/openai/v1/"
        );
        let chat_url = url::Url::parse(azure["chat_completions_url"].as_str().expect("chat URL"))
            .expect("structured URL");
        assert_eq!(
            chat_url.path(),
            "/openai/deployments/fixture-deployment/chat/completions"
        );
        assert_eq!(
            chat_url.query_pairs().collect::<Vec<_>>(),
            vec![("api-version".into(), "2024-10-21".into())]
        );
        ui.set_provider("openai".into());
        ui.set_rate_limit_seconds("0".into());
        std::fs::write(&claude_path, "{broken").expect("damaged fixture");
        ui.invoke_configure_claude();
        assert!(ui.get_status_text().contains("settings failed"));
        assert_eq!(
            std::fs::read_to_string(&claude_path).expect("preserved invalid document"),
            "{broken"
        );
        ui.invoke_stop_server();
        assert_eq!(ui.get_status_text().as_str(), "Server is not running");
        ui.set_server_port("0".into());
        ui.invoke_start_server();
        assert!(application
            .server_handle
            .lock()
            .expect("process handle")
            .is_none());
        assert!(!ui.get_server_running());
        let upstream_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserved upstream listener");
        let upstream_address = upstream_listener.local_addr().expect("upstream address");
        let reserved_port =
            std::net::TcpListener::bind("127.0.0.1:0").expect("available server port");
        let port = reserved_port.local_addr().expect("server address").port();
        drop(reserved_port);
        ui.set_server_port(port.to_string().into());
        ui.set_api_base_url(format!("http://{upstream_address}").into());
        ui.set_api_key("unit-test-no-upstream-calls".into());
        ui.set_hooks_enabled(false);
        ui.invoke_start_server();
        assert!(ui.get_server_running());
        let child_id = application
            .server_handle
            .lock()
            .expect("process handle")
            .as_ref()
            .expect("owned server")
            .id();
        ui.invoke_start_server();
        assert_eq!(ui.get_status_text().as_str(), "Server already running");
        assert_eq!(
            application
                .server_handle
                .lock()
                .expect("process handle")
                .as_ref()
                .expect("same server")
                .id(),
            child_id
        );
        ui.invoke_stop_server();
        assert!(!ui.get_server_running());
        assert!(application
            .server_handle
            .lock()
            .expect("stopped process")
            .is_none());
        assert_eq!(ui.get_status_text().as_str(), "Server stopped");
        ui.invoke_copy_device_code();
        assert_eq!(ui.get_status_text().as_str(), "Device code is empty");
        ui.set_github_device_code("ABCD-EFGH".into());
        ui.invoke_copy_device_code();
        assert!(services
            .calls
            .lock()
            .expect("calls")
            .contains(&"clipboard:ABCD-EFGH".to_string()));
        ui.invoke_copy_login_url();
        assert!(services
            .calls
            .lock()
            .expect("calls")
            .contains(&"clipboard:https://github.com/login/device".to_string()));
        ui.set_github_login_url("".into());
        ui.invoke_copy_login_url();
        assert_eq!(ui.get_status_text().as_str(), "Login URL is empty");
        ui.invoke_toggle_autostart(true);
        assert!(load_config().expect("autostart config").autostart);
        ui.invoke_toggle_autostart(false);
        assert!(!load_config().expect("autostart config").autostart);
        ui.invoke_open_hooks_config();
        assert_eq!(ui.get_status_text().as_str(), "Hooks config opened");
        services
            .fail
            .store(true, std::sync::atomic::Ordering::Relaxed);
        ui.invoke_copy_device_code();
        assert!(ui.get_status_text().contains("Clipboard error"));
        ui.set_github_login_url("https://github.com/login/device".into());
        ui.invoke_copy_login_url();
        assert!(ui.get_status_text().contains("Clipboard error"));
        ui.invoke_toggle_autostart(true);
        assert!(ui.get_status_text().contains("Autostart update failed"));
        ui.invoke_open_hooks_config();
        assert!(ui.get_status_text().contains("Open hooks config failed"));
        services
            .fail
            .store(false, std::sync::atomic::Ordering::Relaxed);
        ui.invoke_check_deps();
        let weak = ui.as_weak();
        run_ui_until(move || {
            weak.upgrade()
                .is_some_and(|ui| ui.get_deps_summary() == "Fixture checked")
        });
        ui.invoke_install_deps();
        let weak = ui.as_weak();
        run_ui_until(move || weak.upgrade().is_some_and(|ui| !ui.get_installing()));
        ui.invoke_open_copilot_auth();
        let weak = ui.as_weak();
        run_ui_until(move || weak.upgrade().is_some_and(|ui| !ui.get_authenticating()));
        assert!(ui.get_status_text().contains("GitHub authorized"));
        assert_eq!(ui.get_github_device_code().as_str(), "ABCD-EFGH");
        services
            .fail
            .store(true, std::sync::atomic::Ordering::Relaxed);
        ui.invoke_open_copilot_auth();
        let weak = ui.as_weak();
        run_ui_until(move || weak.upgrade().is_some_and(|ui| !ui.get_authenticating()));
        assert!(ui.get_status_text().contains("Auth error"));
        services
            .fail
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let weak = ui.as_weak();
        append_log(&weak, "\u{1b}[31mfixture log\u{1b}[0m");
        slint::invoke_from_event_loop(move || {
            let ui = weak.upgrade().expect("live window");
            assert!(ui.get_log_text().contains("fixture log"));
            ui.invoke_copy_log();
            assert_eq!(ui.get_status_text().as_str(), "Log copied to clipboard");
            ui.invoke_clear_log();
            assert!(ui.get_log_text().is_empty());
            ui.invoke_copy_log();
            assert_eq!(ui.get_status_text().as_str(), "Log is empty");
            slint::quit_event_loop().expect("quit mock event loop");
        })
        .expect("schedule assertions");
        slint::run_event_loop().expect("mock event loop");
        let cancelled = application.auth_cancelled.clone();
        drop(application);
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
        println!("HEADLESS_GUI_SETTINGS_PASSED");
    }

    #[test]
    fn gui_audit_log_truncation_preserves_unicode_boundaries() {
        let mut log = "\u{4e2d}".repeat(40_000);
        trim_log_buffer(&mut log);
        assert!(log.len() <= 80_000);
        assert!(log.chars().all(|character| character == '\u{4e2d}'));
    }

    #[test]
    fn gui_audit_device_code_parser_rejects_untrusted_links() {
        assert!(parse_device_code_line(
            "Please enter the code \"ABCD-EFGH\" in https://github.com/login/device"
        )
        .is_some());
        assert!(parse_device_code_line(
            "device code: ABCD-EFGH https://untrusted.example/login/device"
        )
        .is_none());
        assert!(parse_device_code_line(
            "device code: ABCD-EFGH https://github.com/login/device&command=unexpected"
        )
        .is_none());
    }

    #[test]
    fn gui_audit_auth_process_survives_device_code_notification() {
        use std::io::Write;
        let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "tests::auth_process_fixture", "--nocapture"])
            .env("COPILOT_GUI_AUTH_FIXTURE", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("fixture process");
        let handle = Arc::new(Mutex::new(Some(child)));
        let callback_handle = handle.clone();
        let mut observed = false;
        watch_auth_command(&handle, |code, url| {
            assert_eq!(code, "ABCD-EFGH");
            assert_eq!(url, "https://github.com/login/device");
            let mut guard = callback_handle.lock().expect("process lock");
            let child = guard.as_mut().expect("running process");
            assert!(child.try_wait().expect("process state").is_none());
            let mut stdin = child.stdin.take().expect("fixture stdin");
            stdin
                .write_all(b"continue\n")
                .expect("continue authorization");
            observed = true;
        })
        .expect("authorization completion");
        assert!(observed);
        assert!(handle.lock().expect("process lock").is_none());
    }

    #[test]
    fn closed_gui_cannot_launch_a_delayed_authentication_worker() {
        let handle = Arc::new(Mutex::new(None));
        let closed = std::sync::atomic::AtomicBool::new(true);
        let result = auth::run_auth_command(&AppConfig::default(), &handle, &closed, |_, _| {
            panic!("closed application must not begin authorization");
        });
        assert!(result
            .expect_err("closed authentication")
            .contains("cancelled"));
        assert!(handle.lock().expect("no late process").is_none());
    }

    #[test]
    fn auth_process_fixture() {
        if std::env::var("COPILOT_GUI_AUTH_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        use std::io::Write;
        println!("Please enter the code \"ABCD-EFGH\" in https://github.com/login/device");
        std::io::stdout().flush().expect("device output");
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("authorization acknowledgement");
        assert_eq!(input.trim(), "continue");
        println!("GitHub token saved");
    }
}
