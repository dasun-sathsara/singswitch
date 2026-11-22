use std::io::IsTerminal;

use sbctl_core::controller::StatusView;
use sbctl_core::error::SbctlError;
use sbctl_core::ip::IpInfo;
use sbctl_core::profile::Profile;
use sbctl_core::service::RunState;

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    pub plain: bool,
    pub color: bool,
}

impl RenderOptions {
    pub fn resolve(plain_flag: bool, no_color_flag: bool) -> Self {
        let term_dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
        let no_color_env = std::env::var_os("NO_COLOR").is_some();
        let is_tty = std::io::stdout().is_terminal();

        Self {
            plain: plain_flag || term_dumb,
            color: !plain_flag && !no_color_flag && !no_color_env && !term_dumb && is_tty,
        }
    }
}

pub struct Theme {
    opts: RenderOptions,
}

impl Theme {
    pub fn new(opts: RenderOptions) -> Self {
        Self { opts }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.opts.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn ok(&self, msg: &str) -> String {
        let glyph = if self.opts.plain { "[ok]" } else { "✓" };
        format!("{} {msg}", self.paint("32", glyph))
    }

    pub fn warn(&self, msg: &str) -> String {
        let glyph = if self.opts.plain { "[warn]" } else { "⚠" };
        format!("{} {msg}", self.paint("33", glyph))
    }

    pub fn fail(&self, msg: &str) -> String {
        let glyph = if self.opts.plain { "[err]" } else { "✗" };
        format!("{} {msg}", self.paint("31", glyph))
    }

    pub fn hint(&self, msg: &str) -> String {
        let arrow = if self.opts.plain { "->" } else { "→" };
        self.paint("2", &format!("  {arrow} {msg}"))
    }

    pub fn muted(&self, msg: &str) -> String {
        self.paint("2", msg)
    }

    pub fn state_badge(&self, state: RunState) -> String {
        match state {
            RunState::Running => {
                let s = if self.opts.plain {
                    "running"
                } else {
                    "● running"
                };
                self.paint("32", s)
            }
            RunState::Stopped => {
                let s = if self.opts.plain {
                    "stopped"
                } else {
                    "○ stopped"
                };
                self.paint("2", s)
            }
            RunState::Error => {
                let s = if self.opts.plain {
                    "error"
                } else {
                    "✗ error"
                };
                self.paint("31", s)
            }
        }
    }

    pub fn kv_block(&self, rows: &[(&str, String)]) -> String {
        let label_width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let mut out = String::new();
        for (k, v) in rows {
            if v.is_empty() {
                continue;
            }
            let pad = " ".repeat(label_width.saturating_sub(k.len()));
            out.push_str(&format!("{}  {v}\n", self.paint("2", &format!("{k}{pad}"))));
        }
        out
    }

    pub fn render_status(&self, view: &StatusView) -> String {
        let mut rows: Vec<(&str, String)> = vec![("state", self.state_badge(view.state))];

        let profile_val = match (&view.profile, view.running) {
            (None, _) => String::new(),
            (Some(name), true) => name.clone(),
            (Some(name), false) => format!("{name}  {}", self.muted("(configured, not running)")),
        };
        if !profile_val.is_empty() {
            rows.push(("profile", profile_val));
        }
        if let Some(server) = &view.server {
            rows.push(("server", server.clone()));
        }
        if let Some(tun) = &view.tun {
            rows.push(("tun", tun.clone()));
        }
        if let Some(pid) = view.pid
            && pid > 0
            && view.state == RunState::Running
        {
            rows.push(("pid", pid.to_string()));
        }

        let mut body = self.kv_block(&rows);
        if !view.missing.is_empty() {
            body.push_str(&self.warn(&format!(
                "active profile still has placeholders: {}",
                sbctl_core::readiness::join_labels(&view.missing)
            )));
            body.push('\n');
        }
        if let Some(broken) = &view.broken {
            body.push_str(&self.fail(broken));
            body.push('\n');
        }

        if self.opts.plain {
            return body;
        }

        let content_lines: Vec<&str> = body.trim_end_matches('\n').lines().collect();
        let inner_width = content_lines
            .iter()
            .map(|l| visible_width(l))
            .max()
            .unwrap_or(10)
            .max(12);

        let mut boxed = String::new();
        boxed.push_str(&format!(
            "╭─ {} {}╮\n",
            self.paint("1", "sbctl"),
            "─".repeat(inner_width.saturating_sub(6))
        ));
        for line in content_lines {
            let pad = " ".repeat(inner_width.saturating_sub(visible_width(line)));
            boxed.push_str(&format!("│  {line}{pad}  │\n"));
        }
        boxed.push_str(&format!("╰{}╯\n", "─".repeat(inner_width + 4)));
        boxed
    }

    pub fn render_profile_list(
        &self,
        profiles: &[Profile],
        active: Option<&str>,
        running: bool,
    ) -> String {
        if profiles.is_empty() {
            return format!("{}\n", self.muted("no profiles yet"));
        }

        let mut rows = Vec::with_capacity(profiles.len());
        for p in profiles {
            let is_active = active == Some(p.name.as_str());
            let (glyph, note, color) = if !p.ready() {
                (
                    if self.opts.plain { "!" } else { "⚠" },
                    "placeholders",
                    "33",
                )
            } else if is_active && running {
                (if self.opts.plain { "*" } else { "●" }, "active", "32")
            } else if is_active {
                (
                    if self.opts.plain { "*" } else { "●" },
                    "configured, not running",
                    "2",
                )
            } else {
                (if self.opts.plain { "-" } else { "○" }, "", "0")
            };
            rows.push((glyph, p.name.as_str(), note, color));
        }

        let name_width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        let mut out = String::new();
        for (glyph, name, note, color) in rows {
            let lead = if color == "0" {
                format!("{glyph} {name}")
            } else {
                self.paint(color, &format!("{glyph} {name}"))
            };
            if note.is_empty() {
                out.push_str(&format!("{lead}\n"));
            } else {
                let pad = " ".repeat(name_width.saturating_sub(name.len()) + 2);
                out.push_str(&format!("{lead}{pad}{}\n", self.muted(note)));
            }
        }
        out
    }

    pub fn render_ip(&self, info: &IpInfo) -> String {
        let rows = [
            ("ip", info.ip.clone()),
            ("network", info.network.clone()),
            ("location", info.formatted_location()),
            ("coordinates", info.location.clone()),
            ("postal", info.postal.clone()),
            ("timezone", info.timezone.clone()),
        ];
        self.kv_block(&rows)
    }

    pub fn render_error(&self, err: &SbctlError) -> String {
        let mut out = format!("{}\n", self.fail(&err.message));
        for h in &err.hints {
            out.push_str(&format!("{}\n", self.hint(h)));
        }
        out
    }
}

fn visible_width(s: &str) -> usize {
    let mut len = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            len += 1;
        }
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_render_uses_ascii_only_symbols() {
        let theme = Theme::new(RenderOptions {
            plain: true,
            color: false,
        });
        let view = StatusView {
            state: RunState::Running,
            profile: Some("work".into()),
            running: true,
            tun: Some("tun0".into()),
            server: Some("1.2.3.4:443".into()),
            pid: Some(42),
            missing: vec![],
            broken: None,
        };
        let rendered = theme.render_status(&view);
        assert!(rendered.contains("state    running"));
        assert!(rendered.contains("profile  work"));
        assert!(!rendered.contains('●'));
        assert!(!rendered.contains('╭'));
    }
}
