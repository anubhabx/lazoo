#[tokio::main]
async fn main() {
    match lazoo_keeper::run().await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("keeper failed: {error}");
            std::process::exit(21);
        }
    }
}
