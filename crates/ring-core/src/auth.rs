//! Carbon sign-in helpers for desktop apps (RFC 8252 loopback + PKCE). See `udd/accounts.md`.
//!
//! SKELETON: signatures are the contract; the ring-core owner implements them.

use crate::Result;

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
        todo!()
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds the hosted Accounts sign-in URL:
/// `{accounts_url}/authorize?response_type=code&app_id=ring&redirect_uri=…&scope=profile&state=…&code_challenge=…&code_challenge_method=S256`.
pub fn authorize_url(accounts_url: &str, redirect_uri: &str, pkce: &Pkce) -> String {
    let _ = (accounts_url, redirect_uri, pkce);
    todo!()
}

/// A loopback listener on `127.0.0.1:<random port>` that receives the Accounts redirect.
pub struct LoopbackReceiver {
    pub redirect_uri: String,
    pub(crate) listener: tokio::net::TcpListener,
}

/// The authorization result delivered to the redirect URI.
#[derive(Clone, Debug)]
pub enum Callback {
    Code { code: String, state: String },
    Error { error: String, description: Option<String> },
}

impl LoopbackReceiver {
    /// Binds `127.0.0.1:0`; `redirect_uri` is `http://127.0.0.1:<port>/auth/callback`.
    pub async fn bind() -> Result<Self> {
        todo!()
    }

    /// Waits for one request to `/auth/callback`, answers it with a small HTML page
    /// ("You can close this window and return to Ring."), and returns the parsed callback.
    pub async fn wait(self) -> Result<Callback> {
        let _ = self;
        todo!()
    }
}
