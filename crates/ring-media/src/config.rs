use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Runtime configuration, read once from the environment.
#[derive(Clone)]
pub struct Config {
    /// ring-server's internal media WebSocket (`/internal/media`).
    pub server_url: String,
    pub media_secret: String,
    /// LiveKit signalling URL as a `ws://` or `wss://` URL.
    pub livekit_url: String,
    pub openai_key: Option<String>,
    pub deepgram_key: Option<String>,
    pub s3_bucket: Option<String>,
    pub aws_region: Option<String>,
    pub data_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let server_url = required("RING_MEDIA_SERVER_URL")?;
        if !(server_url.starts_with("ws://") || server_url.starts_with("wss://")) {
            bail!("RING_MEDIA_SERVER_URL must be a ws:// or wss:// URL");
        }
        let livekit_url = websocket_url(
            &optional("RING_LIVEKIT_URL").unwrap_or_else(|| "ws://127.0.0.1:7880".into()),
        )?;
        let data_dir =
            PathBuf::from(optional("RING_MEDIA_DATA_DIR").unwrap_or_else(|| "data/media".into()));
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create RING_MEDIA_DATA_DIR {}", data_dir.display()))?;
        Ok(Self {
            server_url,
            media_secret: required("RING_MEDIA_SECRET")?,
            livekit_url,
            openai_key: optional("OPENAI_API_KEY"),
            deepgram_key: optional("DEEPGRAM_API_KEY"),
            s3_bucket: optional("RING_S3_BUCKET"),
            aws_region: optional("AWS_REGION"),
            data_dir,
        })
    }

    pub fn openai_key(&self) -> Result<&str> {
        self.openai_key
            .as_deref()
            .context("OPENAI_API_KEY is not configured")
    }

    pub fn deepgram_key(&self) -> Result<&str> {
        self.deepgram_key
            .as_deref()
            .context("DEEPGRAM_API_KEY is not configured")
    }

    /// Log which optional integrations are missing; values are never logged.
    pub fn warn_missing(&self) {
        for (name, present) in [
            ("OPENAI_API_KEY", self.openai_key.is_some()),
            ("DEEPGRAM_API_KEY", self.deepgram_key.is_some()),
            ("RING_S3_BUCKET", self.s3_bucket.is_some()),
            ("AWS_REGION", self.aws_region.is_some()),
        ] {
            if !present {
                tracing::warn!("{name} is not set; work that needs it will be reported as failed");
            }
        }
    }
}

fn required(name: &str) -> Result<String> {
    optional(name).with_context(|| format!("{name} is required"))
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// LiveKit's SDK wants a WebSocket URL; deployments often configure the HTTP origin instead.
fn websocket_url(url: &str) -> Result<String> {
    let url = url.trim_end_matches('/');
    if let Some(rest) = url.strip_prefix("http://") {
        Ok(format!("ws://{rest}"))
    } else if let Some(rest) = url.strip_prefix("https://") {
        Ok(format!("wss://{rest}"))
    } else if url.starts_with("ws://") || url.starts_with("wss://") {
        Ok(url.to_owned())
    } else {
        bail!("RING_LIVEKIT_URL must be an http(s):// or ws(s):// URL")
    }
}
