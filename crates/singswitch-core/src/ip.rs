use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Result, SingSwitchError};

/// Lookup service reporting public egress IP, ASN, and geolocation.
pub const IP_INFO_URL: &str = "https://ipinfo.io/json";
const MAX_IP_INFO_BYTES: usize = 1024 * 1024;
const IP_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

static IP_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(IP_LOOKUP_TIMEOUT))
        .user_agent("singswitch")
        .tls_config(crate::net::tls_config())
        .build()
        .new_agent()
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpInfo {
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub country: String,
    #[serde(default, rename = "loc")]
    pub location: String,
    #[serde(default, rename = "org")]
    pub network: String,
    #[serde(default)]
    pub postal: String,
    #[serde(default)]
    pub timezone: String,
}

impl IpInfo {
    /// Formatted `City, Region, Country` display string.
    /// Human-readable place for the sample, without repetition.
    ///
    /// ipinfo often returns the same value for city and region (Singapore,
    /// Singapore), and the country code merely repeats it, so duplicates are
    /// dropped and the country code is only used when nothing else names the
    /// place.
    pub fn formatted_location(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for value in [&self.city, &self.region, &self.country] {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if parts.iter().any(|seen| seen.eq_ignore_ascii_case(value)) {
                continue;
            }
            parts.push(value);
        }
        // Drop a trailing two-letter country code when a full name precedes it.
        if parts.len() > 1 && parts.last().is_some_and(|c| c.len() == 2) {
            parts.pop();
        }
        parts.join(", ")
    }
}

/// Fetch public IP details from `https://ipinfo.io/json` (or an override URL in tests).
pub fn fetch_ip_info(url: Option<&str>) -> Result<IpInfo> {
    let target = url.unwrap_or(IP_INFO_URL);
    let resp = IP_AGENT
        .get(target)
        .header("Accept", "application/json")
        .call()
        .map_err(|err| {
            SingSwitchError::general(format!("could not reach the IP lookup service: {err}"))
                .with_hints([
                    "check that you have a working connection",
                    "if sing-box is running, inspect its output with: singswitch logs",
                ])
                .with_cause(err)
        })?;

    let mut body = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_IP_INFO_BYTES as u64)
        .read_to_end(&mut body)?;

    serde_json::from_slice(&body).map_err(|err| {
        SingSwitchError::general("the IP lookup service returned something unreadable")
            .with_cause(err)
    })
}
