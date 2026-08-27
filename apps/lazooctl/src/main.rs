use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = lazooctl::Cli::parse();
    match lazooctl::execute_with(&cli, |line| println!("{line}")).await {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
