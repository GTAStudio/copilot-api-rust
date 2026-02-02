use axum::{routing::{get, post}, Router};
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tower_http::{cors::{Any, CorsLayer}, trace::TraceLayer};
use cli::{Command, StartArgs, AuthArgs, DebugArgs};
use hooks::{HookExecutor, types::HookInput};
use std::io::Read;

mod approval;
mod commands;
mod cli;
mod auth_flow;
mod config;
mod errors;
mod paths;
mod rate_limit;
mod routes;
mod services;
mod state;
mod token_store;
mod utils;
mod tokenizer;
mod hooks;
mod skills_sync;

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();

    init_tracing(resolve_verbose(&cli));

    if let Some(Command::Auth(args)) = &cli.command {
        run_auth_flow(args).await;
        return;
    }

    if let Some(Command::CheckUsage) = &cli.command {
        let client = reqwest::Client::builder()
            .user_agent("copilot-api-rs")
            .build()
            .expect("reqwest client");
        let config = state::AppConfig::default();
        let state = state::AppState {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
            client,
            hooks: None,
        };
        if let Err(err) = commands::run_check_usage(&state).await {
            eprintln!("Failed to fetch usage: {}", err);
        }
        return;
    }

    if let Some(Command::Debug(DebugArgs { json })) = &cli.command {
        if let Err(err) = commands::run_debug(*json).await {
            eprintln!("Failed to print debug info: {}", err);
        }
        return;
    }

    if let Some(Command::SyncSkills) = &cli.command {
        if let Err(err) = skills_sync::sync_skills().await {
            eprintln!("Failed to sync skills: {}", err);
        } else {
            println!("Skills synced into .claude/skills");
        }
        return;
    }

    if let Some(Command::Hook(args)) = &cli.command {
        let input = read_hook_input();
        let event = args.event.clone().or_else(|| input.hook_type.clone()).unwrap_or_else(|| "PreToolUse".to_string());
        let observer = hooks::observe::start_observer().await.ok();
        let config_path = args.config.as_ref().map(std::path::PathBuf::from);
        let executor = HookExecutor::load(config_path, observer).unwrap();
        let results = executor.execute_event(&event, &input).await.unwrap_or_default();
        let blocked = results.iter().any(|r| r.exit_code != 0);
        for r in &results {
            if !r.stderr.is_empty() {
                eprintln!("{}", r.stderr.trim_end());
            }
        }
        println!("{}", serde_json::to_string(&input).unwrap_or_default());
        if blocked {
            std::process::exit(1);
        }
        return;
    }

    let mut client_builder = reqwest::Client::builder()
        .user_agent("copilot-api-rs")
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(20);
    let proxy_env = match &cli.command {
        Some(Command::Start(StartArgs { proxy_env, .. })) => *proxy_env,
        _ => cli.proxy_env,
    };
    if proxy_env {
        if let Ok(proxy) = std::env::var("ALL_PROXY") {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                client_builder = client_builder.proxy(p);
            }
        }
        if let Ok(proxy) = std::env::var("HTTPS_PROXY") {
            if let Ok(p) = reqwest::Proxy::https(proxy) {
                client_builder = client_builder.proxy(p);
            }
        }
        if let Ok(proxy) = std::env::var("HTTP_PROXY") {
            if let Ok(p) = reqwest::Proxy::http(proxy) {
                client_builder = client_builder.proxy(p);
            }
        }
    }

    let client = client_builder.build().expect("reqwest client");

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
    config.vscode_version = services::vscode::fetch_vscode_version().await;

    let hooks_enabled = std::env::var("COPILOT_HOOKS_ENABLED")
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
        .unwrap_or(true);
    let observer = if hooks_enabled { hooks::observe::start_observer().await.ok() } else { None };
    let hook_executor = if hooks_enabled {
        HookExecutor::load(None, observer).ok().map(std::sync::Arc::new)
    } else {
        None
    };
    let state = state::AppState {
        config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        client,
        hooks: hook_executor.clone(),
    };

    if let Some(hooks) = hook_executor.clone() {
        let input = HookInput { hook_type: Some("SessionStart".to_string()), ..Default::default() };
        let _ = hooks.execute_event("SessionStart", &input).await;
        let stop_hooks = hooks.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let input = HookInput { hook_type: Some("SessionEnd".to_string()), ..Default::default() };
            let _ = stop_hooks.execute_event("SessionEnd", &input).await;
        });
    }

    // Prewarm tokens/models in background for stability and faster first request.
    {
        let prewarm_state = state.clone();
        tokio::spawn(async move {
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
        });
    }

    if let Some(Command::Start(StartArgs { host, port, claude_code, .. })) = &cli.command {
        if *claude_code {
            let server_url = format!("http://{}:{}", host, port);
            if let Err(err) = commands::run_claude_code_helper(&state, &server_url).await {
                eprintln!("Failed to prepare Claude Code helper: {}", err);
            }
        }
    } else if cli.claude_code {
        if let Some((host, port)) = cli.addr.split_once(':') {
            let server_url = format!("http://{}:{}", host, port);
            if let Err(err) = commands::run_claude_code_helper(&state, &server_url).await {
                eprintln!("Failed to prepare Claude Code helper: {}", err);
            }
        }
    }

    let app = Router::new()
        .route("/", get(routes::misc::root))
        .route("/chat/completions", post(routes::chat_completions::handle))
        .route("/models", get(routes::models::list))
        .route("/embeddings", post(routes::misc::embeddings))
        .route("/usage", get(routes::misc::usage))
        .route("/token", get(routes::misc::token))
        .route("/auth/device-code", get(routes::auth::device_code))
        .route("/auth/poll", post(routes::auth::poll_token))
        .route("/auth/token", get(routes::auth::current_token))
        .route("/v1/chat/completions", post(routes::chat_completions::handle))
        .route("/v1/models", get(routes::models::list))
        .route("/v1/embeddings", post(routes::misc::embeddings))
        .route("/v1/responses", post(routes::responses::handle))
        .route("/v1/messages", post(routes::messages::handle))
        .route("/v1/messages/count_tokens", post(routes::messages::count_tokens))
        .with_state(state)
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http());

    let addr = match &cli.command {
        Some(Command::Start(StartArgs { host, port, .. })) => format!("{}:{}", host, port),
        _ => cli.addr,
    };

    if let Ok(base) = std::env::var("COPILOT_USAGE_VIEWER_URL") {
        let endpoint = format!("http://{}", addr);
        tracing::info!("Usage viewer: {}?endpoint={}", base, endpoint);
    }
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind failed");

    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await.expect("server failed");
}

async fn run_auth_flow(args: &AuthArgs) {
    let client = reqwest::Client::builder()
        .user_agent("copilot-api-rs")
        .build()
        .expect("reqwest client");

    match services::github::get_device_code(&client).await {
        Ok(device) => {
            println!(
                "Please enter the code \"{}\" in {}",
                device.user_code, device.verification_uri
            );

            match services::github::poll_access_token(&client, &device).await {
                Ok(token) => {
                    if let Err(err) = token_store::write_github_token(&token).await {
                        eprintln!("Failed to write GitHub token: {}", err);
                        return;
                    }

                    if args.show_token {
                        println!("GitHub token: {}", token);
                    }

                    println!("GitHub token saved");
                }
                Err(err) => eprintln!("Failed to poll token: {}", err),
            }
        }
        Err(err) => eprintln!("Failed to get device code: {}", err),
    }
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
        .with(tracing_subscriber::fmt::layer())
        .init();
}

fn read_hook_input() -> HookInput {
    let mut buffer = String::new();
    let _ = std::io::stdin().read_to_string(&mut buffer);
    if buffer.trim().is_empty() {
        return HookInput::default();
    }
    serde_json::from_str::<HookInput>(&buffer).unwrap_or_default()
}
