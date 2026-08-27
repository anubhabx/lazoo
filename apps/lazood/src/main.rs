#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .json()
        .init();
    if let Err(error) = lazood::run().await {
        tracing::error!(%error, "daemon failed");
        std::process::exit(1);
    }
}
