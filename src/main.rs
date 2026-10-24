use std::{error::Error, net::SocketAddr};

use dfcompanion::{api::companion_server::CompanionServer, run_companion_worker, web, AppState};
use tokio::net::TcpListener;
use tokio_stream::{wrappers::TcpListenerStream, StreamExt};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let grpc_address: SocketAddr = "127.0.0.1:50051".parse()?;
    let http_address: SocketAddr = "127.0.0.1:3000".parse()?;
    let (state, queue_rx) = AppState::load().map_err(std::io::Error::other)?;
    let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));
    tokio::spawn(dfcompanion::check_game_at_startup(state.clone()));

    let grpc_listener = TcpListener::bind(grpc_address).await?;
    let http_listener = TcpListener::bind(http_address).await?;
    // A reset on an incoming connection must not stop the server.
    let grpc_incoming = TcpListenerStream::new(grpc_listener).filter(|connection| {
        if let Err(error) = connection {
            eprintln!("Ignoring failed incoming gRPC connection: {error}");
        }
        connection.is_ok()
    });
    let grpc_service = dfcompanion::rpc::service(state.clone());
    let grpc_server = Server::builder()
        .add_service(CompanionServer::new(grpc_service))
        .serve_with_incoming(grpc_incoming);
    let mut app = web::router(state);
    if std::env::args().any(|arg| arg == "--trace-http") {
        app = web::with_request_tracing(app);
    }
    let http_server = axum::serve(http_listener, app);

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
