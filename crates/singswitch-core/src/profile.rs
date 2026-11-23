use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, SingSwitchError};
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
        return Err(SingSwitchError::invalid_name("name must not be empty"));
    }
    if name.len() > MAX_NAME_LENGTH {
        return Err(SingSwitchError::invalid_name(format!(
            "{name:?} is longer than {MAX_NAME_LENGTH} characters"
        )));
    }
    if name == "." || name == ".." {
        return Err(SingSwitchError::invalid_name(format!(
            "{name:?} is a directory reference"
        )));
    }
    if name.starts_with('-') {
        return Err(SingSwitchError::invalid_name(format!(
            "{name:?} must not start with a dash, which would be read as a flag"
        )));
    }
    if name.starts_with('.') {
        return Err(SingSwitchError::invalid_name(format!(
            "{name:?} must not start with a dot"
        )));
    }
    if name.ends_with('.') {
        return Err(SingSwitchError::invalid_name(format!(
            "{name:?} must not end with a dot"
        )));
    }
    for ch in name.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.') {
            return Err(SingSwitchError::invalid_name(format!(
                "{name:?} contains {ch:?}; use letters, digits, dot, dash, or underscore"
            )));
        }
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if WINDOWS_RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(SingSwitchError::invalid_name(format!(
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
        let tree: Option<Value> = raw
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok());
        let source_url = read_source(profiles_dir, &name).ok().flatten();
        // A tree means the file was readable JSON; the facts may still be empty,
        // which is a different answer from "could not read this file".
        let config_parsed = tree.is_some();
        let facts = tree
            .as_ref()
            .map(extract_config_facts_value)
            .unwrap_or_default();
        // Readiness from the parsed document, per outbound type and switches.
        let missing = tree
            .as_ref()
            .map(|v| ProfileFormModel::from_value(v).missing())
            .unwrap_or_default();

        profiles.push(Profile {
            name,
            path,
            missing,
            source_url,
            config_parsed,
            facts,
        });
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// What a profile's own JSON declares, for display. Nothing here is observed
/// from the running daemon; every field is read from the configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigFacts {
    /// Outbound type of the first outbound that has a server (`vless`, `trojan`, …).
    pub outbound_type: Option<String>,
    /// `host:port`, `[v6]:port`, or a bare host when no port is declared.
    pub endpoint: Option<String>,
    /// Declared TUN interface name.
    pub tun_interface: Option<String>,
    /// Whether the config declares any TUN inbound at all. Distinguishes
    /// "no TUN inbound" from "TUN inbound with no interface name".
    pub has_tun_inbound: bool,
}

impl ConfigFacts {
    /// How the TUN part of this document reads. Says nothing about whether the
    /// document parsed: an empty `ConfigFacts` (unreadable file) is `Absent`
    /// too, so callers that need that distinction check `Profile::config_parsed`
    /// first.
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

/// Parse display facts out of an already-parsed profile document.
///
/// Infallible by design: given a tree it returns facts, and an empty
/// `ConfigFacts` is a real answer ("this config declares nothing"). Callers
/// tell that apart from an unreadable file because they only call this when a
/// tree exists — see `config_parsed`.
pub fn extract_config_facts_value(val: &Value) -> ConfigFacts {
    let (outbound_type, endpoint) = val
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
                Some((Some(kind.to_string()), Some(endpoint)))
            })
        })
        .unwrap_or((None, None));

    let inbounds = val.get("inbounds").and_then(Value::as_array);
    let tun_inbound = inbounds.and_then(|list| {
        list.iter()
            .find(|inbound| inbound.get("type").and_then(Value::as_str) == Some("tun"))
    });

    ConfigFacts {
        outbound_type,
        endpoint,
        tun_interface: tun_inbound
            .and_then(|t| t.get("interface_name").and_then(Value::as_str))
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        has_tun_inbound: tun_inbound.is_some(),
    }
}
