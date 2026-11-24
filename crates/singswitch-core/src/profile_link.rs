//! Import a profile from a share link.
//!
//! Share links (`vless://`, `trojan://`, `hysteria2://`, `ss://`, `vmess://`)
//! are untrusted input pasted from a provider's page, so every step here parses
//! into `Result` and a link either yields a config `sing-box check` accepts or a
//! validation error. Parsed links are never echoed back in an error: the
//! userinfo is a password or a UUID.
//!
//! Only two things are defaulted, both matching what every client does with the
//! same link: a TLS server name falls back to the server address, and a Trojan
//! link without an explicit `security` parameter means TLS.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::activator::write_new_file_exclusive;
use crate::checker::{Checker, check_temp_bytes};
use crate::error::{Result, SingSwitchError, store_error};
use crate::profile::{path_for, validate_name};
/// Longest share link accepted, so a hostile paste cannot make us allocate a
/// megabyte to reject it.
const MAX_LINK_CHARS: usize = 16 * 1024;

/// Port used when a link omits one; every supported scheme is TLS on 443.
const DEFAULT_PORT: u16 = 443;

/// Build a minimal sing-box config from `link` and return its JSON bytes.
pub fn profile_from_link(link: &str) -> Result<Vec<u8>> {
    let proxy = outbound_from_link(link)?;
    let config = config_with_proxy(proxy);
    let text = serde_json::to_string_pretty(&config).map_err(|err| {
        SingSwitchError::general(format!("could not serialize the imported config: {err}"))
    })?;
    Ok(format!("{text}\n").into_bytes())
}

/// Validate `link` into a config and store it as a new profile named `name`.
///
/// Bytes go through `sing-box check` before anything is written, so a link whose
/// transport this build mis-maps fails here instead of on the next connect. The
/// create is exclusive, mirroring `Controller::create_from_skeleton`: an import
/// must never replace a profile the user already has.
///
/// Takes its dependencies rather than a `Controller` so the GUI can run it on a
/// background executor while another agent's code owns the controller surface.
pub fn import_profile(
    profiles_dir: &Path,
    checker: &dyn Checker,
    name: &str,
    link: &str,
) -> Result<PathBuf> {
    validate_name(name)?;
    let bytes = profile_from_link(link)?;
    check_temp_bytes(checker, "singswitch-link-", &bytes).map_err(|err| {
        err.with_hint("the link parsed, but the config sing-switch built from it was rejected")
    })?;
    write_new_profile(profiles_dir, name, &bytes)
}

/// Store `bytes` as a new profile named `name`, refusing to overwrite.
fn write_new_profile(profiles_dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    validate_name(name)?;
    fs::create_dir_all(profiles_dir).map_err(|err| store_error(err, profiles_dir, "create"))?;

    let path = path_for(profiles_dir, name);
    write_new_file_exclusive(&path, bytes, 0o644).map_err(|err| {
        if err.kind() == ErrorKind::AlreadyExists {
            SingSwitchError::general(format!("a profile named {name:?} already exists"))
                .with_hint(format!("edit it with: singswitch edit {name}"))
        } else {
            store_error(err, profiles_dir, "write")
        }
    })?;
    Ok(path)
}

fn outbound_from_link(link: &str) -> Result<Map<String, Value>> {
    let trimmed = link.trim();
    if trimmed.is_empty() {
        return Err(share_link_error("the share link is empty"));
    }
    if trimmed.chars().count() > MAX_LINK_CHARS {
        return Err(share_link_error(
            "the share link is far longer than any link",
        ));
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(share_link_error(format!(
            "{} is not a share link",
            first_token(trimmed)
        )));
    };
    match scheme.to_ascii_lowercase().as_str() {
        "vless" => vless_outbound(rest),
        "trojan" => trojan_outbound(rest),
        "hysteria2" | "hy2" => hysteria2_outbound(rest),
        "ss" => shadowsocks_outbound(rest),
        "vmess" => vmess_outbound(rest),
        other => Err(share_link_error(format!(
            "{other}:// links are not supported"
        ))),
    }
}

/// The error every unparseable link returns, with the scheme list as action.
fn share_link_error(message: impl Into<String>) -> SingSwitchError {
    SingSwitchError::validation(message).with_hints([
        "supported links: vless://, trojan://, hysteria2:// (hy2://), ss:// and vmess://",
        "copy the whole link from your provider, including the part after #",
    ])
}

/// A leading word of the link, safe to show: the scheme, never the credentials.
fn first_token(link: &str) -> &str {
    link.split_whitespace().next().unwrap_or("")
}

/// How a link describes its TLS layer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Security {
    None,
    Tls,
    Reality,
}

impl Security {
    fn parse(raw: &str, default: Self) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" => Ok(default),
            "none" => Ok(Self::None),
            "tls" => Ok(Self::Tls),
            "reality" => Ok(Self::Reality),
            other => Err(share_link_error(format!("unknown security {other:?}"))),
        }
    }
}

/// Query of a `scheme://userinfo@host:port?query#fragment` link.
///
/// The fragment is a display name, not configuration, and the user names the
/// profile in the import dialog, so it is dropped with the query split.
struct ShareLink<'a> {
    /// Everything between the scheme and the query, i.e. `userinfo@host:port`.
    authority: &'a str,
    query: Vec<(String, String)>,
}

impl<'a> ShareLink<'a> {
    fn parse(rest: &'a str) -> Self {
        let body = match rest.split_once('#') {
            Some((body, _name)) => body,
            None => rest,
        };
        let (authority, raw_query) = match body.split_once('?') {
            Some((authority, query)) => (authority, query),
            None => (body, ""),
        };
        let query = raw_query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (percent_decode(key), percent_decode(value)),
                None => (percent_decode(pair), String::new()),
            })
            .collect();
        Self { authority, query }
    }

    /// First value for `key` (matched case-insensitively), or `""`.
    fn get(&self, key: &str) -> &str {
        self.query
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    /// Whether a flag-style parameter is switched on (`insecure=1`, `=true`).
    fn flag(&self, key: &str) -> bool {
        matches!(
            self.get(key).trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    }

    /// Split `userinfo@host:port` into credentials, host, and port.
    fn credentials(&self) -> Result<(&'a str, String, u16)> {
        let Some((userinfo, hostport)) = self.authority.rsplit_once('@') else {
            return Err(share_link_error("the share link has no user@host part"));
        };
        let (host, port) = split_host_port(hostport)?;
        Ok((userinfo, host, port))
    }
}

fn split_host_port(hostport: &str) -> Result<(String, u16)> {
    let (host, port) = match hostport.strip_prefix('[') {
        Some(rest) => {
            let Some((host, tail)) = rest.split_once(']') else {
                return Err(
                    share_link_error("the share link has an unterminated IPv6 address")
                        .with_hint("write an IPv6 address as [2001:db8::1]:443"),
                );
            };
            (host.to_string(), tail.strip_prefix(':').unwrap_or(""))
        }
        None => {
            // A bare IPv6 address without brackets has no unambiguous port
            // boundary, so refuse instead of reading its last group as a port.
            if hostport.matches(':').count() > 1 {
                return Err(
                    share_link_error("the share link has an unbracketed IPv6 address")
                        .with_hint("write an IPv6 address as [2001:db8::1]:443"),
                );
            }
            match hostport.rsplit_once(':') {
                Some((host, port)) => (host.to_string(), port),
                None => (hostport.to_string(), ""),
            }
        }
    };

    if host.is_empty() || host.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return Err(share_link_error("the share link has no server address"));
    }
    let port = if port.is_empty() {
        DEFAULT_PORT
    } else {
        port.parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| {
                share_link_error(format!("the share link has an invalid port {port:?}"))
            })?
    };
    Ok((host, port))
}

fn base_outbound(kind: &str, server: &str, port: u16) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String(kind.to_string()));
    out.insert("tag".to_string(), Value::String("proxy".to_string()));
    out.insert("server".to_string(), Value::String(server.to_string()));
    out.insert("server_port".to_string(), json!(port));
    out
}

struct TlsOptions<'a> {
    security: Security,
    server: &'a str,
    sni: &'a str,
    fingerprint: &'a str,
    public_key: &'a str,
    short_id: &'a str,
    insecure: bool,
}

fn apply_tls(out: &mut Map<String, Value>, opts: TlsOptions<'_>) -> Result<()> {
    if opts.security == Security::None {
        return Ok(());
    }
    let mut tls = Map::new();
    tls.insert("enabled".to_string(), Value::Bool(true));

    // Certificate verification stays on unless the link explicitly waives it.
    if opts.insecure {
        tls.insert("insecure".to_string(), Value::Bool(true));
    }
    let server_name = if opts.sni.trim().is_empty() {
        opts.server
    } else {
        opts.sni.trim()
    };
    tls.insert(
        "server_name".to_string(),
        Value::String(server_name.to_string()),
    );

    // sing-box refuses a Reality outbound that has no uTLS fingerprint, so a
    // link carrying none gets the client's usual one instead of failing check.
    let fingerprint = match opts.fingerprint.trim() {
        "" if opts.security == Security::Reality => "chrome",
        other => other,
    };
    if !fingerprint.is_empty() {
        tls.insert(
            "utls".to_string(),
            json!({"enabled": true, "fingerprint": fingerprint}),
        );
    }

    if opts.security == Security::Reality {
        if opts.public_key.trim().is_empty() {
            return Err(
                share_link_error("the Reality link has no public key").with_hint(
                    "a Reality link carries pbk=...; ask your provider for a complete link",
                ),
            );
        }
        tls.insert(
            "reality".to_string(),
            json!({
                "enabled": true,
                "public_key": opts.public_key.trim(),
                "short_id": opts.short_id.trim(),
            }),
        );
    }

    out.insert("tls".to_string(), Value::Object(tls));
    Ok(())
}

struct TransportOptions<'a> {
    kind: &'a str,
    host: &'a str,
    path: &'a str,
    service: &'a str,
}

fn apply_transport(out: &mut Map<String, Value>, opts: TransportOptions<'_>) -> Result<()> {
    let kind = opts.kind.trim().to_ascii_lowercase();
    let host = opts.host.trim();
    let path = opts.path.trim();
    let service = opts.service.trim();

    let mut transport = Map::new();
    match kind.as_str() {
        "" | "tcp" | "raw" | "none" => return Ok(()),
        "ws" | "websocket" => {
            transport.insert("type".to_string(), json!("ws"));
            if !path.is_empty() {
                transport.insert("path".to_string(), json!(path));
            }
            if !host.is_empty() {
                transport.insert("headers".to_string(), json!({"Host": host}));
            }
        }
        "grpc" | "gun" => {
            transport.insert("type".to_string(), json!("grpc"));
            if !service.is_empty() {
                transport.insert("service_name".to_string(), json!(service));
            }
        }
        "httpupgrade" => {
            transport.insert("type".to_string(), json!("httpupgrade"));
            if !path.is_empty() {
                transport.insert("path".to_string(), json!(path));
            }
            if !host.is_empty() {
                transport.insert("host".to_string(), json!(host));
            }
        }
        "http" | "h2" => {
            transport.insert("type".to_string(), json!("http"));
            if !path.is_empty() {
                transport.insert("path".to_string(), json!(path));
            }
            if !host.is_empty() {
                transport.insert("host".to_string(), json!([host]));
            }
        }
        other => {
            return Err(share_link_error(format!(
                "the share link uses the {other:?} transport, which singswitch cannot import"
            ))
            .with_hint("re-export the link with the tcp, ws, grpc or httpupgrade transport"));
        }
    }
    out.insert("transport".to_string(), Value::Object(transport));
    Ok(())
}

/// Transport parameters as shared by the VMess-style URL query form.
fn apply_query_transport(out: &mut Map<String, Value>, link: &ShareLink<'_>) -> Result<()> {
    let service = match link.get("servicename") {
        "" => link.get("service"),
        named => named,
    };
    apply_transport(
        out,
        TransportOptions {
            kind: link.get("type"),
            host: link.get("host"),
            path: link.get("path"),
            service,
        },
    )
}

fn vless_outbound(rest: &str) -> Result<Map<String, Value>> {
    let link = ShareLink::parse(rest);
    let (userinfo, server, port) = link.credentials()?;
    let uuid = percent_decode(userinfo);
    if uuid.is_empty() {
        return Err(share_link_error("the vless link has no UUID")
            .with_hint("a vless link reads vless://uuid@host:port?type=...&security=...#name"));
    }

    let mut out = base_outbound("vless", &server, port);
    out.insert("uuid".to_string(), Value::String(uuid));
    let flow = link.get("flow");
    if !flow.is_empty() {
        out.insert("flow".to_string(), Value::String(flow.to_string()));
    }
    apply_tls(
        &mut out,
        TlsOptions {
            security: Security::parse(link.get("security"), Security::None)?,
            server: &server,
            sni: link.get("sni"),
            fingerprint: link.get("fp"),
            public_key: link.get("pbk"),
            short_id: link.get("sid"),
            insecure: link.flag("insecure"),
        },
    )?;
    apply_query_transport(&mut out, &link)?;
    Ok(out)
}

fn trojan_outbound(rest: &str) -> Result<Map<String, Value>> {
    let link = ShareLink::parse(rest);
    let (userinfo, server, port) = link.credentials()?;
    let password = percent_decode(userinfo);
    if password.is_empty() {
        return Err(share_link_error("the trojan link has no password")
            .with_hint("a trojan link reads trojan://password@host:port?sni=...#name"));
    }

    let mut out = base_outbound("trojan", &server, port);
    out.insert("password".to_string(), Value::String(password));
    // Trojan has no plaintext mode anyone deploys, so a link that omits
    // `security` means TLS; only an explicit security=none turns it off.
    apply_tls(
        &mut out,
        TlsOptions {
            security: Security::parse(link.get("security"), Security::Tls)?,
            server: &server,
            sni: link.get("sni"),
            fingerprint: link.get("fp"),
            public_key: link.get("pbk"),
            short_id: link.get("sid"),
            insecure: link.flag("insecure"),
        },
    )?;
    apply_query_transport(&mut out, &link)?;
    Ok(out)
}

fn hysteria2_outbound(rest: &str) -> Result<Map<String, Value>> {
    let link = ShareLink::parse(rest);
    let (userinfo, server, port) = link.credentials()?;
    let password = percent_decode(userinfo);
    if password.is_empty() {
        return Err(share_link_error("the hysteria2 link has no password")
            .with_hint("a hysteria2 link reads hysteria2://password@host:port?sni=...#name"));
    }

    let mut out = base_outbound("hysteria2", &server, port);
    out.insert("password".to_string(), Value::String(password));
    // QUIC is always TLS here: sing-box rejects a hysteria2 outbound without it.
    apply_tls(
        &mut out,
        TlsOptions {
            security: Security::parse(link.get("security"), Security::Tls)?,
            server: &server,
            sni: link.get("sni"),
            fingerprint: link.get("fp"),
            public_key: "",
            short_id: "",
            insecure: link.flag("insecure"),
        },
    )?;

    let obfs = link.get("obfs");
    if !obfs.is_empty() {
        out.insert(
            "obfs".to_string(),
            json!({"type": obfs, "password": link.get("obfs-password")}),
        );
    }
    Ok(out)
}

fn shadowsocks_outbound(rest: &str) -> Result<Map<String, Value>> {
    let link = ShareLink::parse(rest);
    if !link.authority.contains('@') {
        return legacy_shadowsocks_outbound(&link);
    }
    if !link.get("plugin").is_empty() {
        return Err(share_link_error(
            "this ss link needs a SIP003 plugin, which singswitch cannot import",
        )
        .with_hint("ask your provider for a link without a plugin"));
    }

    let (userinfo, server, port) = link.credentials()?;
    let raw = percent_decode(userinfo);
    // SIP002 puts base64(method:password) in the userinfo. A colon can only come
    // from the plain `method:password` form: it is not in the base64 alphabet.
    let credentials = if raw.contains(':') {
        raw
    } else {
        match decode_base64(&raw).and_then(|bytes| String::from_utf8(bytes).ok()) {
            Some(text) => text,
            None => {
                return Err(share_link_error(
                    "the ss link credentials are not valid base64",
                ));
            }
        }
    };
    let Some((method, password)) = credentials.split_once(':') else {
        return Err(share_link_error(
            "the ss link credentials are not method:password",
        ));
    };
    shadowsocks_outbound_for(method, password, &server, port).map_err(|err| {
        err.with_hint("a SIP002 link reads ss://base64(method:password)@host:port#name")
    })
}

/// The pre-SIP002 form: one base64 blob holding `method:password@host:port`.
fn legacy_shadowsocks_outbound(link: &ShareLink<'_>) -> Result<Map<String, Value>> {
    let decoded = decode_base64(link.authority.trim())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| {
            share_link_error("the ss link is not valid base64")
                .with_hint("a legacy ss link reads ss://base64(method:password@host:port)#name")
        })?;
    let Some((credentials, hostport)) = decoded.rsplit_once('@') else {
        return Err(share_link_error(
            "the ss link decodes to no method:password@host:port",
        ));
    };
    let Some((method, password)) = credentials.split_once(':') else {
        return Err(share_link_error(
            "the ss link decodes to no method:password pair",
        ));
    };
    let (server, port) = split_host_port(hostport)?;
    shadowsocks_outbound_for(method, password, &server, port)
}

fn shadowsocks_outbound_for(
    method: &str,
    password: &str,
    server: &str,
    port: u16,
) -> Result<Map<String, Value>> {
    if method.trim().is_empty() || password.is_empty() {
        return Err(share_link_error("the ss link has no method or password"));
    }
    let mut out = base_outbound("shadowsocks", server, port);
    out.insert(
        "method".to_string(),
        Value::String(method.trim().to_string()),
    );
    out.insert("password".to_string(), Value::String(password.to_string()));
    Ok(out)
}

fn vmess_outbound(rest: &str) -> Result<Map<String, Value>> {
    let link = ShareLink::parse(rest);
    let decoded = decode_base64(link.authority.trim())
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| {
            share_link_error("the vmess link is not valid base64")
                .with_hint("a vmess link reads vmess://base64(json)")
        })?;
    let tree: Value = serde_json::from_slice(&decoded)
        .map_err(|err| share_link_error(format!("the vmess link decodes to no JSON: {err}")))?;

    let field = |key: &str| match tree.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };

    let server = field("add");
    if server.trim().is_empty() {
        return Err(share_link_error("the vmess link has no server address"));
    }
    let port = parse_port(&field("port"))?;
    let uuid = field("id");
    if uuid.trim().is_empty() {
        return Err(share_link_error("the vmess link has no UUID"));
    }

    let mut out = base_outbound("vmess", server.trim(), port);
    out.insert("uuid".to_string(), Value::String(uuid.trim().to_string()));
    let cipher = field("scy");
    out.insert(
        "security".to_string(),
        json!(if cipher.trim().is_empty() {
            "auto"
        } else {
            cipher.trim()
        }),
    );
    let alter_id = field("aid").trim().parse::<u32>().unwrap_or(0);
    out.insert("alter_id".to_string(), json!(alter_id));

    let host = first_host(&field("host"));
    let sni = field("sni");
    apply_tls(
        &mut out,
        TlsOptions {
            security: Security::parse(&field("tls"), Security::None)?,
            server: server.trim(),
            sni: if sni.trim().is_empty() { &host } else { &sni },
            fingerprint: &field("fp"),
            public_key: "",
            short_id: "",
            insecure: false,
        },
    )?;

    let path = field("path");
    let service = match field("serviceName") {
        named if !named.is_empty() => named,
        _ => path.clone(),
    };
    apply_transport(
        &mut out,
        TransportOptions {
            kind: &field("net"),
            host: &host,
            path: &path,
            service: &service,
        },
    )?;
    Ok(out)
}

/// The first entry of a VMess `host` field, which may be a comma-separated list.
fn first_host(host: &str) -> String {
    host.split(',').next().unwrap_or("").trim().to_string()
}

fn parse_port(raw: &str) -> Result<u16> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_PORT);
    }
    trimmed
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| share_link_error(format!("the link has an invalid port {trimmed:?}")))
}

/// The skeleton every imported profile starts from: the same log, DNS, TUN
/// inbound and route rules as `assets/skeleton.json`, with one proxy outbound.
///
/// Built in code rather than embedded so the module has no build-time asset
/// dependency; it has to stay behaviourally identical to the skeleton, since a
/// profile the app creates and one it imports must connect the same way.
fn config_with_proxy(proxy: Map<String, Value>) -> Value {
    json!({
        "log": {
            "level": "info",
            "timestamp": true
        },
        "dns": {
            "servers": [
                {
                    "type": "https",
                    "tag": "cloudflare-doh",
                    "server": "1.1.1.1",
                    "domain_resolver": "local-dns",
                    "detour": "proxy"
                },
                {
                    "type": "local",
                    "tag": "local-dns",
                    "detour": "direct"
                }
            ],
            "strategy": "ipv4_only",
            "final": "cloudflare-doh"
        },
        "inbounds": [
            {
                "type": "tun",
                "tag": "tun-in",
                "interface_name": "",
                "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
                "auto_route": true,
                "strict_route": true,
                "stack": "gvisor",
                "mtu": 1350
            }
        ],
        "outbounds": [
            Value::Object(proxy),
            {"type": "direct", "tag": "direct"}
        ],
        "route": {
            "auto_detect_interface": true,
            "default_domain_resolver": {
                "server": "local-dns",
                "strategy": "ipv4_only"
            },
            "rules": [
                {"ip_version": 6, "action": "reject"},
                {"port": 53, "action": "hijack-dns"},
                {"inbound": "tun-in", "action": "sniff"},
                {"ip_is_private": true, "outbound": "direct"},
                {"outbound": "proxy"}
            ]
        }
    })
}

/// Decode standard or URL-safe base64, with or without padding.
///
/// Hand-rolled because a share link is the only base64 this crate reads, and a
/// dependency for thirty lines of alphabet table is not worth the build time.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut bits: u32 = 0;
    let mut bit_count: u32 = 0;
    let mut padded = false;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                padded = true;
                continue;
            }
            // Whitespace arrives when a link is wrapped by a chat client; any
            // other byte means this is not base64 at all.
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        // Data after padding is a truncated or concatenated blob, not a link.
        if padded {
            return None;
        }
        bits = (bits << 6) | u32::from(value);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
        }
    }
    Some(out)
}

/// Decode `%XX` escapes, leaving anything malformed as written.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(index + 1).copied().and_then(hex_value),
                bytes.get(index + 2).copied().and_then(hex_value),
            )
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ExitCode;

    const UUID: &str = "b831381d-6324-4d53-ad4f-8cda48b30811";
    const REALITY_KEY: &str = "jNXHt1yRo0vDuchQlIP6Z0ZvjT3KtzVI-T4E7RoLJS0";

    /// The first outbound of the generated config, i.e. the parsed link.
    fn outbound(link: &str) -> Result<Value> {
        let bytes = profile_from_link(link)?;
        let tree: Value = serde_json::from_slice(&bytes)
            .map_err(|err| SingSwitchError::general(format!("generated no JSON: {err}")))?;
        Ok(tree
            .get("outbounds")
            .and_then(Value::as_array)
            .and_then(|outbounds| outbounds.first())
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// The whole generated document, for asserting on the skeleton sections.
    fn config(link: &str) -> Result<Value> {
        let bytes = profile_from_link(link)?;
        serde_json::from_slice(&bytes)
            .map_err(|err| SingSwitchError::general(format!("generated no JSON: {err}")))
    }

    fn base64(input: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = input.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut block = [0u8; 3];
            block[..chunk.len()].copy_from_slice(chunk);
            let value =
                (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
            for shift in [18, 12, 6, 0] {
                out.push(char::from(ALPHABET[((value >> shift) & 0x3f) as usize]));
            }
            for pad in 0..(3 - chunk.len()) {
                out.pop();
                let _ = pad;
            }
        }
        out
    }

    #[test]
    fn vless_reality_link_keeps_reality_fields() -> Result<()> {
        let out = outbound(&format!(
            "vless://{UUID}@203.0.113.9:443?type=tcp&security=reality&sni=www.example.com&fp=firefox&pbk={REALITY_KEY}&sid=ab12&flow=xtls-rprx-vision#home"
        ))?;

        assert_eq!(out["type"], json!("vless"));
        assert_eq!(out["tag"], json!("proxy"));
        assert_eq!(out["server"], json!("203.0.113.9"));
        assert_eq!(out["server_port"], json!(443));
        assert_eq!(out["uuid"], json!(UUID));
        assert_eq!(out["flow"], json!("xtls-rprx-vision"));
        assert_eq!(out["tls"]["enabled"], json!(true));
        assert_eq!(out["tls"]["server_name"], json!("www.example.com"));
        assert_eq!(out["tls"]["utls"]["fingerprint"], json!("firefox"));
        assert_eq!(out["tls"]["reality"]["enabled"], json!(true));
        assert_eq!(out["tls"]["reality"]["public_key"], json!(REALITY_KEY));
        assert_eq!(out["tls"]["reality"]["short_id"], json!("ab12"));
        // tcp carries no transport section.
        assert_eq!(out.get("transport"), None);
        Ok(())
    }

    #[test]
    fn vless_websocket_link_carries_path_and_host() -> Result<()> {
        let out = outbound(&format!(
            "vless://{UUID}@edge.example.com:8443?type=ws&security=tls&sni=edge.example.com&host=cdn.example.com&path=%2Fws%3Fed%3D2048#ws"
        ))?;

        assert_eq!(out["server_port"], json!(8443));
        assert_eq!(out["transport"]["type"], json!("ws"));
        assert_eq!(out["transport"]["path"], json!("/ws?ed=2048"));
        assert_eq!(
            out["transport"]["headers"]["Host"],
            json!("cdn.example.com")
        );
        // No fingerprint in the link: no uTLS block, and verification stays on.
        assert_eq!(out["tls"]["utls"], Value::Null);
        assert_eq!(out["tls"]["insecure"], Value::Null);
        Ok(())
    }

    #[test]
    fn vless_grpc_and_httpupgrade_links_map_to_transports() -> Result<()> {
        let grpc = outbound(&format!(
            "vless://{UUID}@g.example.com:443?type=grpc&security=tls&sni=g.example.com&serviceName=gsvc"
        ))?;
        assert_eq!(grpc["transport"]["type"], json!("grpc"));
        assert_eq!(grpc["transport"]["service_name"], json!("gsvc"));

        let upgrade = outbound(&format!(
            "vless://{UUID}@u.example.com:443?type=httpupgrade&security=tls&host=u.example.com&path=/up"
        ))?;
        assert_eq!(upgrade["transport"]["type"], json!("httpupgrade"));
        assert_eq!(upgrade["transport"]["path"], json!("/up"));
        assert_eq!(upgrade["transport"]["host"], json!("u.example.com"));
        Ok(())
    }

    #[test]
    fn trojan_link_defaults_to_tls_with_the_server_as_sni() -> Result<()> {
        let out = outbound("trojan://p%40ssw0rd@t.example.com:443#trojan")?;

        assert_eq!(out["type"], json!("trojan"));
        assert_eq!(out["password"], json!("p@ssw0rd"));
        assert_eq!(out["tls"]["enabled"], json!(true));
        assert_eq!(out["tls"]["server_name"], json!("t.example.com"));
        assert_eq!(out["tls"]["reality"], Value::Null);

        let plain = outbound("trojan://secret@t.example.com:443?security=none")?;
        assert_eq!(plain.get("tls"), None);
        Ok(())
    }

    #[test]
    fn hysteria2_link_maps_insecure_and_obfs() -> Result<()> {
        let out = outbound(
            "hy2://auth-token@h.example.com:8443?sni=h.example.com&insecure=1&obfs=salamander&obfs-password=obfs-key#hy",
        )?;

        assert_eq!(out["type"], json!("hysteria2"));
        assert_eq!(out["server_port"], json!(8443));
        assert_eq!(out["password"], json!("auth-token"));
        assert_eq!(out["tls"]["enabled"], json!(true));
        assert_eq!(out["tls"]["insecure"], json!(true));
        assert_eq!(out["obfs"]["type"], json!("salamander"));
        assert_eq!(out["obfs"]["password"], json!("obfs-key"));
        Ok(())
    }

    #[test]
    fn ss_sip002_link_decodes_its_credentials() -> Result<()> {
        let userinfo = base64("aes-256-gcm:p@ss:word");
        let out = outbound(&format!("ss://{userinfo}@s.example.com:8388#ss"))?;

        assert_eq!(out["type"], json!("shadowsocks"));
        assert_eq!(out["method"], json!("aes-256-gcm"));
        assert_eq!(out["password"], json!("p@ss:word"));
        assert_eq!(out["server_port"], json!(8388));
        Ok(())
    }

    #[test]
    fn ss_legacy_link_decodes_the_whole_blob() -> Result<()> {
        let blob = base64("chacha20-ietf-poly1305:hunter2@s.example.com:8389");
        let out = outbound(&format!("ss://{blob}#legacy"))?;

        assert_eq!(out["type"], json!("shadowsocks"));
        assert_eq!(out["method"], json!("chacha20-ietf-poly1305"));
        assert_eq!(out["password"], json!("hunter2"));
        assert_eq!(out["server"], json!("s.example.com"));
        assert_eq!(out["server_port"], json!(8389));
        Ok(())
    }

    #[test]
    fn vmess_link_reads_its_json() -> Result<()> {
        let body = format!(
            r#"{{"v":"2","ps":"phone","add":"v.example.com","port":"443","id":"{UUID}","aid":"0","scy":"auto","net":"ws","type":"none","host":"cdn.example.com","path":"/vm","tls":"tls","sni":"v.example.com"}}"#
        );
        let out = outbound(&format!("vmess://{}", base64(&body)))?;

        assert_eq!(out["type"], json!("vmess"));
        assert_eq!(out["server"], json!("v.example.com"));
        assert_eq!(out["server_port"], json!(443));
        assert_eq!(out["uuid"], json!(UUID));
        assert_eq!(out["security"], json!("auto"));
        assert_eq!(out["alter_id"], json!(0));
        assert_eq!(out["tls"]["server_name"], json!("v.example.com"));
        assert_eq!(out["transport"]["type"], json!("ws"));
        assert_eq!(out["transport"]["path"], json!("/vm"));
        assert_eq!(
            out["transport"]["headers"]["Host"],
            json!("cdn.example.com")
        );
        Ok(())
    }

    #[test]
    fn vmess_link_accepts_unpadded_url_safe_base64() -> Result<()> {
        let body = format!(
            r#"{{"add":"g.example.com","port":443,"id":"{UUID}","net":"grpc","path":"gsvc","tls":""}}"#
        );
        let padded = base64(&body);
        let bare = padded
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_");
        let out = outbound(&format!("vmess://{bare}"))?;

        assert_eq!(out["server"], json!("g.example.com"));
        assert_eq!(out["transport"]["type"], json!("grpc"));
        assert_eq!(out["transport"]["service_name"], json!("gsvc"));
        assert_eq!(out.get("tls"), None);
        Ok(())
    }

    #[test]
    fn generated_config_keeps_the_skeleton_sections() -> Result<()> {
        let tree = config(&format!(
            "vless://{UUID}@a.example.com:443?security=tls&sni=a.example.com#a"
        ))?;

        assert_eq!(tree["log"]["level"], json!("info"));
        assert_eq!(tree["dns"]["final"], json!("cloudflare-doh"));
        assert_eq!(tree["inbounds"][0]["type"], json!("tun"));
        assert_eq!(tree["inbounds"][0]["stack"], json!("gvisor"));
        assert_eq!(tree["outbounds"][1]["type"], json!("direct"));
        assert_eq!(tree["route"]["rules"][4]["outbound"], json!("proxy"));
        // The imported outbound is what the guided form manages.
        let model = crate::form_model::ProfileFormModel::from_value(&tree);
        assert!(model.has_managed_outbound);
        assert_eq!(model.outbound_type, "vless");
        assert!(model.missing().is_empty(), "{:?}", model.missing());
        Ok(())
    }

    #[test]
    fn malformed_links_are_rejected() {
        let cases = [
            // Unknown or missing scheme.
            "http://example.com/config.json",
            "sing-box://whatever",
            "vless",
            "",
            // Missing host or credentials.
            "vless://@a.example.com:443",
            "vless://b831-...:443",
            "trojan://a.example.com:443",
            "hy2://",
            // Bad ports and transports.
            "vless://id@a.example.com:99999",
            "vless://id@a.example.com:443?type=quic",
            "vless://id@a.example.com:443?security=shadowsocks",
            // Reality without its key.
            &format!("vless://{UUID}@a.example.com:443?security=reality&sni=a.example.com"),
            // Base64 that is not base64, and JSON that is not a config.
            "vmess://not base64!!",
            "ss://not-base64!!@s.example.com:8388",
            "ss://bm8tY29sb24@a.example.com:1",
        ];

        for link in cases {
            let err = profile_from_link(link).err();
            assert_eq!(
                err.as_ref().map(|e| e.code),
                Some(ExitCode::Validation),
                "{link:?} should be rejected as invalid"
            );
            // A rejected link must not put its credentials in the message.
            let message = err.map(|e| e.message.clone()).unwrap_or_default();
            assert!(!message.contains(UUID), "{link:?} leaked the UUID");
        }
    }

    /// A `Checker` that answers from a flag, so the import path can be tested
    /// without a sing-box binary on the host.
    struct StubChecker {
        accept: bool,
    }

    impl Checker for StubChecker {
        fn check(&self, path: &Path) -> Result<()> {
            if self.accept {
                Ok(())
            } else {
                Err(SingSwitchError::invalid_config(
                    path.display().to_string(),
                    "stub rejection",
                ))
            }
        }

        fn version(&self) -> Result<String> {
            Ok("stub".to_string())
        }
    }

    fn scratch_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "singswitch-link-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn import_validates_before_writing_and_refuses_to_overwrite() -> Result<()> {
        let dir = scratch_dir("import");
        let link = "trojan://secret@t.example.com:443";
        let rejecting = StubChecker { accept: false };
        let accepting = StubChecker { accept: true };

        let rejected = import_profile(&dir, &rejecting, "imported", link);
        let first = import_profile(&dir, &accepting, "imported", link);
        let second = import_profile(&dir, &accepting, "imported", link);
        let _ = fs::remove_dir_all(&dir);

        assert!(rejected.is_err(), "a rejected config must not be stored");
        assert!(first.is_ok(), "the first import should write: {first:?}");
        assert!(second.is_err(), "a second import must not overwrite");
        Ok(())
    }
}
