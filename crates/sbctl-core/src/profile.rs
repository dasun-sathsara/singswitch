use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, SbctlError};
use crate::form_model::ProfileFormModel;
use crate::source::read_source;

/// Required filename extension for sing-box profiles.
pub const EXTENSION: &str = ".json";

/// Maximum profile name length keeping filenames portable across filesystems.
const MAX_NAME_LENGTH: usize = 64;

/// Canonical seed template embedded into the binary.
pub const SKELETON_JSON: &str = include_str!("../../../assets/skeleton.json");

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Summary of a stored sing-box profile file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub path: PathBuf,
    /// Fields the user still has to fill in before this can connect.
    ///
    /// Computed from the parsed config, not from scanning the raw text.
    pub missing: Vec<crate::readiness::Field>,
    pub server: Option<String>,
    pub tun: Option<String>,
    pub source_url: Option<String>,
    /// Whether the config parsed at all. Distinguishes "no outbound declares a
    /// server" from "this file is not valid JSON".
    pub config_parsed: bool,
    /// Facts parsed from the config this profile was built from.
    ///
    /// Carried here so rendering never re-reads and re-parses the file: that
    /// happened once per frame on the Profiles stage before.
    pub facts: ConfigFacts,
}

impl Profile {
    /// Whether this profile can connect.
    ///
    /// Requires that the file parsed at all: an unreadable or unparseable file
    /// used to report "ready" because the marker scan found nothing in it.
    pub fn ready(&self) -> bool {
        self.config_parsed && self.missing.is_empty()
    }
}

/// Validate a profile name against traversal, flag confusion, and Windows DOS device names.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(SbctlError::invalid_name("name must not be empty"));
    }
    if name.len() > MAX_NAME_LENGTH {
        return Err(SbctlError::invalid_name(format!(
            "{name:?} is longer than {MAX_NAME_LENGTH} characters"
        )));
    }
    if name == "." || name == ".." {
        return Err(SbctlError::invalid_name(format!(
            "{name:?} is a directory reference"
        )));
    }
    if name.starts_with('-') {
        return Err(SbctlError::invalid_name(format!(
            "{name:?} must not start with a dash, which would be read as a flag"
        )));
    }
    if name.starts_with('.') {
        return Err(SbctlError::invalid_name(format!(
            "{name:?} must not start with a dot"
        )));
    }
    if name.ends_with('.') {
        return Err(SbctlError::invalid_name(format!(
            "{name:?} must not end with a dot"
        )));
    }
    for ch in name.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.') {
            return Err(SbctlError::invalid_name(format!(
                "{name:?} contains {ch:?}; use letters, digits, dot, dash, or underscore"
            )));
        }
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if WINDOWS_RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(SbctlError::invalid_name(format!(
            "{name:?} is a reserved device name on Windows"
        )));
    }
    Ok(())
}

/// Full profile file path inside `profiles_dir`. Caller must validate `name` first.
pub fn path_for(profiles_dir: &Path, name: &str) -> PathBuf {
    profiles_dir.join(format!("{name}{EXTENSION}"))
}

/// Profile name derived from a config file path.
pub fn name_for(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// List every `*.json` profile in `profiles_dir`, sorted alphabetically.
///
/// A missing profiles directory returns an empty list rather than an error so fresh
/// installations display cleanly.
pub fn list_profiles(profiles_dir: &Path) -> std::io::Result<Vec<Profile>> {
    let entries = match fs::read_dir(profiles_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut profiles = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let is_json = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        if !is_json {
            continue;
        }
        let name = name_for(&path);
        // Entries whose name the rest of the app would reject (spaces, a leading
        // dash, an uppercase extension) can never be activated, edited or
        // deleted, so listing them only creates ghosts.
        if validate_name(&name).is_err()
            || path.extension().and_then(|e| e.to_str()) != Some("json")
        {
            continue;
        }
        let raw = fs::read_to_string(&path).ok();
        // One parse per profile. This used to parse the same text twice, once
        // for the server/tun summary and once for the display facts.
        let tree: Option<Value> = raw
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok());
        let (server, tun) = tree
            .as_ref()
            .map(extract_server_and_tun_value)
            .unwrap_or((None, None));
        let source_url = read_source(profiles_dir, &name).ok().flatten();
        let parsed_facts = tree.as_ref().and_then(extract_config_facts_value);
        let config_parsed = parsed_facts.is_some();
        let facts = parsed_facts.unwrap_or_default();
        // Readiness from the parsed document, per outbound type and switches.
        let missing = tree
            .as_ref()
            .map(|v| ProfileFormModel::from_value(v).missing())
            .unwrap_or_default();

        profiles.push(Profile {
            name,
            path,
            missing,
            server,
            tun,
            source_url,
            config_parsed,
            facts,
        });
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Parse `json_text` once and return `(server_summary, tun_interface_name)`.
pub fn extract_server_and_tun(json_text: &str) -> (Option<String>, Option<String>) {
    let Ok(val) = serde_json::from_str::<Value>(json_text) else {
        return (None, None);
    };
    extract_server_and_tun_value(&val)
}

/// As [`extract_server_and_tun`], for callers that already hold a parsed tree.
pub fn extract_server_and_tun_value(val: &Value) -> (Option<String>, Option<String>) {
    (extract_server_value(val), extract_interface_name_value(val))
}

fn extract_interface_name_value(val: &Value) -> Option<String> {
    let inbounds = val.get("inbounds")?.as_array()?;
    for inbound in inbounds {
        if inbound.get("type").and_then(Value::as_str) == Some("tun")
            && let Some(iface) = inbound.get("interface_name").and_then(Value::as_str)
            && !iface.is_empty()
        {
            return Some(iface.to_string());
        }
    }
    None
}

fn extract_server_value(val: &Value) -> Option<String> {
    let outbounds = val.get("outbounds")?.as_array()?;
    for out in outbounds {
        let kind = out.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(kind, "direct" | "block" | "dns") {
            continue;
        }
        let server = out.get("server").and_then(Value::as_str).unwrap_or("");
        if server.is_empty() {
            continue;
        }
        if let Some(port) = out.get("server_port").and_then(Value::as_u64)
            && port > 0
        {
            return Some(format!("{server}:{port}"));
        }
        return Some(server.to_string());
    }
    None
}

/// Return the declared TUN `interface_name` from a profile JSON, if non-empty.
pub fn extract_interface_name(json_text: &str) -> Option<String> {
    let val: Value = serde_json::from_str(json_text).ok()?;
    extract_interface_name_value(&val)
}

/// Read the declared TUN `interface_name` from a profile path.
pub fn interface_name_from_file(path: &Path) -> Option<String> {
    let data = fs::read_to_string(path).ok()?;
    extract_interface_name(&data)
}

/// Return a short `host:port` summary of the first non-direct/non-block outbound.
pub fn extract_server(json_text: &str) -> Option<String> {
    let val: Value = serde_json::from_str(json_text).ok()?;
    extract_server_value(&val)
}

/// What a profile's own JSON declares, for display. Nothing here is observed
/// from the running daemon; every field is read from the configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigFacts {
    /// Outbound type of the first outbound that has a server (`vless`, `trojan`, …).
    pub outbound_type: Option<String>,
    /// Tag of that outbound, when the config sets one.
    pub outbound_tag: Option<String>,
    /// `host:port`, `[v6]:port`, or a bare host when no port is declared.
    pub endpoint: Option<String>,
    /// Declared TUN interface name.
    pub tun_interface: Option<String>,
    /// Whether the config declares any TUN inbound at all. Distinguishes
    /// "no TUN inbound" from "TUN inbound with no interface name".
    pub has_tun_inbound: bool,
}

impl ConfigFacts {
    /// True when the document parsed and declares a TUN inbound.
    pub fn tun_state(&self) -> TunState {
        if !self.has_tun_inbound {
            TunState::Absent
        } else if self.tun_interface.is_none() {
            TunState::Unnamed
        } else {
            TunState::Named
        }
    }
}

/// How a profile's TUN inbound is described, in the three cases that matter to a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunState {
    /// The document has no TUN inbound.
    Absent,
    /// A TUN inbound exists but sets no `interface_name`, so the OS picks the name.
    Unnamed,
    /// The profile names its interface.
    Named,
}

/// Parse a profile document into display facts.
///
/// Returns `None` when the document is not valid JSON: callers must distinguish
/// "cannot read this config" from "this config declares nothing", because they
/// mean different things to the user.
pub fn extract_config_facts(json_text: &str) -> Option<ConfigFacts> {
    let val: Value = serde_json::from_str(json_text).ok()?;
    extract_config_facts_value(&val)
}

/// As [`extract_config_facts`], for callers that already hold a parsed tree.
pub fn extract_config_facts_value(val: &Value) -> Option<ConfigFacts> {
    let (outbound_type, outbound_tag, endpoint) = val
        .get("outbounds")
        .and_then(Value::as_array)
        .and_then(|outbounds| {
            outbounds.iter().find_map(|out| {
                let kind = out.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "direct" | "block" | "dns") {
                    return None;
                }
                let server = out.get("server").and_then(Value::as_str).unwrap_or("");
                if server.is_empty() {
                    return None;
                }
                let endpoint = match out.get("server_port").and_then(Value::as_u64) {
                    Some(port) if port > 0 => {
                        if server.starts_with('[') || !server.contains(':') {
                            format!("{server}:{port}")
                        } else {
                            format!("[{server}]:{port}")
                        }
                    }
                    _ => server.to_string(),
                };
                Some((
                    Some(kind.to_string()),
                    out.get("tag").and_then(Value::as_str).map(str::to_string),
                    Some(endpoint),
                ))
            })
        })
        .unwrap_or((None, None, None));

    let inbounds = val.get("inbounds").and_then(Value::as_array);
    let tun_inbound = inbounds.and_then(|list| {
        list.iter()
            .find(|inbound| inbound.get("type").and_then(Value::as_str) == Some("tun"))
    });

    Some(ConfigFacts {
        outbound_type,
        outbound_tag,
        endpoint,
        tun_interface: tun_inbound
            .and_then(|t| t.get("interface_name").and_then(Value::as_str))
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        has_tun_inbound: tun_inbound.is_some(),
    })
}

/// Read display facts from a profile path.
pub fn config_facts_from_file(path: &Path) -> Option<ConfigFacts> {
    let data = fs::read_to_string(path).ok()?;
    extract_config_facts(&data)
}

/// Read the outbound server summary from a profile path.
pub fn server_from_file(path: &Path) -> Option<String> {
    let data = fs::read_to_string(path).ok()?;
    extract_server(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_facts_separate_absent_and_unnamed_tun() {
        let no_tun = r#"{"inbounds":[{"type":"mixed","listen":"127.0.0.1"}],"outbounds":[{"type":"vless","tag":"proxy","server":"203.0.113.9","server_port":443}]}"#;
        let facts = extract_config_facts(no_tun).expect("valid json");
        assert_eq!(facts.tun_state(), TunState::Absent);
        assert_eq!(facts.outbound_type.as_deref(), Some("vless"));
        assert_eq!(facts.outbound_tag.as_deref(), Some("proxy"));
        assert_eq!(facts.endpoint.as_deref(), Some("203.0.113.9:443"));

        let unnamed =
            r#"{"inbounds":[{"type":"tun","stack":"gvisor"}],"outbounds":[{"type":"direct"}]}"#;
        let facts = extract_config_facts(unnamed).expect("valid json");
        assert_eq!(facts.tun_state(), TunState::Unnamed);
        assert!(facts.endpoint.is_none());

        let named = r#"{"inbounds":[{"type":"tun","interface_name":"utun123"}],"outbounds":[{"type":"trojan","server":"[2001:db8::1]","server_port":8443}]}"#;
        let facts = extract_config_facts(named).expect("valid json");
        assert_eq!(facts.tun_state(), TunState::Named);
        assert_eq!(facts.endpoint.as_deref(), Some("[2001:db8::1]:8443"));
    }

    #[test]
    fn config_facts_report_unparseable_config_as_none() {
        assert!(extract_config_facts("{ not json").is_none());
    }

    #[test]
    fn validate_name_rejects_unsafe_and_reserved_names() {
        for bad in [
            "",
            ".",
            "..",
            "-flag",
            ".hidden",
            "trailing.",
            "../etc",
            "a/b",
            r"a\b",
            "with space",
            "nul",
            "NUL",
            "con.prod",
            "com1",
            "lpt9.backup",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(validate_name(&"a".repeat(65)).is_err());

        for good in ["work-vpn", "aws_sg.v2", "a", &"x".repeat(64)] {
            assert!(validate_name(good).is_ok(), "{good:?} should be valid");
        }
    }

    #[test]
    fn skeleton_seeds_no_sentinel_values() {
        // A new profile must not carry placeholder text into a real config file.
        assert!(!SKELETON_JSON.contains("TODO_"));
        let tree: Value = serde_json::from_str(SKELETON_JSON).expect("skeleton is valid JSON");
        let missing = crate::form_model::ProfileFormModel::from_value(&tree).missing();
        // The seed declares a vless outbound with TLS+Reality, so these are what
        // a fresh profile still needs before it can connect.
        assert!(missing.contains(&crate::readiness::Field::Server));
        assert!(missing.contains(&crate::readiness::Field::Uuid));
        assert!(missing.contains(&crate::readiness::Field::RealityPublicKey));
    }

    #[test]
    fn an_unparseable_profile_is_not_ready() {
        // The old marker scan found nothing in an unreadable file and called it
        // ready. Readiness now requires the config to parse.
        let p = Profile {
            name: "x".into(),
            path: PathBuf::from("/tmp/x.json"),
            missing: Vec::new(),
            server: None,
            tun: None,
            source_url: None,
            config_parsed: false,
            facts: Default::default(),
        };
        assert!(!p.ready());
    }

    #[test]
    fn extract_server_skips_direct_and_formats_port() {
        let json = r#"{
            "outbounds": [
                {"type": "direct", "tag": "direct"},
                {"type": "vless", "server": "vpn.example.com", "server_port": 443}
            ]
        }"#;
        assert_eq!(extract_server(json).as_deref(), Some("vpn.example.com:443"));
    }
}
