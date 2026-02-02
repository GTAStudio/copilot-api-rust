use std::path::{Path, PathBuf};

pub fn hooks_config_path() -> PathBuf {
    if let Some(path) = find_hooks_config(std::env::current_dir().ok().as_deref()) {
        return path;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(path) = find_hooks_config(exe.parent()) {
            return path;
        }
    }
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        return home.join(".claude").join("hooks").join("hooks.json");
    }
    PathBuf::from(".claude/hooks/hooks.json")
}

pub fn hooks_config_path_string() -> String {
    hooks_config_path().to_string_lossy().to_string()
}

fn find_hooks_config(start: Option<&Path>) -> Option<PathBuf> {
    let mut current = start?.to_path_buf();
    for _ in 0..8 {
        let candidate = current.join(".claude").join("hooks").join("hooks.json");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}
