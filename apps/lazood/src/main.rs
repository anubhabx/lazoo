#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();
    if let Err(error) = lazood::run().await {
        tracing::error!(%error, "daemon failed");
        std::process::exit(1);
    }
}
