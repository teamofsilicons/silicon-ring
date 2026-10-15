//! Carbon sign-in helpers for desktop apps (RFC 8252 loopback + PKCE). See `udd/accounts.md`.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::{Error, Result};

/// A PKCE verifier/challenge pair plus a random `state`.
#[derive(Clone, Debug)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    /// 64-char URL-safe verifier, S256 challenge, 32-char state.
    pub fn new() -> Self {
        let verifier = random_urlsafe::<48>();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
            state: random_urlsafe::<24>(),
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// `N` random bytes as unpadded base64url (`4N/3` characters).
fn random_urlsafe<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Builds the hosted Accounts sign-in URL:
/// `{accounts_url}/authorize?response_type=code&app_id=ring&redirect_uri=…&scope=profile&state=…&code_challenge=…&code_challenge_method=S256`.
pub fn authorize_url(accounts_url: &str, redirect_uri: &str, pkce: &Pkce) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("app_id", "ring")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "profile")
        .append_pair("state", &pkce.state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{}/authorize?{query}", accounts_url.trim_end_matches('/'))
}

/// A loopback listener on `127.0.0.1:<random port>` that receives the Accounts redirect.
pub struct LoopbackReceiver {
    pub redirect_uri: String,
    pub(crate) listener: tokio::net::TcpListener,
}

/// The authorization result delivered to the redirect URI.
#[derive(Clone, Debug)]
pub enum Callback {
    Code {
        code: String,
        state: String,
    },
    Error {
        error: String,
        description: Option<String>,
    },
}

/// Browsers open speculative connections that may never send a request; each one gets this long.
const REQUEST_HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST_HEAD: usize = 16 * 1024;

impl LoopbackReceiver {
    /// Binds `127.0.0.1:0`; `redirect_uri` is `http://127.0.0.1:<port>/auth/callback`.
    pub async fn bind() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| {
                Error::Network(format!("could not open a loopback port for sign-in: {e}"))
            })?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::Network(format!("could not read the loopback port: {e}")))?
            .port();
        Ok(LoopbackReceiver {
            redirect_uri: format!("http://127.0.0.1:{port}/auth/callback"),
            listener,
        })
    }

    /// Waits for one request to `/auth/callback`, answers it with a small HTML page
    /// ("You can close this window and return to Ring."), and returns the parsed callback.
    pub async fn wait(self) -> Result<Callback> {
        let (tx, mut rx) = mpsc::channel::<Callback>(1);
        loop {
            tokio::select! {
                Some(callback) = rx.recv() => return Ok(callback),
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted.map_err(|e| Error::Network(format!("sign-in listener failed: {e}")))?;
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        if let Some(callback) = serve(stream).await {
                            let _ = tx.send(callback).await;
                        }
                    });
                }
            }
        }
    }
}

async fn serve(mut stream: TcpStream) -> Option<Callback> {
    let head = tokio::time::timeout(REQUEST_HEAD_TIMEOUT, read_head(&mut stream))
        .await
        .ok()??;
    let request_line = head.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    if method != "GET" {
        respond(
            &mut stream,
            "405 Method Not Allowed",
            &page("Ring", "This address only accepts the sign-in redirect."),
        )
        .await;
        return None;
    }
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        respond(
            &mut stream,
            "400 Bad Request",
            &page("Ring", "That request could not be read."),
        )
        .await;
        return None;
    };
    if url.path() != "/auth/callback" {
        respond(&mut stream, "404 Not Found", &page("Ring", "Nothing here.")).await;
        return None;
    }
    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    if let Some(error) = params.get("error") {
        let cancelled = error == "access_denied";
        let body = if cancelled {
            page(
                "Sign-in cancelled",
                "You can close this window and return to Ring.",
            )
        } else {
            page(
                "Sign-in did not finish",
                "You can close this window and return to Ring to try again.",
            )
        };
        respond(&mut stream, "200 OK", &body).await;
        return Some(Callback::Error {
            error: error.clone(),
            description: params.get("error_description").cloned(),
        });
    }
    match (params.get("code"), params.get("state")) {
        (Some(code), Some(state)) => {
            respond(
                &mut stream,
                "200 OK",
                &page(
                    "You're signed in",
                    "You can close this window and return to Ring.",
                ),
            )
            .await;
            Some(Callback::Code {
                code: code.clone(),
                state: state.clone(),
            })
        }
        _ => {
            respond(
                &mut stream,
                "400 Bad Request",
                &page("Ring", "The sign-in redirect was missing its code."),
            )
            .await;
            None
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            return String::from_utf8(buffer).ok();
        }
        if buffer.len() > MAX_REQUEST_HEAD {
            return None;
        }
    }
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// A small page in Ring's paper-and-ink style. Both strings are static, never request data.
fn page(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title} · Ring</title>\
         <style>:root{{color-scheme:light dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;\
         background:#FFFDF9;color:#1E1E1C;font:16px/1.5 -apple-system,system-ui,sans-serif}}\
         main{{max-width:28rem;padding:2rem;text-align:center}}h1{{font:400 1.75rem/1.2 Georgia,serif;margin:0 0 .5rem}}\
         p{{color:#4A4842;margin:0}}small{{display:block;margin-top:2rem;letter-spacing:.2em;font-size:.7rem;color:#6E6B63}}\
         @media (prefers-color-scheme:dark){{body{{background:#181818;color:#EDE8E0}}p{{color:#C9C3B6}}small{{color:#9C978C}}}}\
         </style></head><body><main><h1>{title}</h1><p>{message}</p><small>RING</small></main></body></html>"
    )
}
