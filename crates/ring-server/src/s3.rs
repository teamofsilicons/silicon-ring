//! S3: presigned GET URLs for recordings and voicemail audio (ring-media uploads them).

use std::time::Duration;

use aws_sdk_s3::presigning::PresigningConfig;
use ring_proto::Time;

const URL_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub struct S3 {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3 {
    pub async fn new(bucket: &str, region: &str) -> S3 {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;
        S3 {
            client: aws_sdk_s3::Client::new(&config),
            bucket: bucket.to_string(),
        }
    }

    /// A URL that downloads `key` for the next 10 minutes.
    pub async fn presign_get(&self, key: &str) -> Result<(String, Time), String> {
        let presigning = PresigningConfig::expires_in(URL_TTL).map_err(|e| e.to_string())?;
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(presigning)
            .await
            .map_err(|e| {
                format!(
                    "presign failed: {}",
                    aws_sdk_s3::error::DisplayErrorContext(&e)
                )
            })?;
        let expires =
            Time(chrono::Utc::now() + chrono::Duration::from_std(URL_TTL).unwrap_or_default());
        Ok((request.uri().to_string(), expires))
    }

    pub async fn delete(&self, key: &str) -> Result<(), String> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map(|_| ())
            .map_err(|e| {
                format!(
                    "delete failed: {}",
                    aws_sdk_s3::error::DisplayErrorContext(&e)
                )
            })
    }
}
