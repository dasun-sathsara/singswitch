use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use crate::checker::Checker;
use crate::error::{ExitCode, Result, SbctlError};

/// Cap remote profile downloads at 8 MiB so an accidental HTML/binary URL cannot exhaust memory.
pub const MAX_REMOTE_PROFILE_BYTES: usize = 8 * 1024 * 1024;

/// Total HTTP timeout covering connection, TLS, and body transfer.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

static REMOTE_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .user_agent("sbctl")
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
        return Err(
            SbctlError::general(format!("{trimmed:?} is not a valid http(s) profile URL"))
                .with_hint("use the full https:// address of the raw configuration file"),
        );
    };

    let host_part = rest.0.split(['/', '?', '#']).next().unwrap_or("");
    if host_part.is_empty() || host_part.contains(char::is_whitespace) || rest.1.is_empty() {
        return Err(
            SbctlError::general(format!("{trimmed:?} is not a valid http(s) profile URL"))
                .with_hint("use the full https:// address of the raw configuration file"),
        );
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
            ureq::Error::StatusCode(code) => SbctlError::general(format!(
                "could not download {url}: server replied HTTP {code}"
            ))
            .with_hint("check the URL points at the raw file, not a preview page"),
            other => SbctlError::general(format!("could not download {url}: {other}"))
                .with_hint("check the URL and that this machine is online")
                .with_cause(other),
        })?;

    let mut body = Vec::new();
    let mut reader = response
        .into_body()
        .into_reader()
        .take((MAX_REMOTE_PROFILE_BYTES + 1) as u64);
    reader.read_to_end(&mut body).map_err(|err| {
        SbctlError::general(format!("could not download {url}: {err}"))
            .with_hint("check the URL and that this machine is online")
    })?;

    if body.len() > MAX_REMOTE_PROFILE_BYTES {
        return Err(SbctlError::general(format!(
            "refusing {url}: larger than {} MiB, not a sing-box profile",
            MAX_REMOTE_PROFILE_BYTES >> 20
        ))
        .with_hint("check the URL points at the raw file, not a preview page"));
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(SbctlError::general(format!(
            "refusing {url}: the server returned an empty file"
        ))
        .with_hint("check the URL points at the raw file, not a preview page"));
    }
    Ok(body)
}

/// Validate downloaded profile bytes with `sing-box check` via a temp file without touching
/// the stored profile directory.
pub fn validate_remote_body(checker: &dyn Checker, body: &[u8]) -> Result<()> {
    let mut tmp = tempfile::Builder::new()
        .prefix("sbctl-remote-")
        .suffix(".json")
        .tempfile()?;
    std::io::Write::write_all(&mut tmp, body)?;
    std::io::Write::flush(&mut tmp)?;

    if let Err(err) = checker.check(tmp.path()) {
        return Err(SbctlError::new(
            ExitCode::Validation,
            "the downloaded file is not a valid sing-box configuration",
        )
        .with_cause(err.to_string())
        .with_hint("check the URL points at the raw file, not a preview page"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn normalise_url_rejects_file_and_empty_hosts() {
        assert!(normalise_profile_url("file:///etc/passwd").is_err());
        assert!(normalise_profile_url("ftp://example.com/a.json").is_err());
        assert!(normalise_profile_url("https://").is_err());
        assert_eq!(
            normalise_profile_url("  https://gist.github.com/raw/a.json ").unwrap(),
            "https://gist.github.com/raw/a.json"
        );
    }

    #[test]
    fn fetch_remote_profile_reads_local_http_and_enforces_empty_or_404() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut req_buf = [0u8; 1024];
                let n = stream.read(&mut req_buf).unwrap();
                let req_str = String::from_utf8_lossy(&req_buf[..n]);
                if req_str.starts_with("GET /ok.json") {
                    let body = r#"{"log":{"level":"info"}}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(resp.as_bytes()).unwrap();
                } else {
                    let resp =
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    stream.write_all(resp.as_bytes()).unwrap();
                }
            }
        });

        let ok_url = format!("http://{addr}/ok.json");
        let bytes = fetch_remote_profile(&ok_url).expect("ok fetch");
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"log":{"level":"info"}}"#
        );

        let missing_url = format!("http://{addr}/missing.json");
        assert!(fetch_remote_profile(&missing_url).is_err());

        handle.join().unwrap();
    }
}
