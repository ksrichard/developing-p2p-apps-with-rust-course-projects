use std::sync::mpsc::channel;

use app::Server;
use clap::Parser;
use cli::Cli;

pub mod app;
pub mod cli;
pub mod file_processor;
pub mod file_store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // CLI
    let cli = Cli::parse();

    match cli.command {
        cli::Commands::Start => {
            // termination handler
            let (term_tx, term_rx) = channel();
            ctrlc::set_handler(move || term_tx.send(()).expect("Can't send signal on channel"))?;

            // start server
            let server = Server::new(cli);
            server.start().await?;

            // wait for termination
            term_rx.recv()?;

            // stop server
            server.stop().await?;
        }
    }

    Ok(())
}
