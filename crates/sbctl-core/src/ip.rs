use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Result, SbctlError};

/// Lookup service reporting public egress IP, ASN, and geolocation.
pub const IP_INFO_URL: &str = "https://ipinfo.io/json";
const MAX_IP_INFO_BYTES: usize = 1024 * 1024;
const IP_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

static IP_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(IP_LOOKUP_TIMEOUT))
        .user_agent("sbctl")
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
    pub fn formatted_location(&self) -> String {
        [&self.city, &self.region, &self.country]
            .into_iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
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
            SbctlError::general(format!("could not reach the IP lookup service: {err}"))
                .with_hints([
                    "check that you have a working connection",
                    "if sing-box is running, inspect its output with: sbctl logs",
                ])
                .with_cause(err)
        })?;

    let mut body = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_IP_INFO_BYTES as u64)
        .read_to_end(&mut body)?;

    serde_json::from_slice(&body).map_err(|err| {
        SbctlError::general("the IP lookup service returned something unreadable").with_cause(err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipinfo_json_and_formats_location() {
        let raw = r#"{
            "ip": "13.215.132.32",
            "city": "Singapore",
            "region": "Central Singapore",
            "country": "SG",
            "loc": "1.2897,103.8501",
            "org": "AS16509 Amazon.com, Inc.",
            "postal": "018989",
            "timezone": "Asia/Singapore"
        }"#;
        let info: IpInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(info.ip, "13.215.132.32");
        assert_eq!(info.network, "AS16509 Amazon.com, Inc.");
        assert_eq!(
            info.formatted_location(),
            "Singapore, Central Singapore, SG"
        );
    }
}
