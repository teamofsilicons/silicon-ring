//! Shared rustls configuration for HTTPS and WSS.

use std::sync::{Arc, OnceLock};

use rustls::ClientConfig;

fn base() -> ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Config for the HTTP client: negotiates HTTP/2 when the server offers it.
pub(crate) fn http() -> ClientConfig {
    static CONFIG: OnceLock<ClientConfig> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut config = base();
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            config
        })
        .clone()
}

/// Config for WebSockets, which must stay on HTTP/1.1 for the upgrade.
pub(crate) fn websocket() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| Arc::new(base())).clone()
}
