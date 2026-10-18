//! ring-media: Silicon Ring's media worker. It runs the server-side LiveKit participants
//! (silicon representatives, the call recorder with live speech-to-text, and the voicemail
//! player) on behalf of ring-server, which directs it over a loopback WebSocket.

mod audio;
mod config;
mod link;
mod live;
mod preroll;
mod recorder;
mod representative;
mod room;
mod roster;
mod services;
mod stt;
mod supervisor;
mod voicemail;

use std::{process::ExitCode, sync::Arc, time::Duration};

use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::{config::Config, link::Reporter, supervisor::Supervisor};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long shutdown waits for sessions to finish uploads and reports. With the final
/// hand-over to ring-server this stays inside systemd's 60 s stop timeout for the unit.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(45);

const HELP: &str = "\
ring-media: Silicon Ring media worker (representatives, recorder, voicemail, speech-to-text)

Usage: ring-media [--version | --help]

Connects to ring-server's internal media socket and runs the LiveKit participants it asks for.

Environment:
  RING_MEDIA_SERVER_URL   ring-server's internal socket, e.g. ws://127.0.0.1:8765/internal/media
  RING_MEDIA_SECRET       shared secret for that socket
  RING_LIVEKIT_URL        LiveKit URL (http(s):// or ws(s)://), default ws://127.0.0.1:7880
  OPENAI_API_KEY          GPT-Live, for representatives and Natural Voice synthesis
  DEEPGRAM_API_KEY        speech-to-text
  RING_S3_BUCKET          bucket for recordings and voicemail
  AWS_REGION              region of that bucket
  RING_MEDIA_DATA_DIR     working files and the greeting cache, default data/media
  RUST_LOG                log filter, default info
";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("-V" | "--version") => {
            println!("ring-media {VERSION}");
            return ExitCode::SUCCESS;
        }
        Some("-h" | "--help") => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("ring-media: unexpected argument {other:?}\n\n{HELP}");
            return ExitCode::from(2);
        }
        None => {}
    }
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // The AWS SDK logs credential details at info.
    let filter = filter.add_directive("aws_config=warn".parse().expect("static directive"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    // Several TLS stacks are linked (LiveKit, AWS, reqwest); pick one provider for all of them.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("ring-media: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let config = Arc::new(Config::from_env()?);
    config.warn_missing();
    tracing::info!(version = VERSION, livekit = %config.livekit_url, "ring-media starting");

    let (reporter, outgoing) = Reporter::channel();
    let shutdown = CancellationToken::new();
    let supervisor = Arc::new(Supervisor::new(config.clone(), reporter, shutdown.clone()));
    let link_stop = CancellationToken::new();
    let link = tokio::spawn(link::run(
        config,
        supervisor.clone(),
        outgoing,
        link_stop.clone(),
    ));

    terminated().await;
    tracing::info!("shutting down: finishing calls in progress");
    shutdown.cancel();
    supervisor.tracker.close();
    tokio::select! {
        _ = supervisor.tracker.wait() => {}
        _ = tokio::time::sleep(DRAIN_TIMEOUT) => tracing::warn!("sessions did not finish in time"),
        _ = terminated() => tracing::warn!("second signal: exiting now"),
    }
    link_stop.cancel();
    if tokio::time::timeout(Duration::from_secs(10), link)
        .await
        .is_err()
    {
        tracing::warn!("could not deliver the last results to ring-server");
    }
    tracing::info!("stopped");
    Ok(())
}

async fn terminated() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
