//! Shared HTTPS client settings.

use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// TLS settings for every outbound HTTP client.
///
/// `ureq` defaults to its own rustls provider and bundled roots. This crate does
/// not build that backend, and profiles routinely come from hosts behind a
/// corporate proxy or a private CA, so the provider and the trust store both
/// come from the operating system.
pub fn tls_config() -> TlsConfig {
    TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build()
}
