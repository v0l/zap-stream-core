use clap::Parser;

#[derive(Parser, Debug)]
struct Args {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    Ok(())
}