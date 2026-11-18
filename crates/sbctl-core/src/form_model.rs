use serde_json::{Map, Value, json};

use crate::error::{Result, SbctlError};

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
}

impl Default for ProfileFormModel {
    fn default() -> Self {
        Self {
            outbound_type: "vless".to_string(),
            outbound_tag: "proxy".to_string(),
            server: String::new(),
            server_port: 443,
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
            tun_mtu: 1350,
            tun_auto_route: true,
            tun_strict_route: true,
            reject_ipv6: true,
            log_level: "info".to_string(),
        }
    }
}

impl ProfileFormModel {
    /// Parse a raw sing-box JSON document into a guided form model.
    pub fn from_json_str(raw: &str) -> Result<Self> {
        let root: Value = serde_json::from_str(raw)
            .map_err(|e| SbctlError::validation(format!("invalid JSON: {e}")))?;
        Ok(Self::from_value(&root))
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
            if let Some(mtu) = tun.get("mtu").and_then(Value::as_u64) {
                model.tun_mtu = mtu as u32;
            }
            if let Some(ar) = tun.get("auto_route").and_then(Value::as_bool) {
                model.tun_auto_route = ar;
            }
            if let Some(sr) = tun.get("strict_route").and_then(Value::as_bool) {
                model.tun_strict_route = sr;
            }
        }

        if let Some(out) = find_primary_outbound(root) {
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
                model.server_port = port as u16;
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

    /// Apply changes onto an existing JSON string without discarding unrelated fields.
    pub fn apply_to_json_str(&self, raw: &str) -> Result<String> {
        let mut root: Value = serde_json::from_str(raw)
            .map_err(|e| SbctlError::validation(format!("invalid JSON: {e}")))?;
        self.apply_to_value(&mut root);
        serde_json::to_string_pretty(&root)
            .map(|s| format!("{s}\n"))
            .map_err(|e| SbctlError::general(format!("could not serialize JSON: {e}")))
    }

    /// Mutate `root` in place for all fields controlled by the form model.
    pub fn apply_to_value(&self, root: &mut Value) {
        let Some(root_obj) = root.as_object_mut() else {
            return;
        };

        // Log level
        let log_obj = ensure_object(root_obj, "log");
        log_obj.insert("level".to_string(), Value::String(self.log_level.clone()));

        // TUN inbound
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
        }

        // Primary outbound
        if let Some(out) = find_primary_outbound_mut(root_obj) {
            out.insert(
                "type".to_string(),
                Value::String(self.outbound_type.clone()),
            );
            if !self.outbound_tag.is_empty() {
                out.insert("tag".to_string(), Value::String(self.outbound_tag.clone()));
            }
            out.insert("server".to_string(), Value::String(self.server.clone()));
            out.insert("server_port".to_string(), json!(self.server_port));

            if !self.uuid.is_empty() || out.contains_key("uuid") {
                if self.uuid.is_empty() {
                    out.remove("uuid");
                } else {
                    out.insert("uuid".to_string(), Value::String(self.uuid.clone()));
                }
            }
            if !self.password.is_empty() || out.contains_key("password") {
                if self.password.is_empty() {
                    out.remove("password");
                } else {
                    out.insert("password".to_string(), Value::String(self.password.clone()));
                }
            }
            if !self.flow.is_empty() || out.contains_key("flow") {
                if self.flow.is_empty() {
                    out.remove("flow");
                } else {
                    out.insert("flow".to_string(), Value::String(self.flow.clone()));
                }
            }

            if self.tls_enabled {
                let tls = ensure_object(out, "tls");
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
                    let utls = ensure_object(tls, "utls");
                    utls.insert("enabled".to_string(), Value::Bool(true));
                    if !self.utls_fingerprint.is_empty() {
                        utls.insert(
                            "fingerprint".to_string(),
                            Value::String(self.utls_fingerprint.clone()),
                        );
                    }
                } else if let Some(utls) = tls.get_mut("utls").and_then(Value::as_object_mut) {
                    utls.insert("enabled".to_string(), Value::Bool(false));
                }

                if self.reality_enabled {
                    let reality = ensure_object(tls, "reality");
                    reality.insert("enabled".to_string(), Value::Bool(true));
                    reality.insert(
                        "public_key".to_string(),
                        Value::String(self.reality_public_key.clone()),
                    );
                    reality.insert(
                        "short_id".to_string(),
                        Value::String(self.reality_short_id.clone()),
                    );
                } else if let Some(reality) = tls.get_mut("reality").and_then(Value::as_object_mut)
                {
                    reality.insert("enabled".to_string(), Value::Bool(false));
                }
            } else if let Some(tls) = out.get_mut("tls").and_then(Value::as_object_mut) {
                tls.insert("enabled".to_string(), Value::Bool(false));
            }
        }

        // IPv6 reject rule in route.rules
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
        }
    }
}

fn ensure_object<'a>(parent: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !parent.get(key).is_some_and(Value::is_object) {
        parent.insert(key.to_string(), Value::Object(Map::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted above")
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
    use crate::profile::{SKELETON_JSON, placeholders_in};

    #[test]
    fn round_trip_preserves_custom_sections_and_replaces_placeholders() {
        let input = r#"{
  "log": { "level": "info", "timestamp": true },
  "endpoints": [{ "type": "tailscale", "tag": "ts-ep", "state_directory": "tailscale" }],
  "inbounds": [{
    "type": "tun",
    "tag": "tun-in",
    "interface_name": "",
    "stack": "gvisor",
    "mtu": 1350,
    "auto_route": true,
    "strict_route": true
  }],
  "outbounds": [{
    "type": "vless",
    "tag": "proxy",
    "server": "TODO_SERVER_IP_OR_HOST",
    "server_port": 443,
    "uuid": "TODO_UUID",
    "flow": "xtls-rprx-vision",
    "tls": {
      "enabled": true,
      "server_name": "TODO_SNI_HOSTNAME",
      "utls": { "enabled": true, "fingerprint": "chrome" },
      "reality": { "enabled": true, "public_key": "TODO_REALITY_PUBLIC_KEY", "short_id": "TODO_SHORT_ID" }
    }
  }],
  "route": {
    "rules": [
      { "ip_version": 6, "action": "reject" },
      { "outbound": "proxy" }
    ]
  },
  "experimental": { "clash_api": { "external_controller": "127.0.0.1:9090" } }
}"#;
        let mut form = ProfileFormModel::from_json_str(input).expect("parse");
        form.server = "13.215.132.32".to_string();
        form.uuid = "11111111-2222-3333-4444-555555555555".to_string();
        form.server_name = "www.microsoft.com".to_string();
        form.reality_public_key = "pubkey123".to_string();
        form.reality_short_id = "abcd".to_string();
        form.tun_mtu = 1400;
        form.reject_ipv6 = false;

        let updated = form.apply_to_json_str(input).expect("apply");
        assert!(placeholders_in(&updated).is_empty());

        let parsed: Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(parsed["endpoints"][0]["tag"], "ts-ep");
        assert_eq!(
            parsed["experimental"]["clash_api"]["external_controller"],
            "127.0.0.1:9090"
        );
        assert_eq!(parsed["inbounds"][0]["mtu"], 1400);
        assert_eq!(parsed["outbounds"][0]["server"], "13.215.132.32");
        assert_eq!(parsed["route"]["rules"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parses_canonical_skeleton() {
        let form = ProfileFormModel::from_json_str(SKELETON_JSON).expect("parse skeleton");
        assert_eq!(form.outbound_type, "vless");
        assert_eq!(form.server_port, 443);
        assert!(form.tls_enabled);
        assert!(form.reality_enabled);
        assert!(form.reject_ipv6);
    }
}
