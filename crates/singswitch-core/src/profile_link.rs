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
fn profile_from_link(link: &str) -> Result<Vec<u8>> {
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
    // Byte length, not char count: the cap exists to bound the allocation a
    // hostile paste can demand, so the cheaper and stricter measure is the
    // right one — a multi-byte link is rejected slightly earlier, never later.
    if trimmed.len() > MAX_LINK_CHARS {
        return Err(share_link_error(
            "the share link is far longer than any link",
        ));
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        // Never echo the link: what was pasted may be userinfo or a URL, and
        // the error lands in a banner people screenshot.
        return Err(share_link_error(
            "this does not look like a share link (no scheme:// prefix)",
        ));
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

// Never echo `rest` (or any part after the scheme): it can hold the pasted
// userinfo, which is a password or a UUID, and the error lands in a banner
// people screenshot.

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
            "none" | "false" => Ok(Self::None),
            "tls" | "true" => Ok(Self::Tls),
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
        // The official hysteria2 and ss URI schemes put a path before the
        // query (`hysteria2://auth@host:443/?insecure=1`, Outline keys end
        // `@host:8388/?outline=1`). These links carry no meaningful path — the
        // forms in the wild use it as a separator or mount noise markers in
        // the query — so everything from the first `/` on is dropped from the
        // authority; `example.com:443/` must not become a port `443/` or a
        // host `example.com/`.
        let authority = authority.split('/').next().unwrap_or(authority);
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
            // `]` may be followed only by `:port`; anything else (`]junk:8443`)
            // is malformed authority, and swallowing it would connect
            // somewhere other than what the link says.
            let port = match tail.strip_prefix(':') {
                Some(port) => port,
                None if tail.is_empty() => "",
                None => {
                    return Err(share_link_error(
                        "the share link has extra text after the IPv6 address",
                    )
                    .with_hint("write an IPv6 address as [2001:db8::1]:443"));
                }
            };
            (host.to_string(), port)
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
    // A percent sign in the authority stays percent-encoded in the host
    // (`ex%61mple.com`), which DNS then fails on — and the query half of the
    // same link decodes its values, so the two halves would disagree. URLs
    // in the wild do not percent-encode reg-names; decode here for parity.
    let host = if host.contains('%') {
        percent_decode(&host)
    } else {
        host
    };
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
    // server_name travels into the TLS handshake; a CR/LF in it is header
    // injection, not a hostname. Reject rather than trying to strip: a value
    // with control bytes is not something the user meant to paste.
    if server_name.as_bytes().iter().any(|b| b.is_ascii_control()) {
        return Err(
            share_link_error("the link's server name contains control characters")
                .with_hint("copy the sni= value unchanged from your provider's link"),
        );
    }
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
        // Reality's short ID is up to 8 bytes, hex. A longer one panics deep
        // inside sing-box (index out of range decoding hex), surfacing a Go
        // stack trace in the import banner; reject it here with a readable
        // reason. Empty is legal (servers may be configured without one).
        let short_id = opts.short_id.trim();
        if !short_id.is_empty() {
            let valid = short_id.len() <= 16
                && short_id.len().is_multiple_of(2)
                && short_id.bytes().all(|b| b.is_ascii_hexdigit());
            if !valid {
                return Err(share_link_error(
                    "the Reality short ID (sid=) must be up to 16 hex characters",
                )
                .with_hint("copy the sid value unchanged from your provider's link"));
            }
        }
        tls.insert(
            "reality".to_string(),
            json!({
                "enabled": true,
                "public_key": opts.public_key.trim(),
                "short_id": short_id,
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
    // These values ride in handshake headers (Host, path, gRPC service name).
    // A CR/LF in any of them is an injected header line, which the link's
    // author controls verbatim; refuse instead of allowing the outbound to
    // speak with extra headers nobody asked for.
    if [host, path, service]
        .iter()
        .any(|value| value.as_bytes().iter().any(|b| b.is_ascii_control()))
    {
        return Err(
            share_link_error("the link's transport parameters contain control characters")
                .with_hint("copy the type/path/host values unchanged from your provider's link"),
        );
    }

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
    let security = Security::parse(link.get("security"), Security::None)?;
    // Vision flow rides only inside raw TLS; on ws/grpc/http transports the
    // connection is not a TLS record at all, and sing-box passes `check` and
    // then fails at connect (`not a valid supported TLS connection:` etc.).
    // Providers do ship flow= on such links, so drop it with a reason rather
    // than emitting a config that cannot connect.
    let transport = link.get("type");
    if !flow.is_empty() {
        let plain_tcp = transport.is_empty() || transport.eq_ignore_ascii_case("tcp");
        let vision_ok = plain_tcp && matches!(security, Security::Tls | Security::Reality);
        if vision_ok {
            out.insert("flow".to_string(), Value::String(flow.to_string()));
        } else {
            return Err(share_link_error(format!(
                "flow={flow:?} needs raw TCP with TLS or REALITY; this link's transport ({transport:?}/security) cannot carry it"
            ))
            .with_hint("remove flow, or change type= to tcp with security=tls"));
        }
    }
    apply_tls(
        &mut out,
        TlsOptions {
            security,
            server: &server,
            sni: link.get("sni"),
            fingerprint: link.get("fp"),
            public_key: link.get("pbk"),
            short_id: link.get("sid"),
            // `allowInsecure` is the spelling some providers export (Shadowrocket
            // among others); a parameter that changes whether a link connects
            // cannot be silently dropped.
            insecure: link.flag("insecure") || link.flag("allowInsecure"),
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
            insecure: link.flag("insecure") || link.flag("allowInsecure"),
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
    // QUIC is always TLS here: sing-box rejects a hysteria2 outbound without
    // it. REALITY is not a thing over hysteria2, so say that instead of
    // failing with "the Reality link has no public key" on a complete link.
    let security = Security::parse(link.get("security"), Security::Tls)?;
    if security == Security::Reality {
        return Err(
            share_link_error("REALITY is not supported on hysteria2 links")
                .with_hint("remove security=reality, or use the vless:// link instead"),
        );
    }
    apply_tls(
        &mut out,
        TlsOptions {
            security,
            server: &server,
            sni: link.get("sni"),
            fingerprint: link.get("fp"),
            public_key: "",
            short_id: "",
            insecure: link.flag("insecure") || link.flag("allowInsecure"),
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
    // The legacy form is chosen because the link has no @ in its authority;
    // its query is still real (Outline keys carry ?outline=1 and SIP003
    // plugins ride the query), and dropping a plugin silently imports a plain
    // outbound that cannot connect to a server that requires obfs.
    if !link.get("plugin").is_empty() {
        return Err(share_link_error(
            "this ss link needs a SIP003 plugin, which singswitch cannot import",
        )
        .with_hint("ask your provider for a link without a plugin"));
    }
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

/// vmess base64 holds JSON whose types vary by generator (port as a number
/// or a string are both common). Coerce only the types that carry the same
/// value (a number to its digits); reject the rest: `null` does not mean the
/// string "null", and an array host connects nowhere while passing `check`.
fn vmess_field_tree(tree: Value) -> Result<Value> {
    if !matches!(tree, Value::Object(_) | Value::Null) {
        return Err(share_link_error("the vmess link decodes to no JSON object")
            .with_hint("a vmess link reads vmess://base64(json)"));
    }
    Ok(tree)
}

fn vmess_field(tree: &Value, key: &str) -> Result<String> {
    match tree.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Number(number)) => Ok(number.to_string()),
        Some(Value::Bool(true)) => Ok("true".to_string()),
        Some(Value::Bool(false)) => Ok("false".to_string()),
        Some(Value::Null) | None => Ok(String::new()),
        Some(_) => Err(share_link_error(format!(
            "the vmess link's {key:?} field is not a string or number"
        ))
        .with_hint("re-export the subscription from your provider")),
    }
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

    // vmess base64 holds JSON whose types vary by generator (port as number
    // or string is both common), and the schema has no fixed home.
    let tree = vmess_field_tree(tree)?;
    let field = |key: &str| vmess_field(&tree, key);

    let server = field("add")?;
    if server.trim().is_empty() {
        return Err(share_link_error("the vmess link has no server address"));
    }
    let port = parse_port(&field("port")?)?;
    let uuid = field("id")?;
    if uuid.trim().is_empty() {
        return Err(share_link_error("the vmess link has no UUID"));
    }

    let mut out = base_outbound("vmess", server.trim(), port);
    out.insert("uuid".to_string(), Value::String(uuid.trim().to_string()));
    out.insert("security".to_string(), json!(vmess_cipher(&field("scy")?)));
    out.insert(
        "alter_id".to_string(),
        json!(field("aid")?.trim().parse::<u32>().unwrap_or(0)),
    );
    apply_vmess_tls_and_transport(&mut out, server.trim(), &field)?;
    Ok(out)
}

/// `scy` is optional in the wild; sing-box's `auto` keeps both AES and
/// ChaCha20 working without the link naming one.
fn vmess_cipher(cipher: &str) -> &str {
    if cipher.trim().is_empty() {
        "auto"
    } else {
        cipher.trim()
    }
}

/// vmess exports no certificate waiver in the wild, and no reality: the
/// v2rayN base64 schema carries neither. The sni falls back to the transport
/// host because generators put the certificate name there.
fn apply_vmess_tls_and_transport(
    out: &mut Map<String, Value>,
    server: &str,
    field: &impl Fn(&str) -> Result<String>,
) -> Result<()> {
    let host = first_host(&field("host")?);
    let sni = field("sni")?;
    let sni = if sni.trim().is_empty() { &host } else { &sni };
    apply_tls(
        out,
        TlsOptions {
            security: Security::parse(&field("tls")?, Security::None)?,
            server,
            sni,
            fingerprint: &field("fp")?,
            public_key: "",
            short_id: "",
            insecure: false,
        },
    )?;

    let path = field("path")?;
    let service = match field("serviceName")? {
        named if !named.is_empty() => named,
        _ => path.clone(),
    };
    apply_transport(
        out,
        TransportOptions {
            kind: &field("net")?,
            host: &host,
            path: &path,
            service: &service,
        },
    )
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
    // Fast path: nothing to decode, so hand the input back untouched rather
    // than round-tripping it through a byte buffer and a lossy copy.
    if !raw.as_bytes().contains(&b'%') {
        return raw.to_string();
    }
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
