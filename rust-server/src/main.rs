use axum::{routing::{get, post}, Router};
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tower_http::{cors::{Any, CorsLayer}, trace::TraceLayer};
use cli::{Command, StartArgs, AuthArgs, DebugArgs};

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

    let state = state::AppState {
        config: std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        client,
    };

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
