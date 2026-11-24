//! Live switching for Clash-style `selector`/`urltest` outbound groups.
//!
//! A sing-box profile can route through a `selector` outbound, but sing-box has
//! no CLI for changing which member a selector points at: only the experimental
//! Clash API exposes that, at `/proxies`. Everything here is opt-in on configs
//! that already declare `experimental.clash_api`; the app must never inject that
//! block itself, because a rewritten profile is what the sync path uploads back.
//!
//! Functions are blocking (`ureq`); callers run them on their own executor.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::error::{Result, SingSwitchError};

/// Local daemon calls: long enough for a busy machine, short enough that the
/// window never waits on a wedged API.
pub const CLASH_API_TIMEOUT: Duration = Duration::from_secs(5);

/// A `/proxies` body is a few KiB. Cap it so a misconfigured controller that
/// answers with something else cannot exhaust memory.
const MAX_CLASH_BODY_BYTES: usize = 4 * 1024 * 1024;

static CLASH_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(CLASH_API_TIMEOUT))
        .user_agent("singswitch")
        // The controller is normally loopback HTTP; TLS matters only when a user
        // points external_controller at something behind a proxy.
        .tls_config(crate::net::tls_config())
        .build()
        .new_agent()
});

/// Which Clash group type a member list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// User-picked member; the API can be told to change it.
    Selector,
    /// Member chosen by the daemon's own latency probes; read-only.
    Urltest,
}

impl GroupKind {
    /// Parse sing-box's config spelling (`selector`, `urltest`).
    fn from_config_type(raw: &str) -> Option<Self> {
        match raw {
            "selector" => Some(Self::Selector),
            "urltest" => Some(Self::Urltest),
            _ => None,
        }
    }

    /// Parse the Clash API spelling (`Selector`, `URLTest`), which differs in case
    /// and hyphenation from the config.
    fn from_api_type(raw: &str) -> Option<Self> {
        let lower = raw.to_ascii_lowercase();
        match lower.as_str() {
            "selector" => Some(Self::Selector),
            "urltest" | "url-test" | "url_test" => Some(Self::Urltest),
            _ => None,
        }
    }
}

/// One switchable outbound group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub tag: String,
    pub kind: GroupKind,
    /// Members in the order the config (or the daemon) declared them.
    pub members: Vec<String>,
    /// Live selection from the Clash API; `None` until one has been fetched.
    pub selected: Option<String>,
}

impl GroupInfo {
    /// Whether the user is allowed to change the selection.
    pub fn is_selectable(&self) -> bool {
        self.kind == GroupKind::Selector
    }
}

/// Where the Clash API lives, as declared by the active config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClashEndpoint {
    /// `host:port`, or a full URL when the user wrote one.
    pub controller: String,
    pub secret: Option<String>,
}

/// Groups declared by a config, in declaration order.
///
/// Members that do not match any outbound are still reported: resolving them is
/// the daemon's job, and hiding a group because one member is stale would hide
/// the switch the user is looking for.
pub fn find_groups(config: &Value) -> Vec<GroupInfo> {
    let Some(outbounds) = config.get("outbounds").and_then(Value::as_array) else {
        return Vec::new();
    };
    outbounds
        .iter()
        .filter_map(|outbound| {
            let object = outbound.as_object()?;
            let kind = GroupKind::from_config_type(object.get("type")?.as_str()?)?;
            let tag = object.get("tag").and_then(Value::as_str)?;
            if tag.is_empty() {
                return None;
            }
            let members = object
                .get("outbounds")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .filter(|member| !member.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(GroupInfo {
                tag: tag.to_string(),
                kind,
                members,
                selected: None,
            })
        })
        .collect()
}

/// Read `experimental.clash_api.external_controller`, when a config opts in.
pub fn has_clash_api(config: &Value) -> Option<String> {
    let controller = config
        .get("experimental")?
        .get("clash_api")?
        .get("external_controller")?
        .as_str()?
        .trim();
    (!controller.is_empty()).then(|| controller.to_string())
}

/// Read `experimental.clash_api.secret`, when the config sets one.
pub fn clash_api_secret(config: &Value) -> Option<String> {
    let secret = config
        .get("experimental")?
        .get("clash_api")?
        .get("secret")?
        .as_str()?
        .trim();
    (!secret.is_empty()).then(|| secret.to_string())
}

impl ClashEndpoint {
    /// Endpoint declared by a config, or `None` when the config does not opt in.
    pub fn from_config(config: &Value) -> Option<Self> {
        Some(Self {
            controller: has_clash_api(config)?,
            secret: clash_api_secret(config),
        })
    }
}

/// Parse a JSON config from disk.
pub fn read_config(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).map_err(|err| {
        SingSwitchError::general(format!(
            "could not read the active config at {}: {err}",
            path.display()
        ))
        .with_hint("activate a profile, then check the service is started")
    })?;
    serde_json::from_str(&text).map_err(|err| {
        SingSwitchError::general(format!(
            "the active config at {} is not valid JSON: {err}",
            path.display()
        ))
        .with_hint("fix the profile, then run: singswitch check <name>")
    })
}

/// Groups declared by the config currently installed for the daemon.
///
/// Best effort: a missing or unreadable config yields no groups, which is the
/// same state as a config that declares none.
pub fn active_groups_from_disk(active_config: &Path) -> Vec<GroupInfo> {
    read_config(active_config)
        .map(|config| find_groups(&config))
        .unwrap_or_default()
}

/// Fold the daemon's live view into the config's declaration order.
///
/// The config decides which groups exist and in what order, because that is the
/// order the user wrote. The daemon decides members (it drops tags that do not
/// resolve) and the selection.
pub fn merge_live(declared: Vec<GroupInfo>, live: &[GroupInfo]) -> Vec<GroupInfo> {
    declared
        .into_iter()
        .map(|mut group| {
            if let Some(live_group) = live.iter().find(|candidate| candidate.tag == group.tag) {
                group.selected = live_group.selected.clone();
                if !live_group.members.is_empty() {
                    group.members = live_group.members.clone();
                }
            }
            group
        })
        .collect()
}

/// Ask the daemon which member each group currently points at.
///
/// Blocking; callers run it on a background executor.
pub fn fetch_groups(controller: &str, secret: Option<&str>) -> Result<Vec<GroupInfo>> {
    let url = api_url(controller, "/proxies")?;
    let mut request = CLASH_AGENT.get(&url).header("Accept", "application/json");
    if let Some(secret) = secret {
        request = request.header("Authorization", &format!("Bearer {secret}"));
    }

    let response = request
        .call()
        .map_err(|err| api_error(err, &url, "read the proxy groups"))?;
    let body = response
        .into_body()
        .with_config()
        .limit(u64::try_from(MAX_CLASH_BODY_BYTES).unwrap_or(u64::MAX))
        .read_to_string()
        .map_err(|err| {
            SingSwitchError::general(format!("could not read {url}: {err}"))
                .with_hint("check experimental.clash_api.external_controller points at sing-box")
        })?;
    let value: Value = serde_json::from_str(&body).map_err(|err| {
        SingSwitchError::general(format!("{url} did not return JSON: {err}")).with_hint(
            "check external_controller points at the sing-box Clash API, not another service",
        )
    })?;

    Ok(parse_proxies(&value))
}

/// Map a `/proxies` payload onto group infos, sorted by tag so the UI order is
/// stable: the daemon's object order is not.
fn parse_proxies(value: &Value) -> Vec<GroupInfo> {
    let Some(proxies) = value.get("proxies").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut groups: Vec<GroupInfo> = proxies
        .iter()
        .filter_map(|(tag, proxy): (&String, &Value)| {
            let object = proxy.as_object()?;
            let kind = GroupKind::from_api_type(object.get("type")?.as_str()?)?;
            Some(GroupInfo {
                tag: tag.clone(),
                kind,
                members: member_list(object),
                selected: object
                    .get("now")
                    .and_then(Value::as_str)
                    .filter(|now| !now.is_empty())
                    .map(str::to_string),
            })
        })
        .collect();
    groups.sort_by(|a, b| a.tag.cmp(&b.tag));
    groups
}

fn member_list(object: &Map<String, Value>) -> Vec<String> {
    object
        .get("all")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .filter(|member| !member.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Point a group at another member.
///
/// Blocking; callers run it on a background executor.
pub fn select_group_member(
    controller: &str,
    secret: Option<&str>,
    group: &str,
    member: &str,
) -> Result<()> {
    if group.trim().is_empty() {
        return Err(SingSwitchError::general("no group to switch")
            .with_hint("reload the window and pick a group again"));
    }
    let path = format!("/proxies/{}", encode_path_segment(group));
    let url = api_url(controller, &path)?;
    let body = serde_json::json!({ "name": member }).to_string();

    let mut request = CLASH_AGENT
        .put(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    if let Some(secret) = secret {
        request = request.header("Authorization", &format!("Bearer {secret}"));
    }

    request
        .send(body.as_str())
        .map_err(|err| api_error(err, &url, &format!("switch {group} to {member}")))?;
    Ok(())
}

/// Build the request URL from `external_controller`, which is documented as
/// `host:port` but is sometimes written with a scheme or a trailing slash.
fn api_url(controller: &str, path: &str) -> Result<String> {
    let trimmed = controller.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return Err(SingSwitchError::general(format!(
            "{controller:?} is not a usable Clash API address"
        ))
        .with_hint("set experimental.clash_api.external_controller to 127.0.0.1:9090"));
    }
    let base = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    Ok(format!("{base}{path}"))
}

fn api_error(err: ureq::Error, url: &str, action: &str) -> SingSwitchError {
    match err {
        ureq::Error::StatusCode(401 | 403) => {
            SingSwitchError::general(format!("the Clash API refused to let singswitch {action}"))
                .with_hint(
                    "copy experimental.clash_api.secret from the profile, or remove it to allow local access",
                )
        }
        ureq::Error::StatusCode(404) => {
            SingSwitchError::general(format!("the Clash API at {url} has no such group"))
                .with_hint("reload the window: the profile may have changed since it was read")
        }
        ureq::Error::StatusCode(code) => {
            SingSwitchError::general(format!("could not {action}: the Clash API replied HTTP {code}"))
                .with_hint("a url-test group picks its own member and cannot be switched")
        }
        other => SingSwitchError::general(format!("could not {action} at {url}: {other}"))
            .with_hint(
                "the Clash API only listens while sing-box runs; start the service and try again",
            )
            .with_cause(other),
    }
}

/// Percent-encode one path segment.
///
/// Group tags are user-chosen and routinely contain spaces and slashes
/// (`Proxy HK / 01`), which would otherwise change the request path. Hand-rolled
/// because a whole crate for one RFC 3986 rule is not worth the build time.
fn encode_path_segment(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => {
                out.push('%');
                out.push(char::from(HEX[usize::from(other >> 4)]));
                out.push(char::from(HEX[usize::from(other & 0x0F)]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config_with_groups() -> Value {
        json!({
            "outbounds": [
                { "type": "selector", "tag": "proxy", "outbounds": ["hk-01", "jp-02"], "default": "hk-01" },
                { "type": "urltest", "tag": "auto", "outbounds": ["hk-01", "jp-02"] },
                { "type": "vless", "tag": "hk-01", "server": "a.example" },
                { "type": "vless", "tag": "jp-02", "server": "b.example" }
            ],
            "experimental": {
                "clash_api": { "external_controller": "127.0.0.1:9090", "secret": "s3cret" }
            }
        })
    }

    #[test]
    fn find_groups_reports_selector_and_urltest_in_order() {
        let groups = find_groups(&config_with_groups());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].tag, "proxy");
        assert_eq!(groups[0].kind, GroupKind::Selector);
        assert!(groups[0].is_selectable());
        assert_eq!(groups[0].members, vec!["hk-01", "jp-02"]);
        assert_eq!(groups[0].selected, None);
        assert_eq!(groups[1].tag, "auto");
        assert_eq!(groups[1].kind, GroupKind::Urltest);
        assert!(!groups[1].is_selectable());
    }

    #[test]
    fn find_groups_skips_untagged_and_memberless_outbounds() {
        let config = json!({
            "outbounds": [
                { "type": "selector", "outbounds": ["a"] },
                { "type": "selector", "tag": "", "outbounds": ["a"] },
                { "type": "selector", "tag": "empty" },
                { "type": "direct", "tag": "direct" }
            ]
        });
        let groups = find_groups(&config);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tag, "empty");
        assert!(groups[0].members.is_empty());
    }

    #[test]
    fn find_groups_without_outbounds_is_empty() {
        assert!(find_groups(&json!({ "log": { "level": "info" } })).is_empty());
        assert!(find_groups(&json!([])).is_empty());
    }

    #[test]
    fn find_groups_reports_members_that_do_not_exist() {
        let config = json!({
            "outbounds": [
                { "type": "selector", "tag": "proxy", "outbounds": ["gone-01", "gone-02"] }
            ]
        });
        let groups = find_groups(&config);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members, vec!["gone-01", "gone-02"]);
    }

    #[test]
    fn has_clash_api_needs_a_non_empty_controller() {
        assert_eq!(
            has_clash_api(&config_with_groups()).as_deref(),
            Some("127.0.0.1:9090")
        );
        assert_eq!(
            clash_api_secret(&config_with_groups()).as_deref(),
            Some("s3cret")
        );

        let endpoint = ClashEndpoint::from_config(&config_with_groups());
        assert_eq!(
            endpoint,
            Some(ClashEndpoint {
                controller: "127.0.0.1:9090".to_string(),
                secret: Some("s3cret".to_string())
            })
        );
    }

    #[test]
    fn has_clash_api_absent_when_experimental_is_missing_or_empty() {
        assert_eq!(has_clash_api(&json!({ "outbounds": [] })), None);
        assert_eq!(
            has_clash_api(
                &json!({ "experimental": { "clash_api": { "external_controller": "  " } } })
            ),
            None
        );
        assert_eq!(
            has_clash_api(
                &json!({ "experimental": { "clash_api": { "external_controller": 9090 } } })
            ),
            None
        );
        assert_eq!(ClashEndpoint::from_config(&json!({})), None);
    }

    #[test]
    fn parse_proxies_maps_selector_and_urltest_only() {
        let value = json!({
            "proxies": {
                "proxy": { "type": "Selector", "now": "jp-02", "all": ["hk-01", "jp-02"] },
                "auto": { "type": "URLTest", "now": "hk-01", "all": ["hk-01", "jp-02"] },
                "hk-01": { "type": "VLESS", "history": [] },
                "reject": { "type": "Reject" }
            }
        });
        let groups = parse_proxies(&value);
        assert_eq!(groups.len(), 2);
        // Sorted by tag, not by the daemon's object order.
        assert_eq!(groups[0].tag, "auto");
        assert_eq!(groups[0].kind, GroupKind::Urltest);
        assert_eq!(groups[0].selected.as_deref(), Some("hk-01"));
        assert_eq!(groups[1].tag, "proxy");
        assert_eq!(groups[1].kind, GroupKind::Selector);
        assert_eq!(groups[1].selected.as_deref(), Some("jp-02"));
        assert_eq!(groups[1].members, vec!["hk-01", "jp-02"]);
    }

    #[test]
    fn parse_proxies_tolerates_a_wrong_shape() {
        assert!(parse_proxies(&json!({})).is_empty());
        assert!(parse_proxies(&json!({ "proxies": [] })).is_empty());
        assert!(
            parse_proxies(&json!({ "proxies": { "proxy": { "type": "Selector" } } }))[0]
                .members
                .is_empty()
        );
    }

    #[test]
    fn merge_live_keeps_config_order_and_takes_the_live_selection() {
        let declared = find_groups(&config_with_groups());
        let live = parse_proxies(&json!({
            "proxies": {
                "auto": { "type": "URLTest", "now": "jp-02", "all": ["jp-02"] },
                "proxy": { "type": "Selector", "now": "jp-02", "all": ["hk-01", "jp-02"] }
            }
        }));
        let merged = merge_live(declared, &live);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].tag, "proxy");
        assert_eq!(merged[0].selected.as_deref(), Some("jp-02"));
        assert_eq!(merged[1].tag, "auto");
        assert_eq!(merged[1].selected.as_deref(), Some("jp-02"));
        // A member the daemon dropped is gone; the rest of the config wins.
        assert_eq!(merged[1].members, vec!["jp-02"]);
    }

    #[test]
    fn merge_live_leaves_groups_the_daemon_did_not_report_unselected() {
        let declared = find_groups(&json!({
            "outbounds": [{ "type": "selector", "tag": "proxy", "outbounds": ["a", "b"] }]
        }));
        let merged = merge_live(declared, &[]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].selected, None);
        assert_eq!(merged[0].members, vec!["a", "b"]);
    }

    #[test]
    fn api_url_accepts_host_port_and_full_urls() {
        assert_eq!(
            api_url("127.0.0.1:9090", "/proxies").unwrap_or_default(),
            "http://127.0.0.1:9090/proxies"
        );
        assert_eq!(
            api_url(" 127.0.0.1:9090/ ", "/proxies").unwrap_or_default(),
            "http://127.0.0.1:9090/proxies"
        );
        assert_eq!(
            api_url("https://clash.example:9090", "/proxies").unwrap_or_default(),
            "https://clash.example:9090/proxies"
        );
        assert!(api_url("", "/proxies").is_err());
        assert!(api_url("host with space", "/proxies").is_err());
    }

    #[test]
    fn encode_path_segment_escapes_everything_reserved() {
        assert_eq!(encode_path_segment("proxy"), "proxy");
        assert_eq!(
            encode_path_segment("Proxy HK / 01"),
            "Proxy%20HK%20%2F%2001"
        );
        assert_eq!(encode_path_segment("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode_path_segment("日本"), "%E6%97%A5%E6%9C%AC");
        assert_eq!(encode_path_segment("50%"), "50%25");
    }
}
