//! Shared clients: HTTP (Deepgram prerecorded) and S3 uploads.

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use aws_sdk_s3::{primitives::ByteStream, types::ServerSideEncryption};
use tokio::sync::OnceCell;

use crate::config::Config;

pub struct Services {
    config: Arc<Config>,
    pub http: reqwest::Client,
    s3: OnceCell<aws_sdk_s3::Client>,
}

const UPLOAD_ATTEMPTS: u32 = 4;

impl Services {
    pub fn new(config: Arc<Config>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("the HTTP client configuration is static");
        Self {
            config,
            http,
            s3: OnceCell::new(),
        }
    }

    async fn s3(&self) -> &aws_sdk_s3::Client {
        self.s3
            .get_or_init(|| async {
                // The SDK's 3.1 s connect timeout is too eager for a busy uplink; retries cover the rest.
                let timeouts = aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .build();
                let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .timeout_config(timeouts);
                if let Some(region) = &self.config.aws_region {
                    loader = loader.region(aws_config::Region::new(region.clone()));
                }
                aws_sdk_s3::Client::new(&loader.load().await)
            })
            .await
    }

    /// Uploads a WAV file to `key` in the Ring bucket, encrypted at rest, retrying transient failures.
    pub async fn upload_wav(&self, path: &Path, key: &str) -> Result<()> {
        let bucket = self
            .config
            .s3_bucket
            .as_deref()
            .context("RING_S3_BUCKET is not configured")?;
        let client = self.s3().await;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let body = ByteStream::from_path(path)
                .await
                .with_context(|| format!("cannot read {}", path.display()))?;
            let result = client
                .put_object()
                .bucket(bucket)
                .key(key)
                .content_type("audio/wav")
                .server_side_encryption(ServerSideEncryption::Aes256)
                .body(body)
                .send()
                .await;
            match result {
                Ok(_) => return Ok(()),
                Err(e) if attempt < UPLOAD_ATTEMPTS => {
                    tracing::warn!(
                        attempt,
                        "S3 upload failed, retrying: {}",
                        aws_sdk_s3::error::DisplayErrorContext(&e)
                    );
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "S3 upload failed: {}",
                        aws_sdk_s3::error::DisplayErrorContext(&e)
                    ));
                }
            }
        }
    }
}
