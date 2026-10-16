//! ring-server: the Silicon Ring control plane.

mod accounts;
mod app;
mod calls;
mod config;
mod db;
mod error;
mod extract;
mod hooks;
mod idempotency;
mod livekit;
mod media;
mod push;
mod ratelimit;
mod realtime;
mod router;
mod s3;
mod sessions;
mod settings;
mod ting;
mod transcript;
mod util;
mod voicemail;
mod web;
mod webpush;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::config::Config;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "ring-server: the Silicon Ring control plane

Usage: ring-server [--version | --help]

Configuration comes from the environment (RING_BIND, RING_DATA_DIR, RING_PUBLIC_URL, RING_WEB_URL,
RING_ENCRYPTION_KEY, RING_ACCOUNTS_*, RING_TING_URL, RING_LIVEKIT_*, RING_MEDIA_SECRET, RING_APNS_*,
RING_FCM_*, RING_VAPID_*, RING_S3_*, AWS_REGION, RING_WEB_DIR, RUST_LOG).";

fn main() {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("ring-server {VERSION}");
                return;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("ring-server: unknown argument {other:?}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    // aws-config logs the loaded credentials (access key id included) at info; keep that out.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("aws_config=warn".parse().expect("static directive"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ring-server: {error}");
            std::process::exit(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if let Err(error) = runtime.block_on(run(config)) {
        eprintln!("ring-server: {error}");
        std::process::exit(1);
    }
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db = db::Db::open(&config.data_dir)?;
    let accounts = accounts::Accounts::new(&config)?;
    let push = push::Push::new(&config).map_err(|e| format!("push configuration: {e}"))?;
    let s3 = match &config.s3_bucket {
        Some(bucket) => Some(s3::S3::new(bucket, &config.aws_region).await),
        None => None,
    };
    let app = Arc::new(App {
        sealer: util::Sealer::new(&config.encryption_key),
        livekit: livekit::LiveKit::new(&config),
        ting: ting::Ting::new(&config.ting_url),
        hub: realtime::Hub::default(),
        media: media::MediaLink::default(),
        call_locks: app::KeyedLocks::default(),
        timers: app::Timers::default(),
        limiter: ratelimit::RateLimiter::default(),
        accounts,
        push,
        s3,
        db,
        config,
    });
    tracing::info!(
        version = VERSION,
        push = app.push.summary().as_str(),
        s3 = app.s3.is_some(),
        accounts_webhook = app.config.accounts_webhook_secret.is_some(),
        "starting ring-server"
    );

    calls::engine::end_leftover_calls(&app).await;
    voicemail::resume_synthesis(&app);
    tokio::spawn(ting::worker(app.clone()));
    tokio::spawn(transcript::batch_worker(app.clone()));
    tokio::spawn(cleanup(app.clone()));

    let listener = tokio::net::TcpListener::bind(app.config.bind).await?;
    tracing::info!(bind = %app.config.bind, "listening");
    let router = router::build(app.clone());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown().await;
        let _ = stop_tx.send(true);
    });
    // Open WebSockets would hold a graceful shutdown forever; give them a few seconds.
    let mut stopping = stop_rx.clone();
    tokio::select! {
        result = server => result?,
        _ = async {
            let _ = stopping.wait_for(|s| *s).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        } => {}
    }
    // Tell open WebSockets to reconnect (1013) and let them say so before the process exits.
    app.hub.shut_down(Duration::from_secs(2)).await;
    tracing::info!("stopped");
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Hourly housekeeping: expired idempotency records, sign-in claims, previews and old outbox rows.
async fn cleanup(app: Arc<App>) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        let now = util::now_ms();
        let day = 24 * 3600 * 1000;
        let result = app.db.with(|c| {
            c.execute("DELETE FROM idempotency WHERE created_at < ?1", [now - day])?;
            c.execute("DELETE FROM auth_claims WHERE created_at < ?1", [now - day])?;
            c.execute("DELETE FROM context_preparations WHERE expires_at < ?1", [now - day])?;
            c.execute("DELETE FROM context_approvals WHERE expires_at < ?1", [now - 30 * day])?;
            c.execute("DELETE FROM webhook_events WHERE received_at < ?1", [now - 7 * day])?;
            c.execute("DELETE FROM ting_outbox WHERE status IN ('delivered', 'failed') AND updated_at < ?1", [now - 14 * day])?;
            // Media results that never arrived (ring-media restarted mid-upload) will not arrive now.
            c.execute("UPDATE calls SET recording_status = 'failed' WHERE recording_status = 'processing' AND ended_at < ?1", [now - 3600 * 1000])?;
            c.execute(
                "UPDATE voicemails SET audio_status = 'failed', transcript_status = CASE transcript_status WHEN 'pending' THEN 'failed' ELSE transcript_status END
                 WHERE audio_status IN ('recording', 'processing') AND created_at < ?1",
                [now - 3600 * 1000],
            )?;
            Ok(())
        });
        if let Err(error) = result {
            tracing::error!(error = %error, "cleanup failed");
        }
    }
}
