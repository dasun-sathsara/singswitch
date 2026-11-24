use std::path::PathBuf;

use gpui_kit::{App, Entity, SharedString};
use singswitch_core::controller::{Snapshot, StatusView};
use singswitch_core::ip::IpInfo;
use singswitch_core::logs::LogLevel;
use singswitch_core::profile::{ConfigFacts, Profile};
use singswitch_core::readiness::Field;
use singswitch_core::service::{Health, RunState};

use crate::actions::NavTab;
use crate::state::{AppStore, LogRecord};
use crate::theme::ThemePreference;

pub fn preview_scene() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--preview" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
        if let Some(val) = arg.strip_prefix("--preview=") {
            return Some(val.to_string());
        }
    }
    std::env::var("SINGSWITCH_PREVIEW").ok()
}

pub fn preview_theme() -> Option<ThemePreference> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--theme" && i + 1 < args.len() {
            return match args[i + 1].to_lowercase().as_str() {
                "dark" => Some(ThemePreference::Dark),
                "light" => Some(ThemePreference::Light),
                _ => Some(ThemePreference::System),
            };
        }
        if let Some(val) = arg.strip_prefix("--theme=") {
            return match val.to_lowercase().as_str() {
                "dark" => Some(ThemePreference::Dark),
                "light" => Some(ThemePreference::Light),
                _ => Some(ThemePreference::System),
            };
        }
    }
    std::env::var("SINGSWITCH_THEME")
        .ok()
        .map(|t| match t.to_lowercase().as_str() {
            "dark" => ThemePreference::Dark,
            "light" => ThemePreference::Light,
            _ => ThemePreference::System,
        })
}

pub fn apply_preview_state(scene: &str, store: &Entity<AppStore>, cx: &mut App) {
    store.update(cx, |s, cx| {
        s.init_error = None;
        s.notice = None;
        s.probed_once = true;
        s.settings.window_width = crate::theme::DEFAULT_WINDOW_WIDTH;
        s.settings.window_height = crate::theme::DEFAULT_WINDOW_HEIGHT;

        match scene {
            "empty" => {
                s.snapshot = Snapshot {
                    health: Health::unknown(RunState::Stopped),
                    active: None,
                    running: false,
                    profiles: Vec::new(),
                    broken: None,
                    status_view: StatusView {
                        state: RunState::Stopped,
                        profile: None,
                        running: false,
                        tun: None,
                        server: None,
                        pid: None,
                        missing: Vec::new(),
                        broken: None,
                    },
                };
                s.selected_tab = NavTab::Profiles;
                s.selected_profile = None;
                s.ip_info = None;
                s.ip_loading = false;
            }
            "connections" | "profiles" => {
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Profiles;
                s.editing = false;
            }
            "notice" => {
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Profiles;
                s.editing = false;
                s.notice = Some(crate::state::Notice {
                    origin: crate::state::NoticeOrigin::Stage,
                    tone: crate::state::BannerTone::Error,
                    message: "sing-box exited with code 1: inbound/tun: configure interface utun123 failed".into(),
                });
            }
            "editor-guided" => {
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Profiles;
                s.editing = true;
            }
            "editor-json" => {
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Profiles;
                s.editing = true;
                s.editor_json_mode = true;
            }
            "logs" => {
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Logs;
                s.editing = false;
                setup_sample_logs(s);
            }
            "nodes" => {
                // The preview's active config declares a selector, a urltest
                // and a `clash_api`; the daemon is not running under snap.py,
                // so the declared groups render with no live overlay — exactly
                // the state a config-only capture shows.
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Profiles;
                s.editing = false;
            }
            "settings" => {
                setup_populated_snapshot(s);
                s.selected_tab = NavTab::Settings;
                s.editing = false;
            }
            _ => {
                setup_populated_snapshot(s);
            }
        }
        cx.notify();
    });
}

fn setup_populated_snapshot(s: &mut AppStore) {
    let preview_dir = std::path::PathBuf::from("/tmp/singswitch_preview");
    let _ = std::fs::create_dir_all(&preview_dir);
    let p1_path = preview_dir.join("home-vless.json");
    // Every populated scene points the active config here, so captures never
    // depend on whatever /usr/local/etc/sing-box/config.json happens to be on
    // the machine taking them. Without this, a box with selector groups in
    // its real config rendered them into the connections scene.
    s.preview_point_active_config(p1_path.clone());
    let p1_content = r#"{
  "log": { "level": "info", "timestamp": true },
  "dns": {
    "servers": [
      { "type": "https", "tag": "cloudflare-doh", "server": "1.1.1.1", "detour": "proxy" },
      { "type": "local", "tag": "local-dns", "detour": "direct" }
    ],
    "strategy": "ipv4_only",
    "final": "cloudflare-doh"
  },
  "inbounds": [
    {
      "type": "tun",
      "tag": "tun-in",
      "interface_name": "utun123",
      "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
      "auto_route": true,
      "strict_route": true,
      "stack": "system",
      "mtu": 9000
    }
  ],
  "outbounds": [
    {
      "type": "vless",
      "tag": "proxy",
      "server": "104.28.19.42",
      "server_port": 443,
      "uuid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
      "flow": "xtls-rprx-vision",
      "tls": {
        "enabled": true,
        "server_name": "gateway.cloudflare.com",
        "utls": { "enabled": true, "fingerprint": "chrome" },
        "reality": {
          "enabled": true,
          "public_key": "xK8L9mN0pQ1rS2tU3vW4xY5z6A7bC8dE9fG0hI1jK2l=",
          "short_id": "0123456789abcdef"
        }
      }
    },
    {
      "type": "selector",
      "tag": "region",
      "outbounds": ["proxy", "direct"]
    },
    {
      "type": "urltest",
      "tag": "auto",
      "outbounds": ["proxy"]
    },
    { "type": "direct", "tag": "direct" }
  ],
  "experimental": {
    "clash_api": { "external_controller": "127.0.0.1:19955" }
  },
  "route": {
    "auto_detect_interface": true,
    "final": "proxy",
    "rules": [
      { "action": "route", "outbound": "direct", "rule_set": ["geoip-private"] }
    ]
  }
}"#;
    let _ = std::fs::write(&p1_path, p1_content);

    let p1 = Profile {
        name: "home-vless".to_string(),
        path: p1_path,
        missing: Vec::new(),
        source_url: None,
        config_parsed: true,
        facts: ConfigFacts {
            outbound_type: Some("vless".into()),
            endpoint: Some("104.28.19.42:443".into()),
            tun_interface: Some("utun123".into()),
            has_tun_inbound: true,
        },
    };

    let p2 = Profile {
        name: "tokyo-node".to_string(),
        path: PathBuf::from("/usr/local/etc/sing-box/profiles/tokyo-node.json"),
        missing: Vec::new(),
        source_url: Some("https://nodes.example.com/tokyo.json".into()),
        config_parsed: true,
        facts: ConfigFacts {
            outbound_type: Some("hysteria2".into()),
            endpoint: Some("162.159.192.1:8443".into()),
            tun_interface: Some("utun123".into()),
            has_tun_inbound: true,
        },
    };

    let p3 = Profile {
        name: "singapore-vless".to_string(),
        path: PathBuf::from("/usr/local/etc/sing-box/profiles/singapore-vless.json"),
        missing: Vec::new(),
        source_url: None,
        config_parsed: true,
        facts: ConfigFacts {
            outbound_type: Some("vless".into()),
            endpoint: Some("198.41.200.5:443".into()),
            tun_interface: Some("utun123".into()),
            has_tun_inbound: true,
        },
    };

    let p4 = Profile {
        name: "backup-shadowsocks".to_string(),
        path: PathBuf::from("/usr/local/etc/sing-box/profiles/backup-shadowsocks.json"),
        missing: vec![Field::Password],
        source_url: None,
        config_parsed: true,
        facts: ConfigFacts {
            outbound_type: Some("shadowsocks".into()),
            endpoint: Some("45.76.12.89:8388".into()),
            tun_interface: None,
            has_tun_inbound: true,
        },
    };

    let profiles = vec![p1, p2, p3, p4];

    s.snapshot = Snapshot {
        health: Health {
            state: RunState::Running,
            pid: Some(18420),
            detail: "sing-box 1.11.0 running (pid 18420)".to_string(),
            last_exit: None,
            restarts: Some(0),
        },
        active: Some("home-vless".to_string()),
        running: true,
        profiles,
        broken: None,
        status_view: StatusView {
            state: RunState::Running,
            profile: Some("home-vless".to_string()),
            running: true,
            tun: Some("utun123".to_string()),
            server: Some("104.28.19.42:443".to_string()),
            pid: Some(18420),
            missing: Vec::new(),
            broken: None,
        },
    };

    s.selected_profile = Some("home-vless".to_string());
    s.ip_info = Some(IpInfo {
        ip: "103.219.171.115".to_string(),
        city: "Singapore".to_string(),
        region: "Central".to_string(),
        country: "SG".to_string(),
        location: "1.2897,103.8501".to_string(),
        network: "AS13335 Cloudflare, Inc.".to_string(),
        postal: "048581".to_string(),
        timezone: "Asia/Singapore".to_string(),
    });
    s.ip_loading = false;
    s.ip_error = None;
}

fn setup_sample_logs(s: &mut AppStore) {
    let raw_entries = [
        (
            LogLevel::Info,
            "14:22:01.102 sing-box 1.11.0 initialized with configuration /usr/local/etc/sing-box/config.json",
        ),
        (
            LogLevel::Info,
            "14:22:01.105 [inbound/tun-in] inbound created: interface utun123, mtu 9000, stack system",
        ),
        (
            LogLevel::Info,
            "14:22:01.112 [router] default route rules initialized (14 rules loaded)",
        ),
        (
            LogLevel::Info,
            "14:22:01.140 [outbound/vless-out] TLS Reality handshake completed with 104.28.19.42:443",
        ),
        (
            LogLevel::Info,
            "14:22:02.320 [inbound/tun-in] connection from 198.18.0.1:52194 to tcp:1.1.1.1:443 routed to [outbound/vless-out]",
        ),
        (
            LogLevel::Info,
            "14:22:05.811 [dns] query response: resolved api.github.com to 140.82.121.4 (via remote DNS)",
        ),
        (
            LogLevel::Warn,
            "14:22:18.490 [outbound/vless-out] handshake latency 142ms exceeded target threshold 100ms",
        ),
        (
            LogLevel::Info,
            "14:22:25.100 [router] matched rule geoip:private -> direct",
        ),
        (
            LogLevel::Error,
            "14:23:05.612 [outbound/vless-out] connection reset by peer (server closed stream 4)",
        ),
        (
            LogLevel::Info,
            "14:23:06.015 [outbound/vless-out] re-established connection to 104.28.19.42:443 (tls reality)",
        ),
        (
            LogLevel::Info,
            "14:23:40.910 [inbound/tun-in] TCP flow finished: 2.4 MB ingress, 480 KB egress",
        ),
    ];

    s.error_logs = raw_entries
        .iter()
        .enumerate()
        .map(|(id, (level, msg))| LogRecord {
            id: id as u64 + 1,
            line: SharedString::from(*msg),
            level: *level,
        })
        .collect();
}
