//! Web Push: RFC 8291 message encryption (aes128gcm, RFC 8188) and RFC 8292 VAPID.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

use crate::util::{b64url, b64url_decode, random_bytes};

const RECORD_SIZE: u32 = 4096;

pub struct Subscription {
    pub endpoint: String,
    pub p256dh: Vec<u8>,
    pub auth: Vec<u8>,
}

impl Subscription {
    /// Parses the JSON of a browser `PushSubscription`.
    pub fn parse(json: &str) -> Option<Subscription> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let endpoint = v["endpoint"].as_str()?.to_string();
        let p256dh = b64url_decode(v["keys"]["p256dh"].as_str()?)?;
        let auth = b64url_decode(v["keys"]["auth"].as_str()?)?;
        (p256dh.len() == 65 && auth.len() == 16).then_some(Subscription {
            endpoint,
            p256dh,
            auth,
        })
    }
}

/// Encrypts `plaintext` for one subscription; the result is the aes128gcm request body.
pub fn encrypt(sub: &Subscription, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
    if plaintext.len() + 17 > RECORD_SIZE as usize {
        return Err("payload too large");
    }
    let ua_public = PublicKey::from_sec1_bytes(&sub.p256dh).map_err(|_| "invalid p256dh")?;
    let as_secret = loop {
        if let Ok(key) = SecretKey::from_slice(&random_bytes::<32>()) {
            break key;
        }
    };
    let as_public = as_secret.public_key().to_encoded_point(false);
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua_public.as_affine());

    // IKM = HKDF(auth_secret, ecdh_secret, "WebPush: info" || 0 || ua_public || as_public)
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&sub.p256dh);
    key_info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&sub.auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .map_err(|_| "hkdf")?;

    let salt: [u8; 16] = random_bytes();
    let prk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| "hkdf")?;
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| "hkdf")?;

    let mut record = plaintext.to_vec();
    record.push(0x02); // last record, no padding
    let cipher = Aes128Gcm::new((&cek).into());
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), record.as_slice())
        .map_err(|_| "encrypt")?;

    let mut body = Vec::with_capacity(16 + 4 + 1 + 65 + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.as_bytes().len() as u8);
    body.extend_from_slice(as_public.as_bytes());
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// The `Authorization: vapid t=…, k=…` header value for an endpoint.
pub fn vapid_header(
    private_key: &[u8; 32],
    public_key_b64: &str,
    subject: &str,
    endpoint: &str,
) -> Option<String> {
    let url = url::Url::parse(endpoint).ok()?;
    let audience = url.origin().ascii_serialization();
    let exp = chrono::Utc::now().timestamp() + 12 * 3600;
    let header = b64url(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64url(
        serde_json::json!({"aud": audience, "exp": exp, "sub": subject})
            .to_string()
            .as_bytes(),
    );
    let signing_input = format!("{header}.{claims}");
    let key = SigningKey::from_slice(private_key).ok()?;
    let signature: Signature = key.sign(signing_input.as_bytes());
    Some(format!(
        "vapid t={signing_input}.{}, k={public_key_b64}",
        b64url(&signature.to_bytes())
    ))
}
