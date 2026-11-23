use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use crate::checker::{Checker, check_temp_bytes};
use crate::error::{ExitCode, Result, SingSwitchError};

/// Cap remote profile downloads at 8 MiB so an accidental HTML/binary URL cannot exhaust memory.
pub const MAX_REMOTE_PROFILE_BYTES: usize = 8 * 1024 * 1024;

/// Total HTTP timeout covering connection, TLS, and body transfer.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

static REMOTE_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .user_agent("singswitch")
        .tls_config(crate::net::tls_config())
        .build()
        .new_agent()
});

/// Validate that a profile source URL is an absolute `http://` or `https://` URL with a host.
pub fn normalise_profile_url(raw_url: &str) -> Result<String> {
    let trimmed = raw_url.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        let orig = &trimmed[8..];
        (r, orig)
    } else if let Some(r) = lower.strip_prefix("http://") {
        let orig = &trimmed[7..];
        (r, orig)
    } else {
        return Err(SingSwitchError::general(format!(
            "{trimmed:?} is not a valid http(s) profile URL"
        ))
        .with_hint("use the full https:// address of the raw configuration file"));
    };

    let host_part = rest.0.split(['/', '?', '#']).next().unwrap_or("");
    if host_part.is_empty() || host_part.contains(char::is_whitespace) || rest.1.is_empty() {
        return Err(SingSwitchError::general(format!(
            "{trimmed:?} is not a valid http(s) profile URL"
        ))
        .with_hint("use the full https:// address of the raw configuration file"));
    }

    Ok(trimmed.to_string())
}

/// Download a remote sing-box profile over HTTP(S), enforcing size and timeout limits.
pub fn fetch_remote_profile(raw_url: &str) -> Result<Vec<u8>> {
    let url = normalise_profile_url(raw_url)?;
    let response = REMOTE_AGENT
        .get(&url)
        .header("Accept", "application/json, */*")
        .call()
        .map_err(|err| match err {
            ureq::Error::StatusCode(code) => SingSwitchError::general(format!(
                "could not download {url}: server replied HTTP {code}"
            ))
            .with_hint("check the URL points at the raw file, not a preview page"),
            other => SingSwitchError::general(format!("could not download {url}: {other}"))
                .with_hint("check the URL and that this machine is online")
                .with_cause(other),
        })?;

    let mut body = Vec::new();
    let mut reader = response
        .into_body()
        .into_reader()
        .take((MAX_REMOTE_PROFILE_BYTES + 1) as u64);
    reader.read_to_end(&mut body).map_err(|err| {
        SingSwitchError::general(format!("could not download {url}: {err}"))
            .with_hint("check the URL and that this machine is online")
    })?;

    if body.len() > MAX_REMOTE_PROFILE_BYTES {
        return Err(SingSwitchError::general(format!(
            "refusing {url}: larger than {} MiB, not a sing-box profile",
            MAX_REMOTE_PROFILE_BYTES >> 20
        ))
        .with_hint("check the URL points at the raw file, not a preview page"));
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(SingSwitchError::general(format!(
            "refusing {url}: the server returned an empty file"
        ))
        .with_hint("check the URL points at the raw file, not a preview page"));
    }
    Ok(body)
}

/// Validate downloaded profile bytes with `sing-box check` via a temp file without touching
/// the stored profile directory.
pub fn validate_remote_body(checker: &dyn Checker, body: &[u8]) -> Result<()> {
    if let Err(err) = check_temp_bytes(checker, "singswitch-remote-", body) {
        return Err(SingSwitchError::new(
            ExitCode::Validation,
            "the downloaded file is not a valid sing-box configuration",
        )
        .with_cause(err.to_string())
        .with_hint("check the URL points at the raw file, not a preview page"));
    }
    Ok(())
}
