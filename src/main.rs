use std::net::SocketAddr;

use anyhow::anyhow;
use dfcompanion::{app::CompanionApp, user_events::UserEventLayer, web};
use regex::regex;
use tokio::net::TcpListener;
use tracing_subscriber::{
    self, fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let http_address: SocketAddr = "127.0.0.1:3000".parse()?;

    let (app, queue_rx) = CompanionApp::load().map_err(std::io::Error::other)?;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("none"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(filter);
    let user_event_layer = UserEventLayer::new(app.notifier().clone());
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(user_event_layer)
        .init();
    tracing::info!("Starting DFCompanion");

    let mut http_tracing = false;
    for arg in std::env::args().skip(1) {
        let Some(caps) = regex!(r"^--(\w+(-\w+)*)").captures(&arg) else {
            return Err(anyhow!("Invalid argument: {}", arg));
        };
        let opt = &caps[1];
        match opt {
            "trace-http" => {
                http_tracing = true;
            }
            _ => {
                return Err(anyhow!("Invalid option: {}", opt));
            }
        }
    }

    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    let lifecycle_poller = tokio::spawn(app.clone().run_lifecycle_poller());
    let http_listener = TcpListener::bind(http_address).await?;
    let http_service = if http_tracing {
        web::with_request_tracing(web::router(app.clone()))
    } else {
        web::router(app.clone())
    };
    let http_server = axum::serve(http_listener, http_service);

    println!("DFCompanion web UI listening on http://{http_address}");

    tokio::select! {
        result = http_server => result.map_err(|e| anyhow!("HTTP server stopped: {e:?}"))?,
        result = tokio::signal::ctrl_c() => result?,
    }

    worker.abort();
    lifecycle_poller.abort();
    Ok(())
}
