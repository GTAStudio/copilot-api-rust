use axum::{
    Router,
    routing::{get, post},
};
use clap::Parser;
use cli::{AuthArgs, Command, DebugArgs, StartArgs};
use hooks::{HookExecutor, types::HookInput};
use std::io::Read;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod approval;
mod auth_flow;
mod cli;
mod commands;
mod config;
mod errors;
mod hooks;
mod paths;
mod protocol;
mod rate_limit;
mod routes;
mod security;
mod services;
mod skills_sync;
mod state;
mod token_store;
mod tokenizer;
mod utils;

#[cfg(test)]
mod integration_tests;

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();
    init_tracing(resolve_verbose(&cli));
    if let Err(error) = run_cli(cli, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    {
        eprintln!("{error}");
        std::process::exit(match error {
            errors::ApiError::BadRequest(_) | errors::ApiError::Forbidden(_) => 2,
            _ => 1,
        });
    }
}

async fn run_cli(
    cli: cli::Cli,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> errors::ApiResult<()> {
    if let Some(Command::Auth(args)) = &cli.command {
        return run_auth_flow(args).await;
    }

    if let Some(Command::CheckUsage) = &cli.command {
        let client = utils::http_client()?;
        let config = state::AppConfig::default();
        let state = state::AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            client,
            hooks: None,
        };
        return commands::run_check_usage(&state).await;
    }

    if let Some(Command::Debug(DebugArgs { json })) = &cli.command {
        return commands::run_debug(*json).await;
    }

    if let Some(Command::SyncSkills) = &cli.command {
        skills_sync::sync_skills().await?;
        println!("Skills synced into .claude/skills");
        return Ok(());
    }

    if let Some(Command::Hook(args)) = &cli.command {
        let input = read_hook_input()?;
        let event = args
            .event
            .clone()
            .or_else(|| input.hook_type.clone())
            .unwrap_or_else(|| "PreToolUse".to_string());
        let observer = if std::env::var("COPILOT_OBSERVATIONS_ENABLED").as_deref() == Ok("1") {
            Some(hooks::observe::start_observer().await?)
        } else {
            None
        };
        let config_path = args.config.as_ref().map(std::path::PathBuf::from);
        let executor = HookExecutor::load(config_path, observer)?;
        let results = executor.execute_event(&event, &input).await;
        if let Some(observer) = &executor.observer {
            observer.flush().await?;
        }
        let results = results?;
        let blocked = results.iter().any(|r| r.exit_code != 0);
        for r in &results {
            if !r.stderr.is_empty() {
                eprintln!("{}", r.stderr.trim_end());
            }
        }
        if blocked {
            return Err(errors::ApiError::Forbidden(
                "Hook blocked the operation".to_string(),
            ));
        }
        return Ok(());
    }

    let client = utils::http_client()?;

    let mut config = state::AppConfig::default();
    match &cli.command {
        Some(Command::Start(args)) => {
            config.account_type = args.account_type.clone();
            config.manual_approve = args.manual;
            config.rate_limit_seconds = args.rate_limit;
            config.rate_limit_wait = args.wait;
            config.show_token = args.show_token;
            if let Some(token) = &args.github_token {
                config.github_token = Some(token.clone());
            }
        }
        _ => {
            config.account_type = cli.account_type;
            config.manual_approve = cli.manual;
            config.rate_limit_seconds = cli.rate_limit;
            config.rate_limit_wait = cli.wait;
            config.show_token = cli.show_token;
            if let Some(token) = cli.github_token {
                config.github_token = Some(token);
            }
        }
    }
    let address = match &cli.command {
        Some(Command::Start(StartArgs { host, port, .. }))
            if host.contains(':') && !host.starts_with('[') =>
        {
            format!("[{host}]:{port}")
        }
        Some(Command::Start(StartArgs { host, port, .. })) => format!("{host}:{port}"),
        _ => cli.addr.clone(),
    };
    let addr = security::validate_bind_address(&address, config.api_key.as_deref())?;
    if !["individual", "business", "enterprise"].contains(&config.account_type.as_str()) {
        return Err(errors::ApiError::BadRequest(
            "Invalid Copilot account type".to_string(),
        ));
    }
    if config
        .rate_limit_seconds
        .is_some_and(|interval| interval > 86_400)
    {
        return Err(errors::ApiError::BadRequest(
            "Rate limit must not exceed 86400 seconds".to_string(),
        ));
    }
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|_| {
        errors::ApiError::BadRequest("Unable to bind the requested listening address".to_string())
    })?;
    let addr = listener.local_addr().map_err(|_| {
        errors::ApiError::Internal("Unable to resolve listening address".to_string())
    })?;
    config.vscode_version = services::vscode::fetch_vscode_version(&client).await;

    let hooks_enabled = std::env::var("COPILOT_HOOKS_ENABLED")
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
        .unwrap_or(true);
    let observer =
        if hooks_enabled && std::env::var("COPILOT_OBSERVATIONS_ENABLED").as_deref() == Ok("1") {
            Some(hooks::observe::start_observer().await?)
        } else {
            None
        };
    let hook_executor = if hooks_enabled {
        Some(std::sync::Arc::new(HookExecutor::load(None, observer)?))
    } else {
        None
    };
    let state = state::AppState {
        config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        client,
        hooks: hook_executor.clone(),
    };

    if let Some(hooks) = hook_executor.clone() {
        let input = HookInput {
            hook_type: Some("SessionStart".to_string()),
            ..Default::default()
        };
        let results = hooks.execute_event("SessionStart", &input).await?;
        if results.iter().any(|result| result.exit_code != 0) {
            return Err(errors::ApiError::Forbidden(
                "Session start hook blocked startup".to_string(),
            ));
        }
    }

    // Prewarm tokens/models in background for stability and faster first request.
    let prewarm = if std::env::var("COPILOT_PROVIDER").unwrap_or_else(|_| "copilot".to_string())
        == "copilot"
    {
        let prewarm_state = state.clone();
        Some(tokio::spawn(async move {
            if let Err(err) = paths::ensure_paths().await {
                tracing::warn!("Failed to ensure paths: {}", err);
            }

            match auth_flow::ensure_copilot_token(&prewarm_state).await {
                Ok(token) => {
                    let cfg = prewarm_state.config.read().await.clone();
                    match services::copilot::get_models(&prewarm_state.client, &cfg, &token).await {
                        Ok(models) => {
                            prewarm_state.config.write().await.models = Some(models);
                        }
                        Err(err) => tracing::warn!("Failed to prewarm models: {}", err),
                    }
                }
                Err(err) => tracing::warn!("Failed to prewarm Copilot token: {}", err),
            }
        }))
    } else {
        None
    };

    if let Some(Command::Start(StartArgs {
        host,
        port,
        claude_code,
        ..
    })) = &cli.command
    {
        if *claude_code {
            let server_url = format!("http://{}:{}", host, port);
            if let Err(err) = commands::run_claude_code_helper(&state, &server_url).await {
                eprintln!("Failed to prepare Claude Code helper: {}", err);
            }
        }
    } else if cli.claude_code
        && let Some((host, port)) = cli.addr.split_once(':')
    {
        let server_url = format!("http://{}:{}", host, port);
        if let Err(err) = commands::run_claude_code_helper(&state, &server_url).await {
            eprintln!("Failed to prepare Claude Code helper: {}", err);
        }
    }

    let app = build_router(state);

    if let Ok(base) = std::env::var("COPILOT_USAGE_VIEWER_URL") {
        let endpoint = format!("http://{}", addr);
        tracing::info!("Usage viewer: {}?endpoint={}", base, endpoint);
    }
    tracing::info!("listening on {}", addr);
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            if let Some(hooks) = hook_executor {
                let input = HookInput {
                    hook_type: Some("SessionEnd".to_string()),
                    ..Default::default()
                };
                let _ = hooks.execute_event("SessionEnd", &input).await;
                if let Some(observer) = &hooks.observer
                    && observer.flush().await.is_err()
                {
                    tracing::warn!("Observation log could not be flushed during shutdown");
                }
            }
        })
        .await;
    if let Some(task) = prewarm {
        task.abort();
        let _ = task.await;
    }
    result.map_err(|_| errors::ApiError::Internal("HTTP server failed".to_string()))
}

fn build_router(state: state::AppState) -> Router {
    Router::new()
        .route("/", get(routes::misc::root))
        .route("/chat/completions", post(routes::chat_completions::handle))
        .route("/models", get(routes::models::list))
        .route("/embeddings", post(routes::misc::embeddings))
        .route("/usage", get(routes::misc::usage))
        .route("/auth/device-code", get(routes::auth::device_code))
        .route("/auth/poll", post(routes::auth::poll_token))
        .route("/auth/status", get(routes::auth::status))
        .route(
            "/v1/chat/completions",
            post(routes::chat_completions::handle),
        )
        .route("/v1/models", get(routes::models::list))
        .route("/v1/embeddings", post(routes::misc::embeddings))
        .route("/v1/responses", post(routes::responses::handle))
        .route("/v1/messages", post(routes::messages::handle))
        .route(
            "/v1/messages/count_tokens",
            post(routes::messages::count_tokens),
        )
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state,
            security::authorize,
        ))
        .layer(TraceLayer::new_for_http())
}

async fn run_auth_flow(args: &AuthArgs) -> errors::ApiResult<()> {
    use std::io::Write;
    let client = utils::http_client()?;
    let config = state::AppConfig::default();
    let device = services::github::get_device_code(&client, &config).await?;
    println!(
        "Please enter the code \"{}\" in {}",
        device.user_code, device.verification_uri
    );
    std::io::stdout()
        .flush()
        .map_err(|_| errors::ApiError::Internal("Failed to display device code".to_string()))?;
    let credential = services::github::poll_access_token(&client, &config, &device).await?;
    services::github::get_github_user(
        &client,
        &state::AppConfig::default(),
        &credential.access_token,
    )
    .await?;
    token_store::write_github_credential(&credential).await?;
    if args.show_token {
        println!("GitHub token: {}", credential.access_token);
    }
    println!("GitHub token saved");
    Ok(())
}

fn resolve_verbose(cli: &cli::Cli) -> bool {
    match &cli.command {
        Some(Command::Start(args)) => args.verbose,
        Some(Command::Auth(args)) => args.verbose,
        Some(Command::Debug(_)) => cli.verbose,
        Some(Command::CheckUsage) => cli.verbose,
        Some(Command::Hook(_)) => cli.verbose,
        Some(Command::SyncSkills) => cli.verbose,
        None => cli.verbose,
    }
}

fn init_tracing(verbose: bool) {
    let filter = if verbose {
        tracing_subscriber::EnvFilter::new("debug")
    } else {
        tracing_subscriber::EnvFilter::from_default_env()
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
}

fn read_hook_input() -> errors::ApiResult<HookInput> {
    let mut buffer = String::new();
    std::io::stdin()
        .take(1_048_577)
        .read_to_string(&mut buffer)
        .map_err(|_| errors::ApiError::BadRequest("Invalid hook input".to_string()))?;
    if buffer.len() > 1_048_576 {
        return Err(errors::ApiError::BadRequest(
            "Hook input too large".to_string(),
        ));
    }
    if buffer.trim().is_empty() {
        return Ok(HookInput::default());
    }
    let input: HookInput = serde_json::from_str(&buffer)
        .map_err(|_| errors::ApiError::BadRequest("Invalid hook JSON".to_string()))?;
    input.validate()?;
    Ok(input)
}
