use std::{
    ffi::OsString,
    fs::File,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{anyhow, Context};
use dfcompanion::{app::CompanionApp, user_events::UserEventLayer, web};
use tokio::net::TcpListener;
use tracing_subscriber::{
    self, fmt::format::FmtSpan, layer::SubscriberExt, registry::LookupSpan,
    util::SubscriberInitExt, EnvFilter, Layer,
};

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    http_tracing: bool,
    trace_file: Option<PathBuf>,
}

fn parse_options(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<Options> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--trace-http" {
            options.http_tracing = true;
        } else if arg == "--trace-file" {
            if options.trace_file.is_some() {
                return Err(anyhow!("--trace-file may only be specified once"));
            }
            let path = args
                .next()
                .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
                .ok_or_else(|| anyhow!("--trace-file requires a file path"))?;
            options.trace_file = Some(path.into());
        } else {
            return Err(anyhow!("Invalid argument: {}", arg.to_string_lossy()));
        }
    }
    Ok(options)
}

fn open_trace_file(path: &Path) -> anyhow::Result<File> {
    File::create(path).with_context(|| format!("Could not open trace file {}", path.display()))
}

fn file_trace_layer<S>(file: File, filter: EnvFilter) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(filter)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = parse_options(std::env::args_os().skip(1))?;
    let http_address: SocketAddr = "127.0.0.1:3000".parse()?;

    let (app, queue_rx) = CompanionApp::load().map_err(std::io::Error::other)?;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("none"));
    let trace_file = options
        .trace_file
        .as_deref()
        .map(open_trace_file)
        .transpose()?;
    let file_layer = trace_file.map(|file| file_trace_layer(file, filter.clone()));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(filter);
    let user_event_layer = UserEventLayer::new(app.notifier().clone());
    tracing_subscriber::registry()
        .with(file_layer)
        .with(fmt_layer)
        .with(user_event_layer)
        .init();
    tracing::info!("Starting DFCompanion");

    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    let lifecycle_poller = tokio::spawn(app.clone().run_lifecycle_poller());
    let http_listener = TcpListener::bind(http_address).await?;
    let http_service = if options.http_tracing {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::Arc};

    #[test]
    fn should_parse_trace_file_and_http_options() {
        let options = parse_options(
            ["--trace-http", "--trace-file", "trace output.log"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap();

        assert_eq!(
            options,
            Options {
                http_tracing: true,
                trace_file: Some(PathBuf::from("trace output.log")),
            }
        );
    }

    #[test]
    fn should_reject_missing_or_repeated_trace_file_paths() {
        for args in [
            vec!["--trace-file"],
            vec!["--trace-file", "--trace-http"],
            vec!["--trace-file", "a", "--trace-file", "b"],
        ] {
            assert!(parse_options(args.into_iter().map(OsString::from)).is_err());
        }
    }

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(data)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn should_duplicate_filtered_tracing_and_overwrite_the_existing_file() {
        let path = std::env::temp_dir().join(format!(
            "dfcompanion-tracing-test-{}.log",
            std::process::id()
        ));
        let mut existing = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        existing.write_all(b"old content").unwrap();
        drop(existing);
        let file = open_trace_file(&path).unwrap();
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let captured = capture.clone();
        let filter = EnvFilter::new("info");
        let subscriber = tracing_subscriber::registry()
            .with(file_trace_layer(file, filter.clone()))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || capture.clone())
                    .with_filter(filter),
            );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("visible trace event");
            tracing::debug!("filtered trace event");
        });

        let file_text = std::fs::read_to_string(&path).unwrap();
        let console_text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        std::fs::remove_file(path).unwrap();

        for output in [file_text, console_text] {
            assert!(output.contains("visible trace event"));
            assert!(!output.contains("filtered trace event"));
            assert!(!output.contains("old content"));
        }
    }
}
