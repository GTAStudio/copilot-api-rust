use crate::{
    auth_flow::ensure_github_token,
    auth_flow::ensure_copilot_token,
    errors::ApiResult,
    paths::get_paths,
    services::{github::get_copilot_usage, copilot::get_models},
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
    let token_exists = read_github_token().await?.map(|t| !t.trim().is_empty()).unwrap_or(false);

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
        println!("{}", serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".to_string()));
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
        if let Some(map) = snapshots.and_then(|s| s.get(name)).and_then(|v| v.as_object()) {
            let entitlement = map.get("entitlement").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let remaining = map.get("remaining").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let percent_remaining = map.get("percent_remaining").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let used = entitlement - remaining;
            let percent_used = if entitlement > 0.0 { (used / entitlement) * 100.0 } else { 0.0 };
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
    let model_ids: Vec<String> = models.data.iter().map(|m| m.id.clone()).collect();

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

    let envs = vec![
        ("ANTHROPIC_BASE_URL", server_url.to_string()),
        ("ANTHROPIC_AUTH_TOKEN", "dummy".to_string()),
        ("ANTHROPIC_MODEL", model.to_string()),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model.to_string()),
        ("ANTHROPIC_SMALL_FAST_MODEL", small_model.to_string()),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", small_model.to_string()),
        ("DISABLE_NON_ESSENTIAL_MODEL_CALLS", "1".to_string()),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_string()),
    ];

    let bash_cmd = envs
        .iter()
        .map(|(k, v)| format!("export {}=\"{}\"", k, v))
        .collect::<Vec<_>>()
        .join("\n")
        + "\nclaude\n";

    let ps_cmd = envs
        .iter()
        .map(|(k, v)| format!("$env:{}=\"{}\"", k, v))
        .collect::<Vec<_>>()
        .join("\n")
        + "\nclaude\n";

    if std::env::var("COPILOT_CLIPBOARD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(ps_cmd.clone());
        }
    }

    println!("\nClaude Code environment (bash/zsh):\n{}", bash_cmd);
    println!("Claude Code environment (PowerShell):\n{}", ps_cmd);

    Ok(())
}
