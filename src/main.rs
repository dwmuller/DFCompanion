use std::{error::Error, net::SocketAddr};

use dfcompanion::{api::companion_server::CompanionServer, app::CompanionApp, web};
use tokio::net::TcpListener;
use tokio_stream::{wrappers::TcpListenerStream, StreamExt};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let grpc_address: SocketAddr = "127.0.0.1:50051".parse()?;
    let http_address: SocketAddr = "127.0.0.1:3000".parse()?;
    let (app, queue_rx) = CompanionApp::load().map_err(std::io::Error::other)?;
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    app.check_game_at_startup().await;

    let grpc_listener = TcpListener::bind(grpc_address).await?;
    let http_listener = TcpListener::bind(http_address).await?;
    // A reset on an incoming connection must not stop the server.
    let grpc_incoming = TcpListenerStream::new(grpc_listener).filter(|connection| {
        if let Err(error) = connection {
            eprintln!("Ignoring failed incoming gRPC connection: {error}");
        }
        connection.is_ok()
    });
    let grpc_service = dfcompanion::rpc::service(app.clone());
    let grpc_server = Server::builder()
        .add_service(CompanionServer::new(grpc_service))
        .serve_with_incoming(grpc_incoming);
    let mut http_service = web::router(app);
    if std::env::args().any(|arg| arg == "--trace-http") {
        http_service = web::with_request_tracing(http_service);
    }
    let http_server = axum::serve(http_listener, http_service);

    println!("DFCompanion gRPC listening on http://{grpc_address}");
    println!("DFCompanion web UI listening on http://{http_address}");

    tokio::select! {
        result = grpc_server => result.map_err(|e| format!("gRPC server stopped: {e:?}"))?,
        result = http_server => result.map_err(|e| format!("HTTP server stopped: {e:?}"))?,
        result = tokio::signal::ctrl_c() => result?,
    }

    worker.abort();
    Ok(())
}
