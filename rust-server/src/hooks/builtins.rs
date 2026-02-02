use chrono::{Duration, Local, Utc};
use regex::Regex;
use std::path::PathBuf;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::errors::ApiResult;
use crate::hooks::{claude_paths, types::{HookInput, HookResult}};
use crate::errors::ApiError;

pub fn run_builtin(name: &str, input: &HookInput) -> ApiResult<HookResult> {
    match name {
        "session_start" => session_start(),
        "session_end" => session_end(input),
        "pre_compact" => pre_compact(input),
        "suggest_compact" => suggest_compact(input),
        "evaluate_session" => evaluate_session(input),
        "check_console_log" => check_console_log(),
        "warn_console_log" => warn_console_log(input),
        "block_doc_creation" => block_doc_creation(input),
        "tmux_dev_block" => tmux_dev_block(),
        "tmux_reminder" => tmux_reminder(),
        "git_push_reminder" => git_push_reminder(),
        "pr_create_notice" => pr_create_notice(input),
        _ => Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: format!("[Hook] Unknown builtin: {}", name) }),
    }
}

fn session_start() -> ApiResult<HookResult> {
    let sessions_dir = claude_paths::sessions_dir()?;
    let learned_dir = claude_paths::learned_skills_dir()?;
    std::fs::create_dir_all(&sessions_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create sessions dir: {e}")))?;
    std::fs::create_dir_all(&learned_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create learned dir: {e}")))?;

    let cutoff = Local::now() - Duration::days(7);
    let mut recent = Vec::new();
    for entry in WalkDir::new(&sessions_dir).max_depth(1) {
        let entry = entry.map_err(|e| ApiError::Internal(format!("Failed to read sessions dir: {e}")))?;
        if entry.file_type().is_file() {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    let modified: chrono::DateTime<Local> = modified.into();
                    if modified > cutoff {
                        recent.push(entry.path().to_path_buf());
                    }
                }
            }
        }
    }
    recent.sort();
    let learned_count = WalkDir::new(&learned_dir)
        .max_depth(2)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().map(|e| e == "md").unwrap_or(false))
        .count();

    let mut stderr = String::new();
    if !recent.is_empty() {
        stderr.push_str(&format!("[SessionStart] Found {} recent session(s)\n", recent.len()));
        stderr.push_str(&format!("[SessionStart] Latest: {}\n", recent.last().unwrap().display()));
    }
    if learned_count > 0 {
        stderr.push_str(&format!("[SessionStart] {} learned skill(s) available\n", learned_count));
    }

    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr })
}

fn session_end(input: &HookInput) -> ApiResult<HookResult> {
    let sessions_dir = claude_paths::sessions_dir()?;
    std::fs::create_dir_all(&sessions_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create sessions dir: {e}")))?;
    let session_id = input.resolved_session_id().unwrap_or_else(|| Uuid::new_v4().to_string());
    let short = session_id.chars().take(8).collect::<String>();
    let date = Local::now().format("%Y-%m-%d").to_string();
    let path = sessions_dir.join(format!("{}-{}-session.tmp", date, short));
    let payload = serde_json::json!({
        "session_id": session_id,
        "ended_at": Utc::now().to_rfc3339(),
    });
    std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap_or_default())
        .map_err(|e| ApiError::Internal(format!("Failed to write session file: {e}")))?;

    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: format!("[SessionEnd] Saved {}", path.display()) })
}

fn pre_compact(input: &HookInput) -> ApiResult<HookResult> {
    let sessions_dir = claude_paths::sessions_dir()?;
    std::fs::create_dir_all(&sessions_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create sessions dir: {e}")))?;
    let session_id = input.resolved_session_id().unwrap_or_else(|| Uuid::new_v4().to_string());
    let path = sessions_dir.join(format!("pre-compact-{}.json", session_id));
    let payload = serde_json::json!({
        "session_id": session_id,
        "timestamp": Utc::now().to_rfc3339(),
        "tool": input.tool,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap_or_default())
        .map_err(|e| ApiError::Internal(format!("Failed to write pre-compact file: {e}")))?;
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: format!("[PreCompact] Saved {}", path.display()) })
}

fn suggest_compact(input: &HookInput) -> ApiResult<HookResult> {
    let session_id = input.resolved_session_id().unwrap_or_else(|| "default".to_string());
    let threshold = std::env::var("COMPACT_THRESHOLD").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(50);
    let reminder_every = 25u32;

    let counter_path = std::env::temp_dir().join(format!("claude-tool-count-{}", session_id));
    let current = std::fs::read_to_string(&counter_path).ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let next = current.saturating_add(1);
    let _ = std::fs::write(&counter_path, next.to_string());

    let mut stderr = String::new();
    if next >= threshold && (next - threshold) % reminder_every == 0 {
        stderr.push_str("[Hook] Consider /compact to keep context focused\n");
    }

    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr })
}

fn evaluate_session(input: &HookInput) -> ApiResult<HookResult> {
    let min_len = std::env::var("CLAUDE_MIN_SESSION_MESSAGES").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(8);
    let path = std::env::var("CLAUDE_TRANSCRIPT_PATH").ok().map(PathBuf::from);
    let Some(path) = path else {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: "[Evaluate] No transcript path".to_string() });
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: "[Evaluate] Transcript not readable".to_string() });
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: "[Evaluate] Transcript invalid JSON".to_string() });
    };

    let mut user_messages = 0u32;
    if let Some(messages) = json.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                user_messages += 1;
            }
        }
    }

    if user_messages < min_len {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: "[Evaluate] Session too short".to_string() });
    }

    let learned_dir = claude_paths::learned_skills_dir()?;
    std::fs::create_dir_all(&learned_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create learned dir: {e}")))?;
    let session_id = input.resolved_session_id().unwrap_or_else(|| Uuid::new_v4().to_string());
    let file = learned_dir.join(format!("learned-{}-{}.md", Local::now().format("%Y-%m-%d"), &session_id[..8.min(session_id.len())]));
    let body = format!("# Learned Pattern\n\n- session_id: {}\n- user_messages: {}\n- extracted_at: {}\n", session_id, user_messages, Utc::now().to_rfc3339());
    std::fs::write(&file, body)
        .map_err(|e| ApiError::Internal(format!("Failed to write learned file: {e}")))?;

    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: format!("[Evaluate] Learned pattern saved: {}", file.display()) })
}

fn check_console_log() -> ApiResult<HookResult> {
    let mut stderr = String::new();
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only"]) 
        .output();

    let Ok(output) = output else {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: "[Hook] git not available".to_string() });
    };
    let files = String::from_utf8_lossy(&output.stdout);
    for file in files.lines() {
        if !is_script_file(file) { continue; }
        if let Ok(content) = std::fs::read_to_string(file) {
            if content.contains("console.log") {
                stderr.push_str(&format!("[Hook] console.log found: {}\n", file));
            }
        }
    }

    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr })
}

fn warn_console_log(input: &HookInput) -> ApiResult<HookResult> {
    let path = input.tool_input.as_ref().and_then(|v| v.get("file_path")).and_then(|v| v.as_str()).unwrap_or("");
    if path.is_empty() {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: String::new() });
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: String::new() });
    };
    let mut lines = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if line.contains("console.log") {
            lines.push(format!("{}: {}", idx + 1, line.trim()));
        }
    }
    let mut stderr = String::new();
    if !lines.is_empty() {
        stderr.push_str(&format!("[Hook] WARNING: console.log found in {}\n", path));
        for line in lines.iter().take(5) {
            stderr.push_str(&format!("{}\n", line));
        }
    }
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr })
}

fn block_doc_creation(input: &HookInput) -> ApiResult<HookResult> {
    let path = input.tool_input.as_ref().and_then(|v| v.get("file_path")).and_then(|v| v.as_str()).unwrap_or("");
    let allow = Regex::new(r"(README|CLAUDE|AGENTS|CONTRIBUTING)\.md$").unwrap();
    if (path.ends_with(".md") || path.ends_with(".txt")) && !allow.is_match(path) {
        return Ok(HookResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("[Hook] BLOCKED: Unnecessary documentation file creation: {}", path),
        });
    }
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: String::new() })
}

fn tmux_dev_block() -> ApiResult<HookResult> {
    if std::env::var("TMUX").is_err() {
        return Ok(HookResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "[Hook] BLOCKED: Dev server should run in tmux".to_string(),
        });
    }
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: String::new() })
}

fn tmux_reminder() -> ApiResult<HookResult> {
    if std::env::var("TMUX").is_err() {
        return Ok(HookResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: "[Hook] Consider running in tmux for session persistence".to_string(),
        });
    }
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: String::new() })
}

fn git_push_reminder() -> ApiResult<HookResult> {
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: "[Hook] Review changes before push".to_string() })
}

fn pr_create_notice(input: &HookInput) -> ApiResult<HookResult> {
    let output = input.tool_output.clone().unwrap_or(serde_json::Value::Null);
    let output_text = output.get("output").and_then(|v| v.as_str()).unwrap_or("");
    let re = Regex::new(r"https://github.com/[^/]+/[^/]+/pull/\d+").unwrap();
    if let Some(m) = re.find(output_text) {
        let url = m.as_str();
        return Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: format!("[Hook] PR created: {}", url) });
    }
    Ok(HookResult { exit_code: 0, stdout: String::new(), stderr: String::new() })
}

fn is_script_file(file: &str) -> bool {
    file.ends_with(".js") || file.ends_with(".jsx") || file.ends_with(".ts") || file.ends_with(".tsx")
}
