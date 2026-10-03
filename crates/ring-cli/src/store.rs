use crate::platform::{self, OpenOptionsExt};
use ring_client::{ConnectOptions, Result, RingError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const DEFAULT_SERVER_URL: &str = "wss://backend.ring.teamofsilicons.com/ws";

#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
    pub key: String,
    pub org: Option<String>,
    pub test: bool,
    pub socket_dir: PathBuf,
}
pub fn io_error(e: impl std::fmt::Display, step: &str) -> RingError {
    RingError::new(
        "LOCAL_IO_ERROR",
        e.to_string(),
        step,
        "Check the path, ownership, permissions and available disk space.",
    )
}
pub fn invalid(message: impl Into<String>, step: &str) -> RingError {
    RingError::new(
        "INVALID_INPUT",
        message,
        step,
        "Inspect this command's --help and correct the input.",
    )
}
impl Store {
    pub fn new(org: Option<String>, test: bool) -> Result<Self> {
        let home = std::env::var_os("SILICON_HOME").ok_or_else(|| {
            RingError::new(
                "SILICON_HOME_REQUIRED",
                "SILICON_HOME is required for runtime operations",
                "storage",
                "Set SILICON_HOME to your interpreter's private home directory.",
            )
        })?;
        if !Path::new(&home).is_absolute() {
            return Err(invalid(
                "SILICON_HOME must be an absolute directory path",
                "storage",
            ));
        }
        let dir = PathBuf::from(home).join(".ring");
        if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(invalid(
                "SILICON_HOME/.ring must not be a symlink",
                "storage",
            ));
        }
        fs::create_dir_all(&dir).map_err(|e| io_error(e, "storage"))?;
        platform::private_dir(&dir)?;
        let hash = hex::encode(Sha256::digest(format!(
            "{}:{}",
            if test { "test" } else { "production" },
            org.as_deref().unwrap_or("default")
        )));
        #[cfg(unix)]
        let socket_dir = {
            use std::os::unix::fs::MetadataExt;
            // Unix sockets have a ~104-byte path limit on macOS.
            let uid = unsafe { libc::geteuid() };
            let path = PathBuf::from(format!("/tmp/silicon-ring-{uid}"));
            if fs::symlink_metadata(&path)
                .is_ok_and(|m| m.file_type().is_symlink() || m.uid() != uid)
            {
                return Err(invalid(
                    "Unsafe daemon socket directory ownership",
                    "storage",
                ));
            }
            fs::create_dir_all(&path).map_err(|e| io_error(e, "socket storage"))?;
            platform::private_dir(&path)?;
            path
        };
        #[cfg(windows)]
        let socket_dir = dir.clone();
        Ok(Self {
            dir,
            key: hash[..16].into(),
            org,
            test,
            socket_dir,
        })
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}-{name}", self.key))
    }
    pub fn socket(&self) -> PathBuf {
        let identity = format!("{}:{}", self.dir.display(), self.key);
        self.socket_dir.join(format!(
            "{}.sock",
            &hex::encode(Sha256::digest(identity))[..24]
        ))
    }
    pub fn read(&self, name: &str) -> Result<Value> {
        read_json(&self.path(name))
    }
    pub fn write(&self, name: &str, v: &Value) -> Result<()> {
        atomic_write(
            &self.path(name),
            &serde_json::to_vec_pretty(v).map_err(|e| io_error(e, "serialize"))?,
        )
    }
    pub fn local(&self) -> Result<Value> {
        read_json(&self.dir.join("config.json"))
    }
    pub fn save_local(&self, v: &Value) -> Result<()> {
        validate_local(v)?;
        atomic_write(
            &self.dir.join("config.json"),
            &serde_json::to_vec_pretty(v).map_err(|e| io_error(e, "serialize"))?,
        )
    }
    pub fn options(&self) -> Result<ConnectOptions> {
        let config = self.local()?;
        let secret = if self.test {
            match std::env::var("SILICON_RING_TEST_APP_SECRET_FILE") {
                Ok(p) => {
                    let path = Path::new(&p);
                    if !platform::protected_secret(path)? {
                        return Err(invalid("Test secret file must be accessible only to its owner (chmod 600 on Unix)","test authentication"));
                    }
                    Some(
                        fs::read_to_string(path)
                            .map_err(|e| io_error(e, "test authentication"))?
                            .trim()
                            .to_owned(),
                    )
                }
                Err(_) => std::env::var("SILICON_RING_TEST_APP_SECRET").ok(),
            }
        } else {
            None
        };
        Ok(ConnectOptions {
            server_url: std::env::var("SILICON_RING_SERVER_URL")
                .ok()
                .or_else(|| config["server_url"].as_str().map(str::to_owned))
                .unwrap_or_else(|| DEFAULT_SERVER_URL.into()),
            org_id: self.org.clone(),
            realm: if self.test { "test" } else { "production" }.into(),
            test_app_secret: secret,
            client_name: "ring-daemon".into(),
        })
    }
}
pub fn read_json(path: &Path) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| io_error(e, "read JSON")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(io_error(e, "read")),
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| io_error(e, "write"))?;
        f.write_all(bytes).map_err(|e| io_error(e, "write"))?;
        f.sync_all().map_err(|e| io_error(e, "sync"))?;
        fs::rename(&temp, path).map_err(|e| io_error(e, "rename"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub fn write_output(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).mode(0o600);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut f = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RingError::new(
                "OUTPUT_CONFLICT",
                "Output file already exists",
                "write output",
                "Choose another path or pass --overwrite.",
            )
        } else {
            io_error(e, "write output")
        }
    })?;
    f.write_all(bytes).map_err(|e| io_error(e, "write output"))
}
pub fn read_text(inline: &Option<String>, file: &Option<String>) -> Result<String> {
    if let Some(text) = inline {
        return Ok(text.clone());
    }
    if let Some(path) = file {
        if path == "-" {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| io_error(e, "stdin"))?;
            Ok(s)
        } else {
            fs::read_to_string(path).map_err(|e| io_error(e, "text file"))
        }
    } else {
        Err(invalid("Provide text or its file alternative", "text"))
    }
}
pub fn validate_local(v: &Value) -> Result<()> {
    let obj = v
        .as_object()
        .ok_or_else(|| invalid("Configuration must be a JSON object", "config"))?;
    for (key, value) in obj {
        let valid = match key.as_str() {
            "server_url" => value
                .as_str()
                .is_some_and(|s| s.starts_with("wss://") || s.starts_with("ws://")),
            "output" => matches!(value.as_str(), Some("text" | "json")),
            "audio.input" | "audio.output" => value.is_string() || value.is_null(),
            "updates.channel" => matches!(value.as_str(), Some("stable" | "beta")),
            "telemetry.enabled" => value.is_boolean(),
            _ => false,
        };
        if !valid {
            return Err(invalid(
                format!("Unknown local key or invalid value: {key}"),
                "config",
            ));
        }
    }
    Ok(())
}
pub fn redact(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                let key = key.to_lowercase();
                if key.contains("token")
                    || key.contains("secret")
                    || key.contains("api_key")
                    || key.contains("credential")
                {
                    *value = json!("[REDACTED]");
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(a) => {
            for value in a {
                redact(value)
            }
        }
        _ => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_config_and_redacts() {
        assert!(validate_local(&json!({"telemetry.enabled":false})).is_ok());
        assert!(validate_local(&json!({"other":1})).is_err());
        let mut v = json!({"session_token":"secret","nested":{"api_key":"secret"},"actor":"si:a"});
        redact(&mut v);
        assert!(!v.to_string().contains(":\"secret\""));
    }
}
