use std::{error::Error, net::SocketAddr};

use dfcompanion::{api::companion_server::CompanionServer, run_companion_worker, web, AppState};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let grpc_address: SocketAddr = "127.0.0.1:50051".parse()?;
    let http_address: SocketAddr = "127.0.0.1:3000".parse()?;
    let plugin_token = std::env::var("DFCOMPANION_PLUGIN_TOKEN").ok();
    if let Some(token) = &plugin_token {
        if token.len() < 32 {
            return Err("DFCOMPANION_PLUGIN_TOKEN must be at least 32 bytes".into());
        }
    } else {
        eprintln!(
            "DFCOMPANION_PLUGIN_TOKEN is not set; the Lua plugin event endpoint will reject requests."
        );
    }
    let (state, queue_rx) = AppState::new();
    let state = match plugin_token {
        Some(token) => state.with_plugin_token(token),
        None => state,
    };
    let worker = tokio::spawn(run_companion_worker(
        queue_rx,
        state.notifications_tx.clone(),
    ));

    let grpc_listener = TcpListener::bind(grpc_address).await?;
    let http_listener = TcpListener::bind(http_address).await?;
    let grpc_incoming = TcpListenerStream::new(grpc_listener);
    let grpc_service = dfcompanion::rpc::service(state.clone());
    let grpc_server = Server::builder()
        .add_service(CompanionServer::new(grpc_service))
        .serve_with_incoming(grpc_incoming);
    let http_server = axum::serve(http_listener, web::router(state));

    println!("DFCompanion gRPC listening on http://{grpc_address}");
    println!("DFCompanion web UI listening on http://{http_address}");

    tokio::select! {
        result = grpc_server => result?,
        result = http_server => result?,
        result = tokio::signal::ctrl_c() => result?,
    }

    worker.abort();
    Ok(())
}
