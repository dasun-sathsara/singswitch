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

/// Build the request URL from `external_controller`.
///
/// The controller is untrusted profile input, so the shape is pinned here:
/// `host:port` or `scheme://host:port`, nothing else. A path of its own would
/// let `#` swallow the appended `/proxies` and a `..` would rewrite it, both
/// of which redirect the selector PUT at an arbitrary endpoint; a non-loopback
/// host would send the profile's own bearer secret to a host the user never
/// chose. No URL crate is used: the accepted shape is small enough to check
/// by character class, and rejecting everything else is the point.
fn api_url(controller: &str, path: &str) -> Result<String> {
    let trimmed = controller.trim().trim_end_matches('/');
    let (scheme, rest) = if let Some(rest) = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
    {
        ("http", rest)
    } else {
        ("http", trimmed)
    };
    let host_port = rest.trim_start_matches("//");
    if !is_loopback_authority(host_port) {
        return Err(unusable(controller));
    }
    Ok(format!("{scheme}://{host_port}{path}"))
}

fn unusable(controller: &str) -> SingSwitchError {
    SingSwitchError::general(format!("{controller:?} is not a usable Clash API address"))
        .with_hint("set experimental.clash_api.external_controller to 127.0.0.1:9090")
}

/// `host:port` for a loopback host, with `host` optionally bracketed for IPv6
/// and a port that is all digits. Anything with a `/`, `?`, `#`, whitespace, or
/// `..` fails, which is what keeps the appended request path intact.
fn is_loopback_authority(host_port: &str) -> bool {
    if host_port.is_empty()
        || host_port.contains(['/', '?', '#', '@', ' ', '\t'])
        || host_port.contains("..")
        || host_port.starts_with(':')
    {
        return false;
    }
    let Some((host, port)) = host_port.rsplit_once(':') else {
        return false;
    };
    if host.is_empty()
        || port.is_empty()
        || !port.bytes().all(|b| b.is_ascii_digit())
        || port.len() > 5
        || port.parse::<u16>().is_err()
    {
        return false;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "localhost" | "::1") || host == "127.0.0.1" || host.starts_with("127.")
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
