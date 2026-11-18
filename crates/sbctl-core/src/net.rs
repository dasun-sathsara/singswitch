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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_clients_use_the_platform_tls_stack() {
        let config = tls_config();
        assert_eq!(config.provider(), TlsProvider::NativeTls);
        assert!(matches!(config.root_certs(), RootCerts::PlatformVerifier));
    }
}
