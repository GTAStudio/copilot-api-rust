use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone)]
pub struct DependencyReport {
    pub summary: String,
    pub details: String,
    #[allow(dead_code)]
    pub missing: Vec<String>,
}

fn command_exists(cmd: &str) -> bool {
    let mut c = if cfg!(windows) {
        let mut c = Command::new("where");
        c.arg(cmd);
        #[cfg(windows)]
        c.creation_flags(CREATE_NO_WINDOW);
        c
    } else {
        let mut c = Command::new("which");
        c.arg(cmd);
        c
    };

    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn get_version(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    
    let out = c.output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = if stdout.trim().is_empty() { stderr } else { stdout };
    // Extract version number pattern
    let line = text.lines().next().unwrap_or("").trim();
    if line.len() > 30 {
        Some(line[..30].to_string())
    } else {
        Some(line.to_string())
    }
}

fn run_output(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn check_vscode_extensions(exts: &[&str]) -> (bool, Vec<String>) {
    let list = run_output("code", &["--list-extensions"]).unwrap_or_default();
    if list.is_empty() {
        return (false, exts.iter().map(|s| s.to_string()).collect());
    }
    let installed: Vec<String> = list
        .lines()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let mut missing = Vec::new();
    for ext in exts {
        let ext_lc = ext.to_lowercase();
        // Use starts_with to handle version suffixes and partial matches
        let found = installed.iter().any(|i| i.starts_with(&ext_lc) || ext_lc.starts_with(i));
        if !found {
            missing.push(ext.to_string());
        }
    }

    (missing.is_empty(), missing)
}

pub fn check_all() -> DependencyReport {
    let missing = Vec::new(); // Kept for struct compatibility
    let mut lines = Vec::new();

    // VS Code is optional but helpful
    let vscode_ok = command_exists("code");
    if vscode_ok {
        let ver = get_version("code", &["--version"]).unwrap_or_default();
        let ver_line = ver.lines().next().unwrap_or("OK");
        lines.push(format!("VS Code: [OK] {}", ver_line));
    } else {
        lines.push("VS Code: [X] Missing (optional)".to_string());
    }

    // Extensions are optional
    let extensions = ["github.copilot-chat", "joouis.agent-maestro"]; 
    if vscode_ok {
        let (ok, missing_exts) = check_vscode_extensions(&extensions);
        if ok {
            lines.push("Extensions: [OK]".to_string());
        } else {
            lines.push(format!(
                "Extensions: [X] Missing {} (optional)",
                missing_exts.join(", ")
            ));
        }
    } else {
        lines.push("Extensions: [-] Skipped".to_string());
    }

    // Claude CLI is optional
    let claude_ok = command_exists("claude");
    if claude_ok {
        lines.push("Claude CLI: [OK]".to_string());
    } else {
        lines.push("Claude CLI: [X] Missing (optional, for Claude Code)".to_string());
    }

    // Server is embedded - no Bun/Node needed!
    lines.push("Copilot API Server: [OK] Embedded".to_string());

    let summary = "[OK] Ready to use (server embedded)".to_string();

    DependencyReport {
        summary,
        details: lines.join("\n"),
        missing,
    }
}

pub fn install_missing(_report: &DependencyReport) -> String {
    // Server is embedded - no external dependencies required.
    "No dependencies needed (server embedded).".to_string()
}
