//! Configuration from the environment. Missing required values stop the process with a clear
//! message; nothing secret is ever printed.

use std::net::SocketAddr;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub public_url: String,
    pub web_url: String,
    pub web_secure: bool,
    pub allowed_origins: Vec<String>,
    pub encryption_key: [u8; 32],
    pub accounts_url: String,
    pub accounts_app_id: String,
    pub accounts_app_secret: String,
    pub accounts_webhook_secret: Option<String>,
    pub ting_url: String,
    pub livekit_url: String,
    pub livekit_public_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    pub media_secret: String,
    pub apns: Option<ApnsConfig>,
    pub fcm_service_account: Option<Vec<u8>>,
    pub vapid: Option<VapidConfig>,
    pub s3_bucket: Option<String>,
    pub s3_prefix: String,
    pub aws_region: String,
    pub web_dir: Option<PathBuf>,
}

pub struct ApnsConfig {
    pub key_id: String,
    pub team_id: String,
    pub bundle_id: String,
    pub key_pem: Vec<u8>,
}

pub struct VapidConfig {
    pub private_key: [u8; 32],
    pub public_key: String,
    pub subject: String,
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn required(name: &str, what: &str) -> Result<String, ConfigError> {
    var(name).ok_or_else(|| ConfigError(format!("missing required configuration {name} ({what})")))
}

fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

fn http_url(name: &str, value: String) -> Result<String, ConfigError> {
    let parsed =
        url::Url::parse(&value).map_err(|_| ConfigError(format!("{name} is not a valid URL")))?;
    if !matches!(parsed.scheme(), "http" | "https" | "ws" | "wss") {
        return Err(ConfigError(format!(
            "{name} must be an http(s) or ws(s) URL"
        )));
    }
    Ok(value.trim_end_matches('/').to_string())
}

impl Config {
    pub fn from_env() -> Result<Config, ConfigError> {
        let bind = var("RING_BIND")
            .unwrap_or_else(|| "127.0.0.1:8765".into())
            .parse()
            .map_err(|_| ConfigError("RING_BIND must be host:port, e.g. 127.0.0.1:8765".into()))?;
        let data_dir =
            PathBuf::from(var("RING_DATA_DIR").unwrap_or_else(|| "/var/lib/ring".into()));
        let public_url = http_url(
            "RING_PUBLIC_URL",
            required("RING_PUBLIC_URL", "this server's public origin")?,
        )?;
        let web_url = http_url(
            "RING_WEB_URL",
            required("RING_WEB_URL", "the web app origin")?,
        )?;
        let web_secure = web_url.starts_with("https://");
        let mut allowed_origins: Vec<String> = var("RING_ALLOWED_ORIGINS")
            .map(|v| v.split(',').filter_map(|o| origin_of(o.trim())).collect())
            .unwrap_or_default();
        if let Some(web_origin) = origin_of(&web_url)
            && !allowed_origins.contains(&web_origin)
        {
            allowed_origins.push(web_origin);
        }

        let key_hex = required(
            "RING_ENCRYPTION_KEY",
            "64 hex characters, the AES-256 key that seals refresh tokens",
        )?;
        let key_bytes = hex::decode(&key_hex)
            .map_err(|_| ConfigError("RING_ENCRYPTION_KEY must be 64 hex characters".into()))?;
        let encryption_key: [u8; 32] = key_bytes.try_into().map_err(|_| {
            ConfigError("RING_ENCRYPTION_KEY must be exactly 32 bytes (64 hex characters)".into())
        })?;

        let accounts_url = http_url(
            "RING_ACCOUNTS_URL",
            var("RING_ACCOUNTS_URL")
                .unwrap_or_else(|| "https://accounts.teamofsilicons.com".into()),
        )?;
        let accounts_app_id = var("RING_ACCOUNTS_APP_ID").unwrap_or_else(|| "ring".into());
        let accounts_app_secret = required(
            "RING_ACCOUNTS_APP_SECRET",
            "the Silicon Accounts app secret for ring",
        )?;
        let accounts_webhook_secret = var("RING_ACCOUNTS_WEBHOOK_SECRET");
        let ting_url = http_url(
            "RING_TING_URL",
            var("RING_TING_URL")
                .unwrap_or_else(|| "https://backend.ting.teamofsilicons.com".into()),
        )?;

        let livekit_url = http_url(
            "RING_LIVEKIT_URL",
            var("RING_LIVEKIT_URL").unwrap_or_else(|| "http://127.0.0.1:7880".into()),
        )?;
        let livekit_public_url = http_url(
            "RING_LIVEKIT_PUBLIC_URL",
            required(
                "RING_LIVEKIT_PUBLIC_URL",
                "the LiveKit URL clients connect to",
            )?,
        )?;
        let livekit_api_key = required("RING_LIVEKIT_API_KEY", "the LiveKit API key")?;
        let livekit_api_secret = required("RING_LIVEKIT_API_SECRET", "the LiveKit API secret")?;
        let media_secret = required(
            "RING_MEDIA_SECRET",
            "the shared secret ring-media uses on /internal/media",
        )?;
        if media_secret.len() < 16 {
            return Err(ConfigError(
                "RING_MEDIA_SECRET must be at least 16 characters".into(),
            ));
        }

        let apns = Self::apns()?;
        let fcm_service_account =
            if let Some(path) = var("RING_FCM_SERVICE_ACCOUNT_FILE") {
                Some(std::fs::read(&path).map_err(|_| {
                    ConfigError("could not read RING_FCM_SERVICE_ACCOUNT_FILE".into())
                })?)
            } else if let Some(b64) = var("RING_FCM_SERVICE_ACCOUNT_BASE64") {
                Some(STANDARD.decode(b64).map_err(|_| {
                    ConfigError("RING_FCM_SERVICE_ACCOUNT_BASE64 is not valid base64".into())
                })?)
            } else {
                None
            };
        let vapid = Self::vapid()?;

        Ok(Config {
            bind,
            data_dir,
            public_url,
            web_url,
            web_secure,
            allowed_origins,
            encryption_key,
            accounts_url,
            accounts_app_id,
            accounts_app_secret,
            accounts_webhook_secret,
            ting_url,
            livekit_url,
            livekit_public_url,
            livekit_api_key,
            livekit_api_secret,
            media_secret,
            apns,
            fcm_service_account,
            vapid,
            s3_bucket: var("RING_S3_BUCKET"),
            s3_prefix: var("RING_S3_PREFIX").unwrap_or_default(),
            aws_region: var("AWS_REGION").unwrap_or_else(|| "us-west-1".into()),
            web_dir: var("RING_WEB_DIR").map(PathBuf::from),
        })
    }

    fn apns() -> Result<Option<ApnsConfig>, ConfigError> {
        let key_pem = if let Some(path) = var("RING_APNS_KEY_FILE") {
            std::fs::read(&path)
                .map_err(|_| ConfigError("could not read RING_APNS_KEY_FILE".into()))?
        } else if let Some(b64) = var("RING_APNS_KEY_BASE64") {
            STANDARD
                .decode(b64)
                .map_err(|_| ConfigError("RING_APNS_KEY_BASE64 is not valid base64".into()))?
        } else {
            return Ok(None);
        };
        Ok(Some(ApnsConfig {
            key_id: required("RING_APNS_KEY_ID", "the APNs signing key id")?,
            team_id: required("RING_APNS_TEAM_ID", "the Apple team id")?,
            bundle_id: var("RING_APNS_BUNDLE_ID")
                .unwrap_or_else(|| "com.teamofsilicons.ring".into()),
            key_pem,
        }))
    }

    fn vapid() -> Result<Option<VapidConfig>, ConfigError> {
        let (Some(private), Some(public)) =
            (var("RING_VAPID_PRIVATE_KEY"), var("RING_VAPID_PUBLIC_KEY"))
        else {
            return Ok(None);
        };
        let private_key: [u8; 32] = crate::util::b64url_decode(&private)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                ConfigError(
                    "RING_VAPID_PRIVATE_KEY must be the raw 32-byte P-256 scalar, base64url".into(),
                )
            })?;
        let public_bytes = crate::util::b64url_decode(&public)
            .filter(|b| b.len() == 65 && b[0] == 4)
            .ok_or_else(|| {
                ConfigError(
                    "RING_VAPID_PUBLIC_KEY must be the 65-byte uncompressed P-256 point, base64url"
                        .into(),
                )
            })?;
        let subject =
            var("RING_VAPID_SUBJECT").unwrap_or_else(|| "https://ring.teamofsilicons.com".into());
        if !(subject.starts_with("mailto:") || subject.starts_with("https://")) {
            return Err(ConfigError(
                "RING_VAPID_SUBJECT must be a mailto: or https: URL".into(),
            ));
        }
        Ok(Some(VapidConfig {
            private_key,
            public_key: crate::util::b64url(&public_bytes),
            subject,
        }))
    }

    /// The web sign-in callback, on the web app's own origin (it proxies `/v1` to this server).
    pub fn web_callback_uri(&self) -> String {
        format!("{}/v1/auth/web/callback", self.web_url)
    }

    pub fn s3_key(&self, kind: &str, id: &str, ext: &str) -> String {
        format!("{}{kind}/{id}.{ext}", self.s3_prefix)
    }
}
