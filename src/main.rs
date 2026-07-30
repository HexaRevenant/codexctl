#[tokio::main]
async fn main() {
    if let Err(e) = codexctl::cli::run().await {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}
