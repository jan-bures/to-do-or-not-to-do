//! Server entry point. Configuration (hostnames, secrets, allowlists) is read at runtime from a local
//! config file outside the repository; nothing deployment-specific is compiled in.

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!(
        protocol = todo_core::PROTOCOL_VERSION,
        "todo-server skeleton"
    );
    Ok(())
}
