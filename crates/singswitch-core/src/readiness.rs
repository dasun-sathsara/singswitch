//! What a profile still needs before it can connect.
//!
//! A profile used to be seeded with `TODO_*` sentinel strings and judged by a
//! regex over the raw file text. That conflated three separate questions - does
//! the file parse, does sing-box accept it, and has the user filled the fields
//! this outbound type needs - and answered all three with one substring scan. It
//! false-flagged a password containing `TODO_`, missed lowercase sentinels,
//! treated an unreadable file as ready, and could only say "replace TODO_
//! markers" rather than naming a field.
//!
//! Readiness is now a predicate over the parsed document, conditioned on the
//! outbound type and the TLS/Reality switches. `sing-box check` remains the
//! separate, authoritative answer to "is this valid".

use serde::{Deserialize, Serialize};

use crate::form_model::ProfileFormModel;

/// A field the guided form can fill, used to report what is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Server,
    Uuid,
    Password,
    ServerName,
    RealityPublicKey,
    RealityShortId,
}

impl Field {
    /// Stable machine name, matching the `--set` key where there is one.
    ///
    /// Emitted in JSON so a script can map a missing field straight back to the
    /// command that fills it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Uuid => "uuid",
            Self::Password => "password",
            Self::ServerName => "server_name",
            Self::RealityPublicKey => "public_key",
            Self::RealityShortId => "short_id",
        }
    }

    /// Human label for banners, tooltips, and the editor strip.
    pub fn label(self) -> &'static str {
        match self {
            Self::Server => "server address",
            Self::Uuid => "UUID",
            Self::Password => "password",
            Self::ServerName => "TLS server name",
            Self::RealityPublicKey => "Reality public key",
            Self::RealityShortId => "Reality short ID",
        }
    }
}

/// Values older builds wrote into a new profile.
///
/// Treated as "not filled in" rather than as data, so existing profiles become
/// correct without any file being rewritten behind the user's back.
pub const LEGACY_SENTINELS: [&str; 5] = [
    "TODO_SERVER_IP_OR_HOST",
    "TODO_UUID",
    "TODO_SNI_HOSTNAME",
    "TODO_REALITY_PUBLIC_KEY",
    "TODO_SHORT_ID",
];

/// Whether a value counts as filled in.
pub fn is_set(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && !LEGACY_SENTINELS.contains(&trimmed)
}

/// Fields this outbound type needs, in display order.
///
/// Unknown types require only a server: an imported config should not be
/// second-guessed here, and `sing-box check` covers the rest.
pub fn required(outbound_type: &str) -> Vec<Field> {
    match outbound_type {
        "vless" | "vmess" | "tuic" => vec![Field::Server, Field::Uuid],
        "trojan" | "hysteria2" | "anytls" | "shadowsocks" => {
            vec![Field::Server, Field::Password]
        }
        _ => vec![Field::Server],
    }
}

/// The TLS-level fields that are required only when their switch is on.
///
/// Turning Reality or TLS off used to leave the sentinels behind and made a
/// perfectly valid config permanently unready.
pub fn required_tls(tls: bool, reality: bool) -> Vec<Field> {
    let mut fields = Vec::new();
    if tls {
        fields.push(Field::ServerName);
    }
    if reality {
        fields.push(Field::RealityPublicKey);
        fields.push(Field::RealityShortId);
    }
    fields
}

/// Warn when a declared TUN inbound is never punctured into the system.
///
/// `auto_route` off means sing-box creates the interface and installs no OS
/// routes, so nothing is sent through it: the tunnel is up and unused. That is
/// a different failure from `strict_route` off, where traffic does go through
/// the tunnel but a stray socket can still find an interface outside it, so the
/// two get different sentences. Returns `None` when there is no TUN inbound to
/// be wrong about, or when both switches are on.
pub fn tun_advisory(model: &ProfileFormModel) -> Option<&'static str> {
    if !model.has_tun_inbound {
        return None;
    }
    if !model.tun_auto_route {
        return Some(
            "auto_route is off: this profile does not install routes, so traffic bypasses the tunnel",
        );
    }
    if !model.tun_strict_route {
        return Some("strict_route is off: some traffic can leak around the tunnel while it runs");
    }
    None
}

/// Render a list of fields for a sentence: "A, B and C".
pub fn join_labels(fields: &[Field]) -> String {
    let labels: Vec<&str> = fields.iter().map(|f| f.label()).collect();
    match labels.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            // Unreachable in practice: this arm requires len >= 3, but the
            // pattern match is kept total rather than panicking on an edge.
            match labels.split_last() {
                Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
                None => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(auto_route: bool, strict_route: bool) -> ProfileFormModel {
        ProfileFormModel {
            has_tun_inbound: true,
            tun_auto_route: auto_route,
            tun_strict_route: strict_route,
            ..ProfileFormModel::default()
        }
    }

    #[test]
    fn a_tun_profile_routed_all_the_way_through_has_no_advisory() {
        assert_eq!(tun_advisory(&model(true, true)), None);
    }

    #[test]
    fn auto_route_off_is_reported_as_not_proxying_at_all() {
        let advisory = tun_advisory(&model(false, true)).unwrap_or_default();
        assert!(advisory.contains("auto_route is off"));
        assert!(advisory.contains("bypasses"));
    }

    #[test]
    fn auto_route_off_wins_over_strict_route_off() {
        // Both are off: the stronger statement is the true one. Telling the user
        // about a leak when nothing is routed at all would misdescribe the file.
        let advisory = tun_advisory(&model(false, false)).unwrap_or_default();
        assert!(advisory.contains("auto_route is off"));
    }

    #[test]
    fn strict_route_off_is_reported_as_a_leak_not_as_no_routing() {
        let advisory = tun_advisory(&model(true, false)).unwrap_or_default();
        assert!(advisory.contains("strict_route is off"));
        assert!(advisory.contains("leak"));
    }

    #[test]
    fn a_profile_without_a_tun_inbound_gets_no_advisory() {
        let mut model = model(false, false);
        model.has_tun_inbound = false;
        assert_eq!(tun_advisory(&model), None);
    }

    fn from_document(inbound: serde_json::Value) -> ProfileFormModel {
        ProfileFormModel::from_value(&serde_json::json!({"inbounds": [inbound]}))
    }

    #[test]
    fn the_form_model_tracks_whether_the_file_has_a_tun_inbound() {
        // The advisory is only as good as this flag, and it has to come from the
        // document rather than from the form's defaults.
        assert!(
            from_document(serde_json::json!({"type": "tun", "auto_route": false})).has_tun_inbound
        );
        assert!(!from_document(serde_json::json!({"type": "mixed"})).has_tun_inbound);
        assert!(
            !ProfileFormModel::from_value(&serde_json::json!({"inbounds": "nope"})).has_tun_inbound
        );
    }

    #[test]
    fn a_parsed_document_with_a_leaky_tun_produces_the_advisory() {
        let parsed = from_document(serde_json::json!({
            "type": "tun",
            "auto_route": true,
            "strict_route": false
        }));
        assert!(tun_advisory(&parsed).is_some_and(|a| a.contains("leak")));
    }
}
