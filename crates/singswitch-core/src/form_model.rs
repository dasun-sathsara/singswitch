use serde_json::{Map, Value, json, map::Entry};

use crate::error::{Result, SingSwitchError};

pub const DEFAULT_SERVER_PORT: u16 = 443;
pub const DEFAULT_TUN_MTU: u32 = 1350;

/// Lossless form view extracted from a sing-box JSON document.
///
/// Editing fields through `ProfileFormModel::apply_to_json` mutates only the targeted keys
/// inside the original `serde_json::Value` tree, preserving DNS servers, custom route rules,
/// `endpoints` (such as Tailscale `tsnet`), and unknown fields verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFormModel {
    pub outbound_type: String,
    pub outbound_tag: String,
    pub server: String,
    pub server_port: u16,
    pub uuid: String,
    pub password: String,
    pub flow: String,
    pub tls_enabled: bool,
    pub server_name: String,
    pub utls_enabled: bool,
    pub utls_fingerprint: String,
    pub reality_enabled: bool,
    pub reality_public_key: String,
    pub reality_short_id: String,
    pub tun_interface_name: String,
    pub tun_stack: String,
    pub tun_mtu: u32,
    pub tun_auto_route: bool,
    pub tun_strict_route: bool,
    pub reject_ipv6: bool,
    pub log_level: String,
    /// Whether the source document contained an outbound this form manages.
    ///
    /// Without one (an endpoints-only config, or `direct` only), the form's
    /// defaults describe nothing real: reporting their fields as missing
    /// demanded values the file never had, and `apply_to_value` cannot write
    /// them, so a valid config could never be saved.
    pub has_managed_outbound: bool,
    /// `tls.disable_sni` waives the server-name requirement.
    pub disable_sni: bool,
}

impl Default for ProfileFormModel {
    fn default() -> Self {
        Self {
            outbound_type: "vless".to_string(),
            outbound_tag: "proxy".to_string(),
            server: String::new(),
            server_port: DEFAULT_SERVER_PORT,
            uuid: String::new(),
            password: String::new(),
            flow: "xtls-rprx-vision".to_string(),
            tls_enabled: true,
            server_name: String::new(),
            utls_enabled: true,
            utls_fingerprint: "chrome".to_string(),
            reality_enabled: true,
            reality_public_key: String::new(),
            reality_short_id: String::new(),
            tun_interface_name: String::new(),
            tun_stack: "gvisor".to_string(),
            tun_mtu: DEFAULT_TUN_MTU,
            tun_auto_route: true,
            tun_strict_route: true,
            reject_ipv6: true,
            log_level: "info".to_string(),
            has_managed_outbound: false,
            disable_sni: false,
        }
    }
}

impl ProfileFormModel {
    /// Set one guided field by its CLI name. Used by `singswitch edit --set` so a
    /// script can fill a profile without writing JSON or opening an editor.
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<()> {
        let parse_u32 = |v: &str| {
            v.trim().parse::<u32>().map_err(|_| {
                crate::error::SingSwitchError::validation(format!(
                    "{key} must be a number, got {v:?}"
                ))
            })
        };
        let parse_bool = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(crate::error::SingSwitchError::validation(format!(
                "{key} must be true or false, got {v:?}"
            ))),
        };
        match key {
            "type" | "outbound_type" => self.outbound_type = value.trim().to_string(),
            "tag" | "outbound_tag" => self.outbound_tag = value.trim().to_string(),
            "server" => self.server = value.trim().to_string(),
            // Checked, not `as`: 70000 used to be written back as 4464.
            "port" | "server_port" => {
                let port = parse_u32(value)?;
                self.server_port = u16::try_from(port).map_err(|_| {
                    crate::error::SingSwitchError::validation(format!(
                        "port must be between 0 and 65535, got {port}"
                    ))
                })?;
            }
            "uuid" => self.uuid = value.trim().to_string(),
            "password" => self.password = value.to_string(),
            "flow" => self.flow = value.trim().to_string(),
            "tls" | "tls_enabled" => self.tls_enabled = parse_bool(value)?,
            "server_name" | "sni" => self.server_name = value.trim().to_string(),
            "utls" | "utls_enabled" => self.utls_enabled = parse_bool(value)?,
            "fingerprint" | "utls_fingerprint" => self.utls_fingerprint = value.trim().to_string(),
            "reality" | "reality_enabled" => self.reality_enabled = parse_bool(value)?,
            "public_key" | "reality_public_key" => {
                self.reality_public_key = value.trim().to_string()
            }
            "short_id" | "reality_short_id" => self.reality_short_id = value.trim().to_string(),
            "tun_name" | "tun_interface_name" => self.tun_interface_name = value.trim().to_string(),
            "stack" | "tun_stack" => self.tun_stack = value.trim().to_string(),
            "mtu" | "tun_mtu" => self.tun_mtu = parse_u32(value)?,
            "auto_route" | "tun_auto_route" => self.tun_auto_route = parse_bool(value)?,
            "strict_route" | "tun_strict_route" => self.tun_strict_route = parse_bool(value)?,
            "reject_ipv6" => self.reject_ipv6 = parse_bool(value)?,
            "log_level" => self.log_level = value.trim().to_string(),
            other => {
                return Err(crate::error::SingSwitchError::validation(format!(
                    "unknown field {other:?}; run `singswitch edit --help` for the list"
                )));
            }
        }
        Ok(())
    }

    /// Parse a raw sing-box JSON document into a guided form model.
    pub fn from_json_str(raw: &str) -> Result<Self> {
        let root: Value = serde_json::from_str(raw)
            .map_err(|e| SingSwitchError::validation(format!("invalid JSON: {e}")))?;
        Ok(Self::from_value(&root))
    }

    /// Fields this model leaves unfilled, for the current outbound type and
    /// TLS/Reality switches. Empty means ready to connect.
    ///
    /// This is the only readiness predicate: it reads the document through the
    /// same extraction the guided form uses, so the form and the file can never
    /// disagree about what is missing.
    pub fn missing(&self) -> Vec<crate::readiness::Field> {
        use crate::readiness::{Field, is_set, required, required_tls};

        // Nothing managed: nothing to fill in. `sing-box check` still decides
        // whether the file is usable.
        if !self.has_managed_outbound {
            return Vec::new();
        }

        let mut missing = Vec::new();
        for field in required(&self.outbound_type) {
            let filled = match field {
                Field::Server => is_set(&self.server),
                Field::Uuid => is_set(&self.uuid),
                Field::Password => is_set(&self.password),
                _ => true,
            };
            if !filled {
                missing.push(field);
            }
        }
        let tls_fields = required_tls(self.tls_enabled, self.reality_enabled);
        for field in tls_fields {
            // A config that turns SNI off does not need a name for it.
            if field == Field::ServerName && self.disable_sni {
                continue;
            }
            let filled = match field {
                Field::ServerName => is_set(&self.server_name),
                Field::RealityPublicKey => is_set(&self.reality_public_key),
                Field::RealityShortId => is_set(&self.reality_short_id),
                _ => true,
            };
            if !filled {
                missing.push(field);
            }
        }
        missing
    }

    /// Extract guided form values from a parsed JSON tree.
    pub fn from_value(root: &Value) -> Self {
        let mut model = Self::default();

        if let Some(level) = root
            .get("log")
            .and_then(|l| l.get("level"))
            .and_then(Value::as_str)
        {
            model.log_level = level.to_string();
        }

        if let Some(tun) = find_tun_inbound(root) {
            model.tun_interface_name = str_field(tun, "interface_name");
            let stack = str_field(tun, "stack");
            if !stack.is_empty() {
                model.tun_stack = stack;
            }
            // Saturating conversion, not `as`: an out-of-range MTU would
            // otherwise wrap to a small number and be written back on save,
            // corrupting a config the user only opened.
            if let Some(mtu) = tun.get("mtu").and_then(Value::as_u64) {
                model.tun_mtu = u32::try_from(mtu).unwrap_or(model.tun_mtu);
            }
            if let Some(ar) = tun.get("auto_route").and_then(Value::as_bool) {
                model.tun_auto_route = ar;
            }
            if let Some(sr) = tun.get("strict_route").and_then(Value::as_bool) {
                model.tun_strict_route = sr;
            }
        }

        if let Some(out) = find_primary_outbound(root) {
            model.has_managed_outbound = true;
            model.disable_sni = out
                .get("tls")
                .and_then(|t| t.get("disable_sni"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let kind = str_field(out, "type");
            if !kind.is_empty() {
                model.outbound_type = kind;
            }
            let tag = str_field(out, "tag");
            if !tag.is_empty() {
                model.outbound_tag = tag;
            }
            model.server = str_field(out, "server");
            if let Some(port) = out.get("server_port").and_then(Value::as_u64) {
                // 70000 used to become 4464 and then be saved back.
                model.server_port = u16::try_from(port).unwrap_or(model.server_port);
            }
            model.uuid = str_field(out, "uuid");
            model.password = str_field(out, "password");
            model.flow = str_field(out, "flow");

            if let Some(tls) = out.get("tls").and_then(Value::as_object) {
                model.tls_enabled = tls.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                model.server_name = tls
                    .get("server_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(utls) = tls.get("utls").and_then(Value::as_object) {
                    model.utls_enabled = utls
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    model.utls_fingerprint = utls
                        .get("fingerprint")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                } else {
                    model.utls_enabled = false;
                    model.utls_fingerprint.clear();
                }
                if let Some(reality) = tls.get("reality").and_then(Value::as_object) {
                    model.reality_enabled = reality
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    model.reality_public_key = reality
                        .get("public_key")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    model.reality_short_id = reality
                        .get("short_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                } else {
                    model.reality_enabled = false;
                    model.reality_public_key.clear();
                    model.reality_short_id.clear();
                }
            } else {
                model.tls_enabled = false;
                model.utls_enabled = false;
                model.reality_enabled = false;
            }
        }

        if let Some(rules) = root
            .get("route")
            .and_then(|r| r.get("rules"))
            .and_then(Value::as_array)
        {
            model.reject_ipv6 = rules.iter().any(is_ipv6_reject_rule);
        } else {
            model.reject_ipv6 = false;
        }

        model
    }

    /// Apply changes onto an existing JSON string, reporting any section the
    /// config has no place for.
    ///
    /// A `--set` that silently writes nothing still rewrites and saves the file,
    /// which reads as success to a script. Callers surface the skipped list.
    pub fn apply_to_json_str_reporting(&self, raw: &str) -> Result<(String, Vec<&'static str>)> {
        let mut root: Value = serde_json::from_str(raw)
            .map_err(|e| SingSwitchError::validation(format!("invalid JSON: {e}")))?;
        let skipped = self.apply_to_value(&mut root);
        let text = serde_json::to_string_pretty(&root)
            .map(|s| format!("{s}\n"))
            .map_err(|e| SingSwitchError::general(format!("could not serialize JSON: {e}")))?;
        Ok((text, skipped))
    }

    /// Write this model into `root`, returning the sections that could not be
    /// applied because the config has no place to put them.
    ///
    /// Guided fields used to be dropped silently in that case while Save still
    /// reported success, so the user's edits vanished behind a success message.
    pub fn apply_to_value(&self, root: &mut Value) -> Vec<&'static str> {
        let mut skipped = Vec::new();
        let Some(root_obj) = root.as_object_mut() else {
            return vec!["the config is not a JSON object"];
        };
        // Only write into sections that exist, so a single-field edit does not
        // add unrelated config the document never had.
        if root_obj.contains_key("log") {
            self.apply_log_section(root_obj);
        }
        if !self.apply_tun_section(root_obj) {
            skipped.push("Tunnel");
        }
        if !self.apply_outbound_section(root_obj) {
            skipped.push("Outbound");
        }
        if !self.apply_ipv6_route_section(root_obj) {
            skipped.push("Route rules");
        }
        skipped
    }

    fn apply_log_section(&self, root_obj: &mut Map<String, Value>) {
        if let Some(log_obj) = root_obj.get_mut("log").and_then(Value::as_object_mut) {
            log_obj.insert("level".to_string(), Value::String(self.log_level.clone()));
        }
    }

    fn apply_tun_section(&self, root_obj: &mut Map<String, Value>) -> bool {
        if let Some(tun) = find_tun_inbound_mut(root_obj) {
            tun.insert(
                "interface_name".to_string(),
                Value::String(self.tun_interface_name.clone()),
            );
            tun.insert("stack".to_string(), Value::String(self.tun_stack.clone()));
            tun.insert("mtu".to_string(), json!(self.tun_mtu));
            tun.insert("auto_route".to_string(), Value::Bool(self.tun_auto_route));
            tun.insert(
                "strict_route".to_string(),
                Value::Bool(self.tun_strict_route),
            );
            true
        } else {
            false
        }
    }

    fn apply_outbound_section(&self, root_obj: &mut Map<String, Value>) -> bool {
        let Some(out) = find_primary_outbound_mut(root_obj) else {
            return false;
        };
        out.insert(
            "type".to_string(),
            Value::String(self.outbound_type.clone()),
        );
        // Do not give an untagged outbound a tag: it would inherit the form's
        // default and could collide with an existing one, turning an unrelated
        // edit into a duplicate-tag rejection.
        if out.contains_key("tag") && !self.outbound_tag.is_empty() {
            out.insert("tag".to_string(), Value::String(self.outbound_tag.clone()));
        }
        out.insert("server".to_string(), Value::String(self.server.clone()));
        out.insert("server_port".to_string(), json!(self.server_port));

        set_or_remove_if_present(out, "uuid", &self.uuid);
        set_or_remove_if_present(out, "password", &self.password);
        set_or_remove_if_present(out, "flow", &self.flow);
        self.apply_tls_section(out);
        true
    }

    fn apply_tls_section(&self, out: &mut Map<String, Value>) {
        if !self.tls_enabled {
            if let Some(tls) = out.get_mut("tls").and_then(Value::as_object_mut) {
                tls.insert("enabled".to_string(), Value::Bool(false));
                // Reality is a TLS extension: leaving `reality.enabled` true
                // under a disabled TLS asked for public_key/short_id that no
                // command could turn off, so the profile could never be saved.
                if let Some(reality) = tls.get_mut("reality").and_then(Value::as_object_mut) {
                    reality.insert("enabled".to_string(), Value::Bool(false));
                }
            }
            return;
        }

        let Some(tls) = ensure_object(out, "tls") else {
            return;
        };
        tls.insert("enabled".to_string(), Value::Bool(true));
        if !self.server_name.is_empty() {
            tls.insert(
                "server_name".to_string(),
                Value::String(self.server_name.clone()),
            );
        } else {
            tls.remove("server_name");
        }

        if self.utls_enabled {
            if let Some(utls) = ensure_object(tls, "utls") {
                utls.insert("enabled".to_string(), Value::Bool(true));
                if !self.utls_fingerprint.is_empty() {
                    utls.insert(
                        "fingerprint".to_string(),
                        Value::String(self.utls_fingerprint.clone()),
                    );
                }
            }
        } else if let Some(utls) = tls.get_mut("utls").and_then(Value::as_object_mut) {
            utls.insert("enabled".to_string(), Value::Bool(false));
        }

        if self.reality_enabled {
            if let Some(reality) = ensure_object(tls, "reality") {
                reality.insert("enabled".to_string(), Value::Bool(true));
                reality.insert(
                    "public_key".to_string(),
                    Value::String(self.reality_public_key.clone()),
                );
                reality.insert(
                    "short_id".to_string(),
                    Value::String(self.reality_short_id.clone()),
                );
            }
        } else if let Some(reality) = tls.get_mut("reality").and_then(Value::as_object_mut) {
            reality.insert("enabled".to_string(), Value::Bool(false));
        }
    }

    fn apply_ipv6_route_section(&self, root_obj: &mut Map<String, Value>) -> bool {
        if let Some(rules) = root_obj
            .get_mut("route")
            .and_then(Value::as_object_mut)
            .and_then(|r| r.get_mut("rules"))
            .and_then(Value::as_array_mut)
        {
            let has_rule = rules.iter().any(is_ipv6_reject_rule);
            if self.reject_ipv6 && !has_rule {
                rules.insert(0, json!({"ip_version": 6, "action": "reject"}));
            } else if !self.reject_ipv6 && has_rule {
                rules.retain(|r| !is_ipv6_reject_rule(r));
            }
            true
        } else {
            // No route.rules to hold the rule. Only a skipped write if the user
            // actually toggled it off; leaving it alone is not a silent failure.
            !self.reject_ipv6
        }
    }
}

fn set_or_remove_if_present(obj: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.is_empty() || obj.contains_key(key) {
        if value.is_empty() {
            obj.remove(key);
        } else {
            obj.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
}

/// The `key` entry of `parent`, replaced with a fresh object when it holds
/// anything else.
///
/// Returning `Option` keeps the invariant in the type instead of in an
/// `unreachable!`: the tree is user input, and a panic on a save aborts the GUI.
/// `None` cannot follow the replacement below; callers treat it as "nothing to
/// write".
fn ensure_object<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
) -> Option<&'a mut Map<String, Value>> {
    let slot = match parent.entry(key.to_string()) {
        Entry::Occupied(mut entry) => {
            if !entry.get().is_object() {
                entry.insert(Value::Object(Map::new()));
            }
            entry.into_mut()
        }
        Entry::Vacant(entry) => entry.insert(Value::Object(Map::new())),
    };
    slot.as_object_mut()
}

fn find_tun_inbound(root: &Value) -> Option<&Map<String, Value>> {
    root.get("inbounds")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|m| m.get("type").and_then(Value::as_str) == Some("tun"))
}

fn find_tun_inbound_mut(root: &mut Map<String, Value>) -> Option<&mut Map<String, Value>> {
    root.get_mut("inbounds")?
        .as_array_mut()?
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .find(|m| m.get("type").and_then(Value::as_str) == Some("tun"))
}

fn find_primary_outbound(root: &Value) -> Option<&Map<String, Value>> {
    root.get("outbounds")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|m| {
            let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
            !matches!(kind, "direct" | "block" | "dns" | "selector" | "urltest")
        })
}

fn find_primary_outbound_mut(root: &mut Map<String, Value>) -> Option<&mut Map<String, Value>> {
    root.get_mut("outbounds")?
        .as_array_mut()?
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .find(|m| {
            let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
            !matches!(kind, "direct" | "block" | "dns" | "selector" | "urltest")
        })
}

fn is_ipv6_reject_rule(rule: &Value) -> bool {
    rule.get("ip_version").and_then(Value::as_u64) == Some(6)
        && rule.get("action").and_then(Value::as_str) == Some("reject")
}

fn str_field(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_object_tls_is_replaced_instead_of_panicking() {
        // `tls` is a number here; the save must replace it, not abort.
        let mut root = json!({
            "outbounds": [{"type": "vless", "server": "a", "tls": 7}]
        });
        let mut model = ProfileFormModel::from_value(&root);
        assert!(!model.tls_enabled);
        model.tls_enabled = true;
        model.server_name = "example.com".to_string();

        model.apply_to_value(&mut root);

        assert_eq!(root["outbounds"][0]["tls"]["enabled"], json!(true));
        assert_eq!(
            root["outbounds"][0]["tls"]["server_name"],
            json!("example.com")
        );
    }

    #[test]
    fn out_of_range_port_and_mtu_keep_the_defaults() {
        let root = json!({
            "inbounds": [{"type": "tun", "mtu": 5_000_000_000u64}],
            "outbounds": [{"type": "vless", "server_port": 70_000u32}]
        });
        let model = ProfileFormModel::from_value(&root);
        assert_eq!(model.tun_mtu, DEFAULT_TUN_MTU);
        assert_eq!(model.server_port, DEFAULT_SERVER_PORT);
    }
}
