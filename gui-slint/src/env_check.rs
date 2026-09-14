use std::{
    io::{self, Read},
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct DependencyReport {
    pub summary: String,
    pub details: String,
    #[allow(dead_code)]
    pub missing: Vec<String>,
}

struct CheckProcess(Box<dyn process_wrap::std::ChildWrapper>);

impl Drop for CheckProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn run_bounded(mut command: Command, timeout: Duration) -> io::Result<Output> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid check timeout"))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = process_wrap::std::CommandWrap::from(command);
    #[cfg(windows)]
    {
        command.wrap(process_wrap::std::CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        command.wrap(process_wrap::std::JobObject);
    }
    #[cfg(unix)]
    command.wrap(process_wrap::std::ProcessGroup::leader());
    let mut child = CheckProcess(command.spawn()?);
    let stdout: Box<dyn Read + Send> = Box::new(
        child
            .0
            .stdout()
            .take()
            .ok_or_else(|| io::Error::other("Missing check stdout"))?,
    );
    let stderr: Box<dyn Read + Send> = Box::new(
        child
            .0
            .stderr()
            .take()
            .ok_or_else(|| io::Error::other("Missing check stderr"))?,
    );
    let (sender, receiver) = mpsc::sync_channel(2);
    for (index, stream) in [stdout, stderr].into_iter().enumerate() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.take(65_537).read_to_end(&mut bytes).and_then(|_| {
                if bytes.len() > 65_536 {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Dependency check output exceeds limit",
                    ))
                } else {
                    Ok(bytes)
                }
            });
            let _ = sender.send((index, result));
        });
    }
    drop(sender);
    let mut streams = [None, None];
    loop {
        let status = child.0.try_wait()?;
        if let Some(status) = status {
            if streams.iter().all(Option::is_some) {
                return Ok(Output {
                    status,
                    stdout: streams[0].take().unwrap_or_default(),
                    stderr: streams[1].take().unwrap_or_default(),
                });
            }
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Dependency check timed out"))?;
        match receiver.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok((index, bytes)) => streams[index] = Some(bytes?),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::park_timeout(remaining.min(Duration::from_millis(20)))
            }
        }
    }
}

fn run_output(cmd: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(cmd);
    command.args(args);
    let output = run_bounded(command, Duration::from_secs(3)).ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn check_vscode_extensions(list: &str, exts: &[&str]) -> (bool, Vec<String>) {
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
        let found = installed
            .iter()
            .any(|installed| installed.split('@').next() == Some(ext_lc.as_str()));
        if !found {
            missing.push(ext.to_string());
        }
    }

    (missing.is_empty(), missing)
}

pub fn check_all() -> DependencyReport {
    check_with(run_output, cfg!(embedded_server))
}

fn check_with(run: impl Fn(&str, &[&str]) -> Option<String>, embedded: bool) -> DependencyReport {
    let missing = Vec::new();
    let mut lines = Vec::new();
    let command_exists =
        |name| run(if cfg!(windows) { "where" } else { "which" }, &[name]).is_some();
    let vscode_ok = command_exists("code");
    if vscode_ok {
        let ver = run("code", &["--version"]).unwrap_or_default();
        let ver_line: String = ver
            .lines()
            .next()
            .unwrap_or("OK")
            .chars()
            .take(30)
            .collect();
        lines.push(format!("VS Code: [OK] {}", ver_line));
    } else {
        lines.push("VS Code: [X] Missing (optional)".to_string());
    }

    let extensions = ["github.copilot-chat", "anthropic.claude-code"];
    if vscode_ok {
        let list = run("code", &["--list-extensions"]).unwrap_or_default();
        let (ok, missing_exts) = check_vscode_extensions(&list, &extensions);
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

    let claude_ok = command_exists("claude");
    if claude_ok {
        lines.push("Claude CLI: [OK]".to_string());
    } else {
        lines.push("Claude CLI: [X] Missing (optional, for Claude Code)".to_string());
    }

    lines.push(
        if embedded {
            "Copilot API Server: [OK] Embedded"
        } else {
            "Copilot API Server: External executable required"
        }
        .to_string(),
    );

    let summary = if embedded {
        "[OK] Ready to use"
    } else {
        "Build server or place it beside the GUI executable"
    }
    .to_string();

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn fixture_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "env_check::tests::dependency_process_fixture",
                "--nocapture",
            ])
            .env("COPILOT_DEPENDENCY_FIXTURE", mode);
        command
    }

    #[test]
    fn dependency_process_output_and_runtime_are_bounded() {
        let output =
            run_bounded(fixture_command("normal"), Duration::from_secs(3)).expect("normal check");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("fixture stdout"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("fixture stderr"));
        let failed = run_bounded(fixture_command("failure"), Duration::from_secs(3))
            .expect("failed process status");
        assert!(!failed.status.success());
        let started = Instant::now();
        assert_eq!(
            run_bounded(fixture_command("timeout"), Duration::from_millis(400))
                .expect_err("bounded runtime")
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            run_bounded(fixture_command("large"), Duration::from_secs(3))
                .expect_err("bounded output")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(run_bounded(
            Command::new("nonexistent-unit-test-command-12345"),
            Duration::from_secs(1)
        )
        .is_err());
    }

    #[test]
    fn dependency_reports_distinguish_optional_tools_and_exact_extensions() {
        for (vscode, claude, embedded, extensions, valid_extensions) in [
            (false, false, false, "", false),
            (
                true,
                true,
                true,
                "GitHub.Copilot-Chat@1.0\nAnthropic.Claude-Code",
                true,
            ),
            (
                true,
                false,
                true,
                "github.copilot-chat-fake\nanthropic.claude-code-old",
                false,
            ),
            (true, true, false, "", false),
        ] {
            let calls = std::cell::RefCell::new(Vec::new());
            let report = check_with(
                |program, arguments| {
                    calls
                        .borrow_mut()
                        .push((program.to_string(), arguments.join(" ")));
                    match arguments {
                        ["code"] => vscode.then(|| "fixture code executable".to_string()),
                        ["claude"] => claude.then(|| "fixture Claude executable".to_string()),
                        ["--version"] => Some("1.104.3\nfixture hash".to_string()),
                        ["--list-extensions"] => Some(extensions.to_string()),
                        _ => panic!("Unexpected system request: {program} {arguments:?}"),
                    }
                },
                embedded,
            );
            assert_eq!(report.details.contains("VS Code: [OK]"), vscode);
            assert_eq!(report.details.contains("Claude CLI: [OK]"), claude);
            assert_eq!(
                report.details.contains("Extensions: [OK]"),
                valid_extensions
            );
            assert_eq!(report.summary.contains("Ready to use"), embedded);
            assert_eq!(
                calls
                    .borrow()
                    .iter()
                    .any(|(_, args)| args == "--list-extensions"),
                vscode
            );
            if vscode {
                assert!(report.details.contains("1.104.3"));
            }
        }
    }

    #[test]
    fn dependency_process_fixture() {
        let Ok(mode) = std::env::var("COPILOT_DEPENDENCY_FIXTURE") else {
            return;
        };
        match mode.as_str() {
            "normal" => {
                println!("fixture stdout");
                eprintln!("fixture stderr");
            }
            "failure" => panic!("fixture nonzero exit"),
            "timeout" => std::thread::park_timeout(Duration::from_secs(10)),
            "large" => println!("{}", "fixture".repeat(20_000)),
            _ => panic!("invalid fixture mode"),
        }
    }
}
