//! Uses the official AWS SDK credential chain, including EC2 roles, for managed or BYO buckets.
use crate::{Error, Result};
use aws_sdk_s3::{primitives::ByteStream, Client};

#[derive(Clone)]
pub struct S3 {
    client: Client,
    bucket: String,
    prefix: String,
}
impl S3 {
    pub async fn new(
        bucket: String,
        region: Option<String>,
        endpoint: Option<String>,
        prefix: String,
    ) -> Result<Self> {
        if bucket.is_empty() || prefix.contains("..") || prefix.starts_with('/') {
            return Err(Error::new(
                "INVALID_STORAGE",
                "Provide a bucket and relative object prefix.",
                false,
            ));
        }
        crate::init_tls();
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(region) = region {
            loader = loader.region(aws_config::Region::new(region));
        }
        let config = loader.load().await;
        let mut config = aws_sdk_s3::config::Builder::from(&config);
        if let Some(endpoint) = endpoint {
            let url = reqwest::Url::parse(&endpoint).map_err(|_| {
                Error::new(
                    "INVALID_STORAGE",
                    "S3 endpoint must be an AWS HTTPS origin.",
                    false,
                )
            })?;
            if url.scheme() != "https"
                || !url.host_str().is_some_and(|host| {
                    host.ends_with(".amazonaws.com")
                        && (host.contains(".s3.") || host.starts_with("s3."))
                })
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::new(
                    "INVALID_STORAGE",
                    "Only AWS S3 HTTPS endpoints are supported.",
                    false,
                ));
            }
            config = config.endpoint_url(endpoint).force_path_style(true);
        }
        Ok(Self {
            client: Client::from_conf(config.build()),
            bucket,
            prefix,
        })
    }
    pub async fn new_with_credentials(
        bucket: String,
        region: String,
        prefix: String,
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    ) -> Result<Self> {
        if bucket.is_empty()
            || prefix.contains("..")
            || prefix.starts_with('/')
            || access_key_id.is_empty()
            || secret_access_key.is_empty()
        {
            return Err(Error::new(
                "INVALID_STORAGE",
                "Provide bucket, region and valid BYO S3 credentials.",
                false,
            ));
        }
        crate::init_tls();
        let credentials = aws_sdk_s3::config::Credentials::new(
            access_key_id,
            secret_access_key,
            session_token,
            None,
            "ring-org",
        );
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region))
            .credentials_provider(credentials)
            .load()
            .await;
        Ok(Self {
            client: Client::new(&config),
            bucket,
            prefix,
        })
    }
    fn key(&self, key: &str) -> Result<String> {
        if key.is_empty() || key.starts_with('/') || key.split('/').any(|v| v == ".." || v == ".") {
            return Err(Error::new(
                "INVALID_STORAGE_KEY",
                "Use a relative object key without traversal.",
                false,
            ));
        }
        Ok(format!("{}{}", self.prefix, key))
    }
    pub async fn put(&self, key: &str, content_type: &str, bytes: Vec<u8>) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.key(key)?)
            .content_type(content_type)
            .server_side_encryption(aws_sdk_s3::types::ServerSideEncryption::Aes256)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|_| Error::network("S3 upload"))?;
        Ok(())
    }
    /// Authorize the caller's participation interval before retrieving any recording bytes.
    pub async fn get(&self, key: &str) -> Result<Vec<u8>> {
        self.client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key(key)?)
            .send()
            .await
            .map_err(|_| Error::network("S3 download"))?
            .body
            .collect()
            .await
            .map(|v| v.into_bytes().to_vec())
            .map_err(|_| Error::network("S3 download"))
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.key(key)?)
            .send()
            .await
            .map_err(|_| Error::network("S3 delete"))?;
        Ok(())
    }
}
