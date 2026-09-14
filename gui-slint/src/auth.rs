use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{config::AppConfig, server};

pub type ProcessHandle = Arc<Mutex<Option<Child>>>;

pub fn run_auth_command(
    config: &AppConfig,
    handle: &ProcessHandle,
    cancelled: &std::sync::atomic::AtomicBool,
    on_device: impl FnMut(String, String),
) -> Result<(), String> {
    let mut guard = handle
        .lock()
        .map_err(|_| "Authentication process lock failed")?;
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Authentication was cancelled".to_string());
    }
    if guard.is_some() {
        return Err("Authentication is already running".to_string());
    }
    config.validate().map_err(|error| error.to_string())?;
    let mut command = Command::new(server::get_server_exe_path()?);
    command
        .arg("auth")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    server::configure_proxy(&mut command, config);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    *guard = Some(
        command
            .spawn()
            .map_err(|error| format!("Failed to start authentication: {error}"))?,
    );
    drop(guard);
    let result = watch_auth_command(handle, on_device);
    if result.is_err() {
        stop_process(handle);
    }
    result
}

pub fn stop_process(handle: &ProcessHandle) {
    if let Ok(mut guard) = handle.lock() {
        if let Some(mut child) = guard.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

pub fn watch_auth_command(
    handle: &ProcessHandle,
    mut on_device: impl FnMut(String, String),
) -> Result<(), String> {
    let (sender, receiver) = mpsc::sync_channel(64);
    {
        let mut guard = handle
            .lock()
            .map_err(|_| "Authentication process lock failed")?;
        let child = guard.as_mut().ok_or("Authentication was cancelled")?;
        let streams: Vec<Box<dyn std::io::Read + Send>> = child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn std::io::Read + Send>)
            .into_iter()
            .chain(
                child
                    .stderr
                    .take()
                    .map(|stream| Box::new(stream) as Box<dyn std::io::Read + Send>),
            )
            .collect();
        for stream in streams {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
        }
    }
    drop(sender);
    let started = Instant::now();
    let mut code_sent = false;
    loop {
        if started.elapsed() >= Duration::from_secs(930) {
            return Err("Authentication timed out".to_string());
        }
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(line) if !code_sent => {
                if let Some((code, url)) = parse_device_code_line(&line) {
                    on_device(code, url);
                    code_sent = true;
                }
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::park_timeout(Duration::from_millis(20))
            }
        }
        let mut guard = handle
            .lock()
            .map_err(|_| "Authentication process lock failed")?;
        let child = guard.as_mut().ok_or("Authentication was cancelled")?;
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            guard.take();
            return if status.success() {
                Ok(())
            } else {
                Err("GitHub authentication failed or was denied".to_string())
            };
        }
    }
}

pub fn parse_device_code_line(line: &str) -> Option<(String, String)> {
    let lower = line.to_lowercase();
    if !lower.contains("login/device")
        && !lower.contains("device code")
        && !lower.contains("user code")
    {
        return None;
    }
    let url = line
        .find("https://")
        .and_then(|start| line[start..].split_whitespace().next())
        .unwrap_or("https://github.com/login/device");
    let quoted = line.split('"').nth(1);
    let code = quoted.or_else(|| {
        ["code: ", "Code: ", "code:", "Code:"]
            .into_iter()
            .find_map(|prefix| {
                line.split_once(prefix)
                    .and_then(|(_, suffix)| suffix.split_whitespace().next())
            })
    })?;
    if code.len() != 9
        || code.as_bytes()[4] != b'-'
        || !code
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_uppercase() || byte.is_ascii_digit())
        || url != "https://github.com/login/device"
    {
        return None;
    }
    Some((code.to_string(), url.to_string()))
}
