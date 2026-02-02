use clap::{Args, Parser, Subcommand};

#[derive(Debug, Clone, Parser)]
#[command(name = "copilot-api-rs", version, about = "Copilot API server (Rust)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(long, default_value = "127.0.0.1:4141")]
    pub addr: String,

    #[arg(long, default_value = "individual")]
    pub account_type: String,

    #[arg(long, default_value_t = false)]
    pub manual: bool,

    #[arg(long)]
    pub rate_limit: Option<u64>,

    #[arg(long, default_value_t = false)]
    pub wait: bool,

    #[arg(long)]
    pub github_token: Option<String>,

    #[arg(long, default_value_t = false)]
    pub show_token: bool,

    #[arg(long, short = 'v', default_value_t = false)]
    pub verbose: bool,

    #[arg(long, default_value_t = false)]
    pub proxy_env: bool,

    #[arg(long, default_value_t = false)]
    pub claude_code: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Start the server
    Start(StartArgs),
    /// Run GitHub device auth flow
    Auth(AuthArgs),
    /// Show Copilot usage/quota information
    CheckUsage,
    /// Print debug information
    Debug(DebugArgs),
    /// Run Claude hooks processor
    Hook(HookArgs),
    /// Sync everything-claude-code skills into .claude/skills
    SyncSkills,
}

#[derive(Debug, Clone, Args)]
pub struct StartArgs {
    #[arg(long, default_value_t = 4141)]
    pub port: u16,

    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value = "individual")]
    pub account_type: String,

    #[arg(long, default_value_t = false)]
    pub manual: bool,

    #[arg(long)]
    pub rate_limit: Option<u64>,

    #[arg(long, default_value_t = false)]
    pub wait: bool,

    #[arg(long)]
    pub github_token: Option<String>,

    #[arg(long, default_value_t = false)]
    pub show_token: bool,

    #[arg(long, short = 'v', default_value_t = false)]
    pub verbose: bool,

    #[arg(long, default_value_t = false)]
    pub proxy_env: bool,

    #[arg(long, default_value_t = false)]
    pub claude_code: bool,
}

#[derive(Debug, Clone, Args)]
pub struct AuthArgs {
    #[arg(long, default_value_t = false)]
    pub show_token: bool,

    #[arg(long, short = 'v', default_value_t = false)]
    pub verbose: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DebugArgs {
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct HookArgs {
    #[arg(long)]
    pub event: Option<String>,

    #[arg(long)]
    pub config: Option<String>,
}
