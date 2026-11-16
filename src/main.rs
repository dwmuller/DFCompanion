use std::{error::Error, net::SocketAddr};

use dfcompanion::{app::CompanionApp, web};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let http_address: SocketAddr = "127.0.0.1:3000".parse()?;
    let (app, queue_rx) = CompanionApp::load().map_err(std::io::Error::other)?;
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    let lifecycle_poller = tokio::spawn(app.clone().run_lifecycle_poller());
    app.check_game_at_startup().await;

    let http_listener = TcpListener::bind(http_address).await?;
    let mut http_service = web::router(app);
    if std::env::args().any(|arg| arg == "--trace-http") {
        http_service = web::with_request_tracing(http_service);
    }
    let http_server = axum::serve(http_listener, http_service);

    println!("DFCompanion web UI listening on http://{http_address}");

    tokio::select! {
        result = http_server => result.map_err(|e| format!("HTTP server stopped: {e:?}"))?,
        result = tokio::signal::ctrl_c() => result?,
    }

    worker.abort();
    lifecycle_poller.abort();
    Ok(())
}
