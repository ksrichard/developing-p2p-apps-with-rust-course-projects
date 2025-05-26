use std::sync::mpsc::channel;

use app::Server;

mod app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // termination handler
    let (term_tx, term_rx) = channel();
    ctrlc::set_handler(move || term_tx.send(()).expect("Can't send signal on channel"))?;

    // start server
    let server = Server::new();
    server.start().await?;

    // wait for termination
    term_rx.recv()?;

    // stop server
    server.stop().await?;

    Ok(())
}
