use clap::Parser;

mod native_hook;

#[tokio::main]
async fn main() {
    if let Some(code) = native_hook::run_if_requested().await {
        std::process::exit(code);
    }
    let cli = lazooctl::Cli::parse();
    let _governor = lazoo_runtime::DataRoot::resolve()
        .and_then(|root| lazoo_runtime::Installation::load_or_create(&root))
        .ok()
        .and_then(|installation| {
            match lazoo_runtime::InfrastructureGovernor::attach_current(
                installation.id().to_string(),
            ) {
                Ok((governor, _, _)) => Some(governor),
                Err(error) => {
                    eprintln!("infrastructure CPU cap unavailable: {error}");
                    None
                }
            }
        });
    match lazooctl::execute_with(&cli, |line| println!("{line}")).await {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
