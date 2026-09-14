use serde_json::{json, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn configure_claude_code(config: &crate::config::AppConfig) -> io::Result<String> {
    config.validate()?;
    let base = config.effective_claude_base_url();
    validate_base_url(&base)?;
    let path = claude_dir()?.join("settings.json");
    let mut doc = load_json_or_object(&path)?;
    let env = settings_env(&mut doc)?;
    env.insert("ANTHROPIC_BASE_URL".to_string(), base.into());
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".to_string(),
        std::env::var("COPILOT_API_KEY")
            .unwrap_or_else(|_| "local-only".to_string())
            .into(),
    );
    env.remove("ANTHROPIC_API_KEY");
    env.insert(
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".to_string(),
        "1".into(),
    );
    for (name, model) in [
        ("ANTHROPIC_MODEL", &config.main_model),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &config.fast_model),
    ] {
        if !model.trim().is_empty() {
            env.insert(name.to_string(), model.trim().into());
        } else {
            env.remove(name);
        }
    }
    write_json_atomic(&path, &doc)?;
    Ok("Claude Code gateway settings updated".to_string())
}

fn claude_dir() -> io::Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No home directory"))?;
    let path = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| base.home_dir().join(".claude"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn validate_base_url(base: &str) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid Claude gateway URL");
    let url = url::Url::parse(base).map_err(|_| invalid())?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(())
}

fn settings_env(doc: &mut Value) -> io::Result<&mut serde_json::Map<String, Value>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Existing Claude settings or env is not an object",
        )
    };
    doc.as_object_mut()
        .ok_or_else(invalid)?
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(invalid)
}

#[cfg(test)]
fn ensure_settings_base_url(path: &Path, base_url: &str) -> io::Result<bool> {
    validate_base_url(base_url)?;
    let mut doc = load_json_or_object(path)?;
    let env = settings_env(&mut doc)?;
    let changed = env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str) != Some(base_url);
    if changed {
        env.insert("ANTHROPIC_BASE_URL".to_string(), base_url.into());
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
    let data = serde_json::to_string_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    crate::config::write_atomic(path, data.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_audit_claude_settings_preserve_unrelated_values() {
        let directory = tempfile::tempdir().expect("test directory");
        let path = directory.path().join("settings.json");
        let original = json!({"permissions": {"deny": ["Read(private)"]}, "env": {"EXISTING_SETTING": "kept"}});
        fs::write(&path, original.to_string()).expect("original settings");
        assert!(ensure_settings_base_url(&path, "http://127.0.0.1:5050").expect("merge"));
        let updated = load_json_or_object(&path).expect("updated settings");
        assert_eq!(updated["permissions"], original["permissions"]);
        assert_eq!(updated["env"]["EXISTING_SETTING"], "kept");
        assert_eq!(
            updated["env"]["ANTHROPIC_BASE_URL"],
            "http://127.0.0.1:5050"
        );
        assert!(!ensure_settings_base_url(&path, "http://127.0.0.1:5050").expect("idempotent"));
    }

    #[test]
    fn gui_audit_claude_settings_reject_invalid_existing_shapes() {
        let directory = tempfile::tempdir().expect("test directory");
        let path = directory.path().join("settings.json");
        for original in ["[]", "{\"env\":false}", "{broken"] {
            fs::write(&path, original).expect("original settings");
            assert!(ensure_settings_base_url(&path, "http://127.0.0.1:5050").is_err());
            assert_eq!(
                fs::read_to_string(&path).expect("unchanged settings"),
                original
            );
        }
    }
}
