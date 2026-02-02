use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

use crate::errors::{ApiError, ApiResult};
use crate::hooks::{builtins, matcher::evaluator, observe, types::{HookInput, HookResult, HooksJson}};

#[derive(Debug, Clone)]
pub struct HookExecutor {
    pub config: HooksJson,
    pub observer: Option<observe::ObservationHub>,
}

impl HookExecutor {
    pub fn load(config_path: Option<PathBuf>, observer: Option<observe::ObservationHub>) -> ApiResult<Self> {
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

    pub async fn execute_event(&self, event: &str, input: &HookInput) -> ApiResult<Vec<HookResult>> {
        if let Some(observer) = &self.observer {
            observer.emit(observe::build_event(event, input));
        }

        let mut results = Vec::new();
        if let Some(entries) = self.config.hooks.get(event) {
            for config in entries {
                let matched = evaluator::evaluate(&config.matcher, input)
                    .unwrap_or(false);
                if !matched {
                    continue;
                }
                for hook in &config.hooks {
                    if !hook.enabled {
                        continue;
                    }
                    let result = match hook.hook_type.as_str() {
                        "builtin" => {
                            let name = hook.name.as_deref().unwrap_or("unknown");
                            builtins::run_builtin(name, input)?
                        }
                        "command" => {
                            let command = hook.command.clone().unwrap_or_default();
                            run_command(&command, input, hook.timeout).await?
                        }
                        _ => HookResult { exit_code: 0, stdout: String::new(), stderr: format!("[Hook] Unknown hook type: {}", hook.hook_type) },
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

async fn run_command(command: &str, input: &HookInput, timeout: Option<u64>) -> ApiResult<HookResult> {
    let mut cmd = if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    };
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| ApiError::Internal(format!("Failed to spawn hook command: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let data = serde_json::to_vec(input).unwrap_or_default();
        stdin.write_all(&data).await.ok();
    }

    let output = if let Some(secs) = timeout {
        tokio::time::timeout(std::time::Duration::from_secs(secs), child.wait_with_output())
            .await
            .map_err(|_| ApiError::Internal("Hook command timeout".to_string()))?
            .map_err(|e| ApiError::Internal(format!("Hook command failed: {e}")))?
    } else {
        child.wait_with_output().await.map_err(|e| ApiError::Internal(format!("Hook command failed: {e}")))?
    };

    Ok(HookResult {
        exit_code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}
