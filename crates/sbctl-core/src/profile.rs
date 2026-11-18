use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, SbctlError};
use crate::source::read_source;

/// Required filename extension for sing-box profiles.
pub const EXTENSION: &str = ".json";

/// Maximum profile name length keeping filenames portable across filesystems.
const MAX_NAME_LENGTH: usize = 64;

/// Canonical seed template embedded into the binary.
pub const SKELETON_JSON: &str = include_str!("../../../assets/skeleton.json");

static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"TODO_[A-Z0-9_]+").expect("valid regex"));

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Summary of a stored sing-box profile file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub path: PathBuf,
    pub placeholders: Vec<String>,
    pub server: Option<String>,
    pub tun: Option<String>,
    pub source_url: Option<String>,
}

impl Profile {
    /// Reports whether all template `TODO_` placeholders have been replaced.
    pub fn ready(&self) -> bool {
        self.placeholders.is_empty()
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
        let raw = fs::read_to_string(&path).ok();
        let placeholders = raw.as_deref().map(placeholders_in).unwrap_or_default();
        let (server, tun) = raw
            .as_deref()
            .map(extract_server_and_tun)
            .unwrap_or((None, None));
        let source_url = read_source(profiles_dir, &name).ok().flatten();

        profiles.push(Profile {
            name,
            path,
            placeholders,
            server,
            tun,
            source_url,
        });
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Return the sorted, deduplicated `TODO_*` markers in `text`.
pub fn placeholders_in(text: &str) -> Vec<String> {
    let mut found: Vec<String> = PLACEHOLDER_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Read placeholders from disk.
pub fn placeholders_in_file(path: &Path) -> std::io::Result<Vec<String>> {
    let data = fs::read_to_string(path)?;
    Ok(placeholders_in(&data))
}

/// Parse `json_text` once and return `(server_summary, tun_interface_name)`.
pub fn extract_server_and_tun(json_text: &str) -> (Option<String>, Option<String>) {
    let Ok(val) = serde_json::from_str::<Value>(json_text) else {
        return (None, None);
    };
    (
        extract_server_value(&val),
        extract_interface_name_value(&val),
    )
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

/// Read the outbound server summary from a profile path.
pub fn server_from_file(path: &Path) -> Option<String> {
    let data = fs::read_to_string(path).ok()?;
    extract_server(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn skeleton_contains_expected_placeholders() {
        let markers = placeholders_in(SKELETON_JSON);
        assert_eq!(
            markers,
            vec![
                "TODO_REALITY_PUBLIC_KEY",
                "TODO_SERVER_IP_OR_HOST",
                "TODO_SHORT_ID",
                "TODO_SNI_HOSTNAME",
                "TODO_UUID",
            ]
        );
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
