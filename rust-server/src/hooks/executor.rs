use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

use crate::errors::{ApiError, ApiResult};
use crate::hooks::{
    builtins,
    matcher::evaluator,
    observe,
    types::{HookInput, HookResult, HooksJson},
};

#[derive(Debug, Clone)]
pub struct HookExecutor {
    pub config: HooksJson,
    pub observer: Option<observe::ObservationHub>,
}

impl HookExecutor {
    pub async fn post_tool_use(
        &self,
        tool: &str,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
    ) {
        let input = HookInput {
            hook_type: Some("PostToolUse".to_string()),
            tool: Some(tool.to_string()),
            tool_input: Some(input),
            tool_output: output,
            session_id: None,
        };
        if self.execute_event("PostToolUse", &input).await.is_err() {
            tracing::warn!("PostToolUse hook failed");
        }
    }

    pub fn load(
        config_path: Option<PathBuf>,
        observer: Option<observe::ObservationHub>,
    ) -> ApiResult<Self> {
        let path = resolve_hooks_path(config_path)?;
        let config = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| ApiError::Internal(format!("Failed to read hooks.json: {e}")))?;
            serde_json::from_str::<HooksJson>(&content)
                .map_err(|e| ApiError::Internal(format!("Invalid hooks.json: {e}")))?
        } else {
            HooksJson::default()
        };

        Ok(Self { config, observer })
    }

    pub async fn execute_event(
        &self,
        event: &str,
        input: &HookInput,
    ) -> ApiResult<Vec<HookResult>> {
        input.validate()?;
        if event.is_empty()
            || event.len() > 128
            || !event.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(ApiError::BadRequest("Invalid hook event name".to_string()));
        }
        if let Some(observer) = &self.observer {
            observer.emit(observe::build_event(event, input));
        }

        let mut results = Vec::new();
        if let Some(entries) = self.config.hooks.get(event) {
            for config in entries {
                let matched = evaluator::evaluate(&config.matcher, input)
                    .map_err(|_| ApiError::BadRequest("Invalid hook matcher".to_string()))?;
                if !matched {
                    continue;
                }
                for hook in &config.hooks {
                    if !hook.enabled {
                        continue;
                    }
                    let result = match hook.hook_type.as_str() {
                        "builtin" => {
                            let name = hook.name.clone().unwrap_or_else(|| "unknown".to_string());
                            let input = input.clone();
                            tokio::task::spawn_blocking(move || {
                                builtins::run_builtin(&name, &input)
                            })
                            .await
                            .map_err(|_| {
                                ApiError::Internal("Builtin hook execution failed".to_string())
                            })??
                        }
                        "command" => {
                            if std::env::var("COPILOT_ALLOW_COMMAND_HOOKS").as_deref() != Ok("1") {
                                return Err(ApiError::Forbidden(
                                    "External command hooks require COPILOT_ALLOW_COMMAND_HOOKS=1"
                                        .to_string(),
                                ));
                            }
                            let command = hook.command.clone().unwrap_or_default();
                            run_command(&command, input, hook.timeout).await?
                        }
                        _ => return Err(ApiError::BadRequest("Unknown hook type".to_string())),
                    };
                    results.push(result);
                }
            }
        }
        Ok(results)
    }
}

fn resolve_hooks_path(explicit: Option<PathBuf>) -> ApiResult<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Ok(path) = std::env::var("CLAUDE_HOOKS_PATH") {
        return Ok(PathBuf::from(path));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project = cwd.join(".claude").join("hooks").join("hooks.json");
    if project.exists() {
        return Ok(project);
    }
    Ok(crate::hooks::claude_paths::hooks_dir()?.join("hooks.json"))
}

async fn run_command(
    command: &str,
    input: &HookInput,
    timeout: Option<u64>,
) -> ApiResult<HookResult> {
    if command.trim().is_empty()
        || command.len() > 8192
        || timeout.is_some_and(|seconds| seconds == 0 || seconds > 300)
    {
        return Err(ApiError::BadRequest(
            "Invalid hook command or timeout".to_string(),
        ));
    }
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{command}\""));
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    };
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut cmd = process_wrap::tokio::CommandWrap::from(cmd);
    cmd.wrap(process_wrap::tokio::KillOnDrop);
    #[cfg(windows)]
    {
        cmd.wrap(process_wrap::tokio::CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        cmd.wrap(process_wrap::tokio::JobObject);
    }
    #[cfg(unix)]
    cmd.wrap(process_wrap::tokio::ProcessGroup::leader());

    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::Internal(format!("Failed to spawn hook command: {e}")))?;
    let stdin = child
        .stdin()
        .take()
        .ok_or_else(|| ApiError::Internal("Hook stdin unavailable".to_string()))?;
    let stdout = child
        .stdout()
        .take()
        .ok_or_else(|| ApiError::Internal("Hook stdout unavailable".to_string()))?;
    let stderr = child
        .stderr()
        .take()
        .ok_or_else(|| ApiError::Internal("Hook stderr unavailable".to_string()))?;
    let data = serde_json::to_vec(input)
        .map_err(|_| ApiError::Internal("Invalid hook input".to_string()))?;
    let execution = async {
        let write_input = async {
            let mut stdin = stdin;
            stdin.write_all(&data).await?;
            stdin.shutdown().await
        };
        let (_, stdout, stderr, status) = tokio::try_join!(
            write_input,
            bounded_output(stdout),
            bounded_output(stderr),
            child.wait()
        )?;
        Ok::<_, std::io::Error>((stdout, stderr, status))
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout.unwrap_or(30)),
        execution,
    )
    .await;
    let (stdout, stderr, status) = match result {
        Ok(Ok(output)) => output,
        _ => {
            let _ = std::pin::Pin::from(child.kill()).await;
            let _ = child.wait().await;
            return Err(ApiError::Internal(
                "Hook command failed, timed out, or exceeded output limits".to_string(),
            ));
        }
    };
    Ok(HookResult {
        exit_code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
    })
}

async fn bounded_output(stream: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    stream.take(1_048_577).read_to_end(&mut bytes).await?;
    if bytes.len() > 1_048_576 {
        return Err(std::io::Error::other("Hook output limit exceeded"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timed_out_command_reaps_its_descendant_process() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("handshake listener");
        let endpoint = listener
            .local_addr()
            .expect("handshake endpoint")
            .to_string();
        let input = HookInput {
            tool_input: Some(serde_json::json!({"endpoint": endpoint})),
            ..Default::default()
        };
        let executable = std::env::current_exe().expect("test executable");
        let command = format!(
            "\"{}\" --exact hooks::executor::tests::waiting_command_fixture --nocapture",
            executable.display()
        );
        let execution = tokio::spawn(async move { run_command(&command, &input, Some(3)).await });
        let (mut connection, _) = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            listener.accept(),
        )
        .await
        {
            Ok(connection) => connection.expect("accept fixture"),
            Err(_) => panic!(
                "fixture did not connect; hook result: {:?}",
                execution.await.expect("hook worker")
            ),
        };
        let mut ready = [0u8; 1];
        connection
            .read_exact(&mut ready)
            .await
            .expect("fixture ready");
        assert_eq!(ready, [1]);
        let result = execution.await.expect("hook worker");
        assert!(result.is_err());
        let mut closed = [0u8; 1];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            connection.read(&mut closed),
        )
        .await;
        drop(connection);
        assert!(
            matches!(read, Ok(Ok(0)) | Ok(Err(_))),
            "timed-out hook left its descendant running"
        );
    }

    #[test]
    fn waiting_command_fixture() {
        if !std::env::args().any(|argument| argument == "--exact") {
            return;
        }
        use std::io::{Read, Write};
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .expect("hook input");
        if input.is_empty() {
            return;
        }
        let input: HookInput = serde_json::from_str(&input).expect("hook JSON");
        let endpoint = input
            .tool_input
            .as_ref()
            .and_then(|input| input["endpoint"].as_str())
            .expect("fixture endpoint");
        let mut connection = std::net::TcpStream::connect(endpoint).expect("fixture handshake");
        connection
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("fixture bound");
        connection.write_all(&[1]).expect("fixture ready");
        let mut done = [0u8; 1];
        let _ = connection.read(&mut done);
    }

    #[tokio::test]
    async fn command_hooks_capture_both_streams_and_exit_status() {
        let command = if cfg!(windows) {
            "echo hook-output & echo hook-error 1>&2 & exit /b 7"
        } else {
            "printf hook-output; printf hook-error >&2; exit 7"
        };
        let output = run_command(command, &HookInput::default(), Some(5))
            .await
            .expect("bounded command");
        assert_eq!(output.exit_code, 7);
        assert!(output.stdout.contains("hook-output"));
        assert!(output.stderr.contains("hook-error"));
        for (command, timeout) in [
            ("", None),
            ("echo fixture", Some(0)),
            ("echo fixture", Some(301)),
        ] {
            assert!(
                run_command(command, &HookInput::default(), timeout)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn command_output_limits_are_enforced() {
        let valid = std::io::Cursor::new(vec![b'x'; 1024]);
        assert_eq!(
            bounded_output(valid).await.expect("valid output").len(),
            1024
        );
        let excessive = std::io::Cursor::new(vec![b'x'; 1_048_577]);
        assert!(bounded_output(excessive).await.is_err());
    }

    #[tokio::test]
    async fn executor_loads_configuration_and_skips_disabled_or_unmatched_hooks() {
        let directory = tempfile::tempdir().expect("hook configuration directory");
        let path = directory.path().join("hooks.json");
        assert!(
            HookExecutor::load(Some(path.clone()), None)
                .expect("missing config")
                .config
                .hooks
                .is_empty()
        );
        std::fs::write(&path, "{broken").expect("invalid config");
        assert!(HookExecutor::load(Some(path.clone()), None).is_err());
        let config = serde_json::json!({"hooks": {"PreToolUse": [
            {"matcher": "Write", "hooks": [{"type": "unsupported"}]},
            {"matcher": "Read", "hooks": [{"type": "unsupported", "enabled": false}, {"type": "builtin", "name": "git_push_reminder"}]}
        ]}});
        std::fs::write(&path, config.to_string()).expect("valid configuration");
        let executor = HookExecutor::load(Some(path), None).expect("configuration");
        let input = HookInput {
            tool: Some("Read".to_string()),
            ..Default::default()
        };
        let results = executor
            .execute_event("PreToolUse", &input)
            .await
            .expect("execute builtin");
        assert_eq!(results.len(), 1);
        assert!(results[0].stderr.contains("Review changes"));
        assert!(
            executor
                .execute_event("not-an-event", &input)
                .await
                .is_err()
        );
        assert!(
            executor
                .execute_event("Stop", &input)
                .await
                .expect("absent event")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn malformed_matchers_and_unknown_hooks_fail_closed() {
        for (matcher, hook) in [
            (
                "tool ==",
                serde_json::json!({"type": "builtin", "name": "git_push_reminder"}),
            ),
            ("*", serde_json::json!({"type": "unsupported"})),
            (
                "*",
                serde_json::json!({"type": "builtin", "name": "missing"}),
            ),
        ] {
            let config = serde_json::from_value(serde_json::json!({"hooks": {"PreToolUse": [{"matcher": matcher, "hooks": [hook]}]}})).expect("hook config");
            let executor = HookExecutor {
                config,
                observer: None,
            };
            assert!(
                executor
                    .execute_event("PreToolUse", &HookInput::default())
                    .await
                    .is_err()
            );
        }
    }
}
