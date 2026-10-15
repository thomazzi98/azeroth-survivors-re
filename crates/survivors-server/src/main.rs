use clap::Parser;

#[derive(Parser)]
#[command(about = "Authoritative server for online Azeroth Survivors (PoC)")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:8100")]
    bind: String,
    #[arg(long, default_value_t = 2)]
    max_players: usize,
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let cli = Cli::parse();
    let app = survivors_server::Server::start(cli.max_players, cli.seed).router();
    let listener = tokio::net::TcpListener::bind(&cli.bind).await?;
    tracing::info!(bind = %cli.bind, max_players = cli.max_players, "survivors-server listening on /ws");
    axum::serve(listener, app).await?;
    Ok(())
}
