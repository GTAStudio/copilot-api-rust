use serde_json::{json, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn ensure_claude_files(base_url: &str) -> io::Result<String> {
    let claude_dir = claude_dir()?;
    let mut updates = Vec::new();

    let onboarding_path = claude_dir.join(".claude.json");
    if ensure_onboarding(&onboarding_path)? {
        updates.push(".claude.json updated");
    }

    let settings_path = claude_dir.join("settings.json");
    if ensure_settings_base_url(&settings_path, base_url)? {
        updates.push("settings.json base URL updated");
    }

    if updates.is_empty() {
        Ok("Claude files already up to date".to_string())
    } else {
        Ok(updates.join(", "))
    }
}

fn claude_dir() -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No home directory"))?;
    let path = base.home_dir().join(".claude");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn ensure_onboarding(path: &Path) -> io::Result<bool> {
    let mut doc = load_json_or_object(path)?;
    let mut changed = false;

    if let Some(obj) = doc.as_object_mut() {
        if obj.get("hasCompletedOnboarding") != Some(&Value::Bool(true)) {
            obj.insert("hasCompletedOnboarding".to_string(), Value::Bool(true));
            changed = true;
        }
    } else {
        doc = json!({ "hasCompletedOnboarding": true });
        changed = true;
    }

    if changed {
        write_json_atomic(path, &doc)?;
    }
    Ok(changed)
}

fn ensure_settings_base_url(path: &Path, base_url: &str) -> io::Result<bool> {
    let mut doc = load_json_or_object(path)?;
    let mut changed = false;

    if !doc.is_object() {
        doc = json!({});
        changed = true;
    }

    let env = doc
        .as_object_mut()
        .and_then(|obj| obj.entry("env").or_insert_with(|| json!({})).as_object_mut());

    if let Some(env_obj) = env {
        let current = env_obj.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str());
        if current != Some(base_url) {
            env_obj.insert("ANTHROPIC_BASE_URL".to_string(), Value::String(base_url.to_string()));
            changed = true;
        }
    }

    if changed {
        write_json_atomic(path, &doc)?;
    }
    Ok(changed)
}

fn load_json_or_object(path: &Path) -> io::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let data = fs::read_to_string(path)?;
    serde_json::from_str::<Value>(&data)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn write_json_atomic(path: &Path, value: &Value) -> io::Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    let data = serde_json::to_string_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(&tmp_path, data)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}
