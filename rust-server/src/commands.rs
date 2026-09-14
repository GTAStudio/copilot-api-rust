use crate::{
    auth_flow::ensure_copilot_token,
    auth_flow::ensure_github_token,
    errors::ApiResult,
    paths::get_paths,
    services::{copilot::get_models, github::get_copilot_usage},
    state::AppState,
    token_store::read_github_token,
};
use dialoguer::Select;

pub async fn run_debug(json: bool) -> ApiResult<()> {
    let version = env!("CARGO_PKG_VERSION");
    let runtime = serde_json::json!({
        "name": "rust",
        "version": std::env::var("RUSTC_VERSION").unwrap_or_else(|_| "unknown".to_string()),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    });

    let paths = get_paths()?;
    let token_exists = read_github_token()
        .await?
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);

    let info = serde_json::json!({
        "version": version,
        "runtime": runtime,
        "paths": {
            "APP_DIR": paths.app_dir.to_string_lossy(),
            "GITHUB_TOKEN_PATH": paths.github_token_path.to_string_lossy(),
        },
        "tokenExists": token_exists,
    });

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "copilot-api-rs debug\n\nVersion: {}\nRuntime: {} {} ({} {})\n\nPaths:\n- APP_DIR: {}\n- GITHUB_TOKEN_PATH: {}\n\nToken exists: {}",
            version,
            info["runtime"]["name"].as_str().unwrap_or("rust"),
            info["runtime"]["version"].as_str().unwrap_or("unknown"),
            info["runtime"]["platform"].as_str().unwrap_or("unknown"),
            info["runtime"]["arch"].as_str().unwrap_or("unknown"),
            info["paths"]["APP_DIR"].as_str().unwrap_or(""),
            info["paths"]["GITHUB_TOKEN_PATH"].as_str().unwrap_or(""),
            if token_exists { "Yes" } else { "No" },
        );
    }

    Ok(())
}

pub async fn run_check_usage(state: &AppState) -> ApiResult<()> {
    let github_token = ensure_github_token(state).await?;
    let config = state.config.read().await.clone();
    let usage = get_copilot_usage(&state.client, &config, &github_token).await?;

    let plan = usage
        .get("copilot_plan")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let reset = usage
        .get("quota_reset_date")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let snapshots = usage.get("quota_snapshots").and_then(|v| v.as_object());

    let format_quota = |name: &str| -> String {
        if let Some(map) = snapshots
            .and_then(|s| s.get(name))
            .and_then(|v| v.as_object())
        {
            let entitlement = map
                .get("entitlement")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let remaining = map.get("remaining").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let percent_remaining = map
                .get("percent_remaining")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let used = entitlement - remaining;
            let percent_used = if entitlement > 0.0 {
                (used / entitlement) * 100.0
            } else {
                0.0
            };
            return format!(
                "{}: {}/{} used ({:.1}% used, {:.1}% remaining)",
                name,
                used.round(),
                entitlement.round(),
                percent_used,
                percent_remaining,
            );
        }
        format!("{}: N/A", name)
    };

    let premium = format_quota("premium_interactions");
    let chat = format_quota("chat");
    let completions = format_quota("completions");

    println!(
        "Copilot Usage (plan: {})\nQuota resets: {}\n\nQuotas:\n  {}\n  {}\n  {}",
        plan, reset, premium, chat, completions
    );

    Ok(())
}

pub async fn run_claude_code_helper(state: &AppState, server_url: &str) -> ApiResult<()> {
    let token = ensure_copilot_token(state).await?;

    if state.config.read().await.models.is_none() {
        let config_snapshot = state.config.read().await.clone();
        let models = get_models(&state.client, &config_snapshot, &token).await?;
        state.config.write().await.models = Some(models);
    }

    let models = state.config.read().await.models.clone().unwrap();
    let model_ids: Vec<String> = models
        .data
        .iter()
        .filter(|model| model.id.starts_with("claude-"))
        .map(|model| model.id.clone())
        .collect();

    if model_ids.is_empty() {
        println!("No models available for Claude Code helper.");
        return Ok(());
    }

    let selected = Select::new()
        .with_prompt("Select a model to use with Claude Code")
        .items(&model_ids)
        .default(0)
        .interact()
        .unwrap_or(0);

    let selected_small = Select::new()
        .with_prompt("Select a small model to use with Claude Code")
        .items(&model_ids)
        .default(selected)
        .interact()
        .unwrap_or(selected);

    let model = &model_ids[selected];
    let small_model = &model_ids[selected_small];

    let (bash_cmd, ps_cmd) = claude_environment_commands(
        server_url,
        model,
        small_model,
        state.config.read().await.api_key.is_some(),
    );

    if std::env::var("COPILOT_CLIPBOARD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        && let Ok(mut clipboard) = arboard::Clipboard::new()
    {
        let _ = clipboard.set_text(ps_cmd.clone());
    }

    println!("\nClaude Code environment (bash/zsh):\n{}", bash_cmd);
    println!("Claude Code environment (PowerShell):\n{}", ps_cmd);

    Ok(())
}

fn claude_environment_commands(
    server_url: &str,
    model: &str,
    small_model: &str,
    authenticated: bool,
) -> (String, String) {
    let values = [
        ("ANTHROPIC_BASE_URL", server_url),
        ("ANTHROPIC_MODEL", model),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", small_model),
        ("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "1"),
    ];
    let mut bash = values
        .iter()
        .map(|(name, value)| format!("export {name}='{}'", value.replace('\'', "'\"'\"'")))
        .collect::<Vec<_>>()
        .join("\n");
    let mut powershell = values
        .iter()
        .map(|(name, value)| format!("$env:{name}='{}'", value.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join("\n");
    if authenticated {
        bash.push_str("\nexport ANTHROPIC_AUTH_TOKEN=\"$COPILOT_API_KEY\"");
        powershell.push_str("\n$env:ANTHROPIC_AUTH_TOKEN=$env:COPILOT_API_KEY");
    } else {
        bash.push_str("\nexport ANTHROPIC_AUTH_TOKEN='local-only'");
        powershell.push_str("\n$env:ANTHROPIC_AUTH_TOKEN='local-only'");
    }
    bash.push_str("\nclaude\n");
    powershell.push_str("\nclaude\n");
    (bash, powershell)
}

#[cfg(test)]
mod tests {
    #[test]
    fn helper_commands_quote_untrusted_model_names() {
        let (bash, powershell) = super::claude_environment_commands(
            "http://127.0.0.1:4141",
            "claude-$(fixture)'value",
            "claude-haiku-4-5",
            true,
        );
        assert!(powershell.contains("'claude-$(fixture)''value'"));
        assert!(powershell.contains("$env:ANTHROPIC_AUTH_TOKEN=$env:COPILOT_API_KEY"));
        assert!(bash.contains("'claude-$(fixture)'\"'\"'value'"));
        assert!(!powershell.contains("dummy"));
    }
}
