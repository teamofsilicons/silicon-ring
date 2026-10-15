//! Small helpers shared by every module: time, ids, hashing, sealing and text checks.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use ring_proto::Time;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn time(ms: i64) -> Time {
    Time::from_millis(ms).unwrap_or_else(Time::now)
}

pub fn opt_time(ms: Option<i64>) -> Option<Time> {
    ms.and_then(Time::from_millis)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rand::rng().fill_bytes(&mut out);
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn b64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim().trim_end_matches('=')).ok()
}

const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// `prefix` + a lowercase ULID (48-bit millisecond time, 80 random bits), so ids sort by creation.
pub fn new_id(prefix: &str) -> String {
    let ms = now_ms().max(0) as u128 & ((1u128 << 48) - 1);
    let random: [u8; 10] = random_bytes();
    let mut value = ms << 80;
    for (i, b) in random.iter().enumerate() {
        value |= (*b as u128) << (8 * (9 - i));
    }
    let mut out = String::with_capacity(prefix.len() + 26);
    out.push_str(prefix);
    for i in (0..26).rev() {
        out.push(CROCKFORD[((value >> (5 * i)) & 0x1f) as usize] as char);
    }
    out
}

/// A fresh Ring session token: `rst_` + 64 hex.
pub fn new_session_token() -> String {
    format!(
        "{}{}",
        ring_proto::prefix::SESSION_TOKEN,
        hex::encode(random_bytes::<32>())
    )
}

pub fn looks_like_session_token(token: &str) -> bool {
    token
        .strip_prefix(ring_proto::prefix::SESSION_TOKEN)
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// AES-256-GCM sealing for Accounts refresh tokens and login cookies.
/// Layout: 12-byte nonce || ciphertext+tag. `aad` binds a sealed value to its purpose.
#[derive(Clone)]
pub struct Sealer {
    cipher: Aes256Gcm,
}

impl Sealer {
    pub fn new(key: &[u8; 32]) -> Self {
        Sealer {
            cipher: Aes256Gcm::new(key.into()),
        }
    }

    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
        let nonce_bytes: [u8; 12] = random_bytes();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let mut out = nonce_bytes.to_vec();
        let sealed = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("AES-GCM encryption cannot fail");
        out.extend_from_slice(&sealed);
        out
    }

    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < 12 + 16 {
            return None;
        }
        let (nonce, body) = sealed.split_at(12);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: body, aad })
            .ok()
    }

    pub fn seal_str(&self, plaintext: &str, aad: &[u8]) -> Vec<u8> {
        self.seal(plaintext.as_bytes(), aad)
    }

    pub fn open_str(&self, sealed: &[u8], aad: &[u8]) -> Option<String> {
        self.open(sealed, aad)
            .and_then(|v| String::from_utf8(v).ok())
    }
}

/// Rejects text over `limit` code points. Never truncates.
pub fn check_len(field: &str, text: &str, limit: usize) -> Result<(), ApiError> {
    let length = ring_proto::text_len(text);
    if length > limit {
        return Err(ApiError::text_too_long(field, limit, length));
    }
    Ok(())
}

/// Trims and drops empty optional text.
pub fn clean(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Constant-time string comparison for shared secrets.
pub fn secret_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
