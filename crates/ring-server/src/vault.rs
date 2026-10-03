//! Small encrypted credential store. Callers serialize refreshes per owner and retain operation IDs.
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub struct Vault {
    directory: PathBuf,
    cipher: Aes256Gcm,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}
impl Vault {
    /// Remove application/session grants while preserving the key used by sealed settings.
    pub fn clear_records(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|v| v == "bin") {
                fs::remove_file(path)?;
            }
        }
        sync_directory(&self.directory)
    }
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let encoded = std::env::var("RING_ENCRYPTION_KEY").ok();
        let production = std::env::var("RING_ENV").as_deref() == Ok("production");
        Self::open_with_key(data_dir, encoded.as_deref(), production)
    }
    fn open_with_key(data_dir: &Path, encoded: Option<&str>, production: bool) -> io::Result<Self> {
        let directory = data_dir.join("credentials");
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut key = if let Some(encoded) = encoded {
            STANDARD
                .decode(encoded)
                .map_err(|_| invalid("RING_ENCRYPTION_KEY must be base64 for exactly 32 bytes"))?
        } else {
            if production {
                return Err(invalid("RING_ENCRYPTION_KEY is required in production"));
            }
            let path = directory.join("master.key");
            if !path.exists() {
                let mut generated = [0u8; 32];
                OsRng.fill_bytes(&mut generated);
                match private_file(&path) {
                    Ok(mut file) => {
                        file.write_all(&generated)?;
                        file.sync_all()?;
                        sync_directory(&directory)?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
                generated.fill(0);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(&path)?.permissions().mode() & 0o077 != 0 {
                    return Err(invalid("Credential master.key permissions must be 0600"));
                }
            }
            fs::read(path)?
        };
        if key.len() != 32 {
            return Err(invalid("Encryption key must be exactly 32 bytes"));
        }
        let cipher =
            Aes256Gcm::new_from_slice(&key).map_err(|_| invalid("Invalid encryption key"))?;
        key.fill(0);
        Ok(Self { directory, cipher })
    }
    fn path(&self, owner: &str) -> PathBuf {
        self.directory
            .join(format!("{:x}.bin", Sha256::digest(owner.as_bytes())))
    }
    pub fn set(&self, owner: &str, value: &Value) -> io::Result<()> {
        if owner.is_empty() {
            return Err(invalid("Credential owner is required"));
        }
        let mut plaintext =
            serde_json::to_vec(value).map_err(|_| invalid("Could not encode credential"))?;
        if plaintext.len() > 4 * 1024 * 1024 {
            return Err(invalid("Credential record exceeds 4MiB"));
        }
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let encrypted = self.cipher.encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: owner.as_bytes(),
            },
        );
        plaintext.fill(0);
        let encrypted = encrypted.map_err(|_| invalid("Could not encrypt credential"))?;
        let temp = self
            .directory
            .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = private_file(&temp)?;
            file.write_all(&[1])?;
            file.write_all(&nonce)?;
            file.write_all(&encrypted)?;
            file.sync_all()?;
            fs::rename(&temp, self.path(owner))?;
            sync_directory(&self.directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
    pub fn get(&self, owner: &str) -> io::Result<Option<Value>> {
        let path = self.path(owner);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if bytes.len() < 29 || bytes.len() > 4 * 1024 * 1024 + 29 || bytes[0] != 1 {
            return Err(invalid("Invalid encrypted credential record"));
        }
        let mut plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&bytes[1..13]),
                Payload {
                    msg: &bytes[13..],
                    aad: owner.as_bytes(),
                },
            )
            .map_err(|_| {
                invalid("Credential authentication failed; restore the correct encryption key")
            })?;
        let value = serde_json::from_slice(&plaintext)
            .map_err(|_| invalid("Invalid decrypted credential JSON"));
        plaintext.fill(0);
        value.map(Some)
    }
    pub fn seal(&self, label: &str, value: &Value) -> io::Result<String> {
        let mut plaintext =
            serde_json::to_vec(value).map_err(|_| invalid("Could not encode secret"))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self.cipher.encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: label.as_bytes(),
            },
        );
        plaintext.fill(0);
        let mut bytes = nonce.to_vec();
        bytes.extend(ciphertext.map_err(|_| invalid("Could not encrypt secret"))?);
        Ok(format!("sealed:{}", STANDARD.encode(bytes)))
    }
    pub fn unseal(&self, label: &str, value: &str) -> io::Result<Value> {
        let bytes = STANDARD
            .decode(
                value
                    .strip_prefix("sealed:")
                    .ok_or_else(|| invalid("Expected sealed secret"))?,
            )
            .map_err(|_| invalid("Invalid sealed secret"))?;
        if bytes.len() < 28 {
            return Err(invalid("Invalid sealed secret"));
        }
        let mut plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&bytes[..12]),
                Payload {
                    msg: &bytes[12..],
                    aad: label.as_bytes(),
                },
            )
            .map_err(|_| invalid("Secret authentication failed"))?;
        let value =
            serde_json::from_slice(&plaintext).map_err(|_| invalid("Invalid decrypted secret"));
        plaintext.fill(0);
        value
    }
    pub fn remove(&self, owner: &str) -> io::Result<()> {
        match fs::remove_file(self.path(owner)) {
            Ok(()) => sync_directory(&self.directory),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn survives_restart_without_plaintext_and_rejects_tampering_and_owner_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open_with_key(dir.path(), None, false).unwrap();
        let value =
            serde_json::json!({"refresh_token":"secret-refresh-token","operation":"retry-123"});
        vault.set("production|tos|si:alice", &value).unwrap();
        let path = vault.path("production|tos|si:alice");
        assert!(
            !String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("secret-refresh-token")
        );
        assert_eq!(
            Vault::open_with_key(dir.path(), None, false)
                .unwrap()
                .get("production|tos|si:alice")
                .unwrap(),
            Some(value)
        );
        fs::copy(&path, vault.path("production|tos|si:bob")).unwrap();
        assert!(vault.get("production|tos|si:bob").is_err());
        let mut bytes = fs::read(&path).unwrap();
        bytes[15] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(vault.get("production|tos|si:alice").is_err());
        vault.remove("production|tos|si:alice").unwrap();
        assert!(vault.get("production|tos|si:alice").unwrap().is_none());
        assert!(Vault::open_with_key(dir.path(), None, true).is_err());
    }
}
