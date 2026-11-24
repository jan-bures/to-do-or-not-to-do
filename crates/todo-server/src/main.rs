//! `todo-server` binary. Configuration (bind address, data directory, token hashes) is read at runtime
//! from a config file outside the repository and from `TODO_SERVER_*` environment variables; nothing
//! deployment-specific is compiled in.

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;
use todo_server::auth::service::AuthService;
use todo_server::config::CONFIG_ENV;
use todo_server::{Config, Engine, Options, Server};

#[derive(Debug, Parser)]
#[command(name = "todo-server", version, about = "Personal to-do server")]
struct Cli {
    /// Path to the TOML config file.
    #[arg(long, global = true, env = CONFIG_ENV)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the HTTP server until Ctrl+C.
    Serve,
    /// Rebuild the projection tables from the event log, then exit. Run while the server is stopped.
    RebuildProjections,
    /// Clear all owner sign-ins and prepare a new setup code. Run while the server is stopped.
    ResetOwner {
        /// Confirm deletion of all passkeys and sessions and revocation of all paired devices.
        #[arg(long)]
        yes: bool,
        /// Also revoke all agent tokens. Recommended after a compromise.
        #[arg(long)]
        revoke_agents: bool,
        /// Also revoke all share links. Recommended after a compromise.
        #[arg(long)]
        revoke_shares: bool,
    },
    /// Give the event log a new epoch, then exit. Run it, with the server stopped, after restoring
    /// the database from a backup: clients that followed the replaced log then resync from scratch
    /// instead of mixing the two histories (see `GET /api/v1/events`).
    RotateLogEpoch,
    /// Read a token from stdin and print the SHA-256 hash to put in the config.
    HashToken,
    /// Check the config and print the absolute path of the database a server started in this
    /// working directory would open, then exit. Deploy scripts use it to find what to back up.
    DatabasePath,
    /// Generate a Web Push VAPID key in a new owner-only file. Does not require a running server.
    WebPushKey {
        /// Path outside the repo. The file must not already exist.
        #[arg(long)]
        out: PathBuf,
    },
    /// Make a new token for the local admin API: write it to a new owner-only file and print the
    /// config lines with its hash. Works while the server runs (it touches neither the database nor
    /// the config); restart the server after changing the config.
    AdminToken {
        /// The file to write the token to. It must not exist yet.
        #[arg(long)]
        out: PathBuf,
    },
}

async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "cannot listen for Ctrl+C");
        std::future::pending::<()>().await;
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(todo_server::mcp::log_filter(
            std::env::var("RUST_LOG").ok().as_deref(),
        ))
        .init();
    let cli = Cli::parse();
    match cli.command {
        Cmd::HashToken => {
            let mut token = String::new();
            std::io::stdin()
                .read_to_string(&mut token)
                .context("reading the token from stdin")?;
            let token = token.trim();
            anyhow::ensure!(!token.is_empty(), "no token on stdin");
            println!("{}", todo_server::auth::hash_token(token));
            Ok(())
        }
        Cmd::WebPushKey { out } => {
            todo_server::write_vapid_key(&out)?;
            println!(
                "Created an owner-only VAPID key. Set web_push.vapid_key_file and restart the server."
            );
            Ok(())
        }
        Cmd::AdminToken { out } => {
            let hash = todo_server::admin::write_token_file(&out)?;
            eprintln!(
                "Wrote a new admin token to {} (readable only by this account).\n\
                 Put these lines in the server config, with a loopback `bind`, and restart the server:\n",
                out.display()
            );
            println!("[admin]\nbind = \"127.0.0.1:47321\"\ntoken_sha256 = \"{hash}\"");
            Ok(())
        }
        Cmd::DatabasePath => {
            let config = Config::load(cli.config.as_deref())?;
            let cwd = std::env::current_dir().context("reading the working directory")?;
            println!("{}", config.database_path_in(&cwd).display());
            Ok(())
        }
        Cmd::RebuildProjections => {
            let config = Config::load(cli.config.as_deref())?;
            let path = config.database_path();
            let head = Engine::rebuild_projections(&path)
                .with_context(|| format!("rebuilding projections in {}", path.display()))?;
            tracing::info!(head, "projections rebuilt");
            Ok(())
        }
        Cmd::ResetOwner {
            yes,
            revoke_agents,
            revoke_shares,
        } => {
            anyhow::ensure!(
                yes,
                "pass --yes to clear all owner sign-ins; task data is kept"
            );
            let config = Config::load(cli.config.as_deref())?;
            let report = AuthService::reset_owner(&config, revoke_agents, revoke_shares)?;
            let counts = report.counts;
            println!(
                "Owner sign-ins reset in {}.\n\
                 Passkeys removed: {}; browser sessions removed: {}; paired devices revoked: {}; invites removed: 0 (already cleared when the server stopped).\n\
                 Start the server and open /setup. The one-time code is in {} (readable only by this account).\n\
                 Active agent tokens remaining: {} (revoked: {}). Active share links remaining: {} (revoked: {}).\n\
                 To revoke remaining credentials, use --revoke-agents and/or --revoke-shares on reset, Agents and Shares after setup, or the local admin API (POST /api/v1/admin/agents/{{id}}/revoke and POST /api/v1/admin/shares/{{id}}/revoke).",
                report.database.display(),
                counts.passkeys_removed,
                counts.sessions_removed,
                counts.devices_revoked,
                report.setup_file.display(),
                counts.agents_remaining,
                counts.agents_revoked,
                counts.shares_remaining,
                counts.shares_revoked,
            );
            Ok(())
        }
        Cmd::RotateLogEpoch => {
            let config = Config::load(cli.config.as_deref())?;
            let path = config.database_path();
            let epoch = Engine::rotate_log_epoch(&path)
                .with_context(|| format!("rotating the log epoch in {}", path.display()))?;
            tracing::info!(%epoch, "log epoch rotated");
            Ok(())
        }
        Cmd::Serve => {
            let config = Config::load(cli.config.as_deref())?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("starting the async runtime")?;
            rt.block_on(async {
                let server = Server::bind(&config, Options::default()).await?;
                server.run_until(ctrl_c()).await?;
                anyhow::Ok(())
            })
        }
    }
}
