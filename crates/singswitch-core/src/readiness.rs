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
/// be wrong about, or when both switches are on. A file that omits both keys
/// counts as off: sing-box defaults them to false.
fn tun_route_advisory(model: &ProfileFormModel) -> Option<&'static str> {
    if !model.has_tun_inbound {
        return None;
    }
    if !model.tun_auto_route {
        if model.tun_route_keys_present {
            return Some(
                "auto_route is off: this profile does not install routes, so traffic bypasses the tunnel",
            );
        }
        return Some(
            "the tun inbound sets no routes: sing-box defaults auto_route off, so traffic bypasses the tunnel",
        );
    }
    if !model.tun_strict_route {
        return Some("strict_route is off: some traffic can leak around the tunnel while it runs");
    }
    None
}

/// Public advisory kept as a thin alias so callers do not reach into the
/// presence-tracking detail.
pub fn tun_advisory(model: &ProfileFormModel) -> Option<&'static str> {
    tun_route_advisory(model)
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
