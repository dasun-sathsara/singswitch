use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div, prelude::FluentBuilder as _, px, relative,
};
use sbctl_core::form_model::ProfileFormModel;
use sbctl_core::profile::placeholders_in;

use crate::state::{AppStore, BannerTone};
use crate::theme::surface_card_bg;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum EditorTab {
    #[default]
    Guided,
    Json,
}

pub struct ProfileEditorPanel {
    store: Entity<AppStore>,
    loaded_profile: Option<String>,
    seen_disk_revision: u64,
    disk_json: String,
    disk_model: Option<ProfileFormModel>,
    external_change_pending: bool,
    active_tab: EditorTab,

    json_editor: Entity<EditorState>,

    // Guided Setup input states mapped to ProfileFormModel fields.
    model: ProfileFormModel,
    in_outbound_type: Entity<InputState>,
    in_server: Entity<InputState>,
    in_server_port: Entity<InputState>,
    in_uuid: Entity<InputState>,
    in_password: Entity<InputState>,
    in_flow: Entity<InputState>,
    in_server_name: Entity<InputState>,
    in_utls_fp: Entity<InputState>,
    in_reality_pubkey: Entity<InputState>,
    in_reality_short_id: Entity<InputState>,
    in_tun_iface: Entity<InputState>,
    in_tun_stack: Entity<InputState>,
    in_tun_mtu: Entity<InputState>,
    in_log_level: Entity<InputState>,
}

impl ProfileEditorPanel {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let json_editor = cx.new(|cx| EditorState::new(window, cx).language("json"));

        let in_outbound_type = cx.new(|cx| InputState::new(window, cx).placeholder("vless"));
        let in_server = cx.new(|cx| InputState::new(window, cx).placeholder("vpn.example.com"));
        let in_server_port = cx.new(|cx| InputState::new(window, cx).placeholder("443"));
        let in_uuid = cx.new(|cx| InputState::new(window, cx).placeholder("UUID for vless/vmess"));
        let in_password =
            cx.new(|cx| InputState::new(window, cx).placeholder("Password (trojan/ss)"));
        let in_flow = cx.new(|cx| InputState::new(window, cx).placeholder("xtls-rprx-vision"));
        let in_server_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("TLS / Reality SNI"));
        let in_utls_fp = cx.new(|cx| InputState::new(window, cx).placeholder("chrome"));
        let in_reality_pubkey =
            cx.new(|cx| InputState::new(window, cx).placeholder("Reality public key"));
        let in_reality_short_id =
            cx.new(|cx| InputState::new(window, cx).placeholder("Reality short ID"));
        let in_tun_iface =
            cx.new(|cx| InputState::new(window, cx).placeholder("Auto (utun / singbox_tun)"));
        let in_tun_stack = cx.new(|cx| InputState::new(window, cx).placeholder("gvisor"));
        let in_tun_mtu = cx.new(|cx| InputState::new(window, cx).placeholder("1350"));
        let in_log_level = cx.new(|cx| InputState::new(window, cx).placeholder("info"));

        let disk_rev = store.read(cx).disk_revision;
        let mut panel = Self {
            store,
            loaded_profile: None,
            seen_disk_revision: disk_rev,
            disk_json: String::new(),
            disk_model: None,
            external_change_pending: false,
            active_tab: EditorTab::Guided,
            json_editor,
            model: ProfileFormModel::default(),
            in_outbound_type,
            in_server,
            in_server_port,
            in_uuid,
            in_password,
            in_flow,
            in_server_name,
            in_utls_fp,
            in_reality_pubkey,
            in_reality_short_id,
            in_tun_iface,
            in_tun_stack,
            in_tun_mtu,
            in_log_level,
        };
        panel.sync_with_store(window, cx);
        panel
    }

    pub fn sync_with_store(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (target, disk_rev) = {
            let s = self.store.read(cx);
            (s.selected_profile.clone(), s.disk_revision)
        };

        if target != self.loaded_profile {
            self.loaded_profile = target.clone();
            self.seen_disk_revision = disk_rev;
            self.external_change_pending = false;
            if let Some(name) = target {
                let content = self.store.read(cx).read_profile_content(&name);
                self.load_json_into_views(content, window, cx);
            } else {
                self.disk_json.clear();
                self.disk_model = None;
            }
            return;
        }

        if disk_rev != self.seen_disk_revision {
            self.seen_disk_revision = disk_rev;
            if let Some(name) = &self.loaded_profile {
                let latest = self.store.read(cx).read_profile_content(name);
                if latest == self.disk_json {
                    self.external_change_pending = false;
                } else if !self.is_dirty(cx) {
                    self.load_json_into_views(latest, window, cx);
                    self.external_change_pending = false;
                } else {
                    self.disk_json = latest.clone();
                    self.disk_model = ProfileFormModel::from_json_str(&latest).ok();
                    self.external_change_pending = true;
                }
            }
        }
    }

    pub fn force_reload_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = &self.loaded_profile {
            let content = self.store.read(cx).read_profile_content(name);
            self.load_json_into_views(content, window, cx);
            self.external_change_pending = false;
        }
    }

    fn load_json_into_views(
        &mut self,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.disk_json = content.clone();
        self.json_editor
            .update(cx, |s, cx| s.set_value(content.clone(), window, cx));
        if let Ok(parsed) = ProfileFormModel::from_json_str(&content) {
            self.disk_model = Some(parsed.clone());
            self.model = parsed;
            self.push_model_to_inputs(window, cx);
        } else {
            self.disk_model = None;
        }
    }

    fn push_model_to_inputs(&self, window: &mut Window, cx: &mut App) {
        let m = &self.model;
        self.in_outbound_type
            .update(cx, |s, cx| s.set_value(m.outbound_type.clone(), window, cx));
        self.in_server
            .update(cx, |s, cx| s.set_value(m.server.clone(), window, cx));
        self.in_server_port.update(cx, |s, cx| {
            s.set_value(m.server_port.to_string(), window, cx)
        });
        self.in_uuid
            .update(cx, |s, cx| s.set_value(m.uuid.clone(), window, cx));
        self.in_password
            .update(cx, |s, cx| s.set_value(m.password.clone(), window, cx));
        self.in_flow
            .update(cx, |s, cx| s.set_value(m.flow.clone(), window, cx));
        self.in_server_name
            .update(cx, |s, cx| s.set_value(m.server_name.clone(), window, cx));
        self.in_utls_fp.update(cx, |s, cx| {
            s.set_value(m.utls_fingerprint.clone(), window, cx)
        });
        self.in_reality_pubkey.update(cx, |s, cx| {
            s.set_value(m.reality_public_key.clone(), window, cx)
        });
        self.in_reality_short_id.update(cx, |s, cx| {
            s.set_value(m.reality_short_id.clone(), window, cx)
        });
        self.in_tun_iface.update(cx, |s, cx| {
            s.set_value(m.tun_interface_name.clone(), window, cx)
        });
        self.in_tun_stack
            .update(cx, |s, cx| s.set_value(m.tun_stack.clone(), window, cx));
        self.in_tun_mtu
            .update(cx, |s, cx| s.set_value(m.tun_mtu.to_string(), window, cx));
        self.in_log_level
            .update(cx, |s, cx| s.set_value(m.log_level.clone(), window, cx));
    }

    fn build_model_from_inputs(&self, cx: &App) -> Result<ProfileFormModel, String> {
        let port_raw = self.in_server_port.read(cx).value().trim().to_string();
        let port: u16 = if port_raw.is_empty() {
            443
        } else {
            port_raw
                .parse()
                .map_err(|_| format!("invalid server port: {port_raw:?}"))?
        };
        let mtu_raw = self.in_tun_mtu.read(cx).value().trim().to_string();
        let mtu: u32 = if mtu_raw.is_empty() {
            1350
        } else {
            mtu_raw
                .parse()
                .map_err(|_| format!("invalid TUN MTU: {mtu_raw:?}"))?
        };

        let mut next = self.model.clone();
        next.outbound_type = self.in_outbound_type.read(cx).value().trim().to_string();
        next.server = self.in_server.read(cx).value().trim().to_string();
        next.server_port = port;
        next.uuid = self.in_uuid.read(cx).value().trim().to_string();
        next.password = self.in_password.read(cx).value().trim().to_string();
        next.flow = self.in_flow.read(cx).value().trim().to_string();
        next.server_name = self.in_server_name.read(cx).value().trim().to_string();
        next.utls_fingerprint = self.in_utls_fp.read(cx).value().trim().to_string();
        next.reality_public_key = self.in_reality_pubkey.read(cx).value().trim().to_string();
        next.reality_short_id = self.in_reality_short_id.read(cx).value().trim().to_string();
        next.tun_interface_name = self.in_tun_iface.read(cx).value().trim().to_string();
        next.tun_stack = self.in_tun_stack.read(cx).value().trim().to_string();
        next.tun_mtu = mtu;
        next.log_level = self.in_log_level.read(cx).value().trim().to_string();
        Ok(next)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        if self.json_editor.read(cx).value().as_ref() != self.disk_json {
            return true;
        }
        if self.active_tab == EditorTab::Guided
            && let Some(disk_model) = &self.disk_model
        {
            return match self.build_model_from_inputs(cx) {
                Ok(cur) => &cur != disk_model,
                Err(_) => true,
            };
        }
        false
    }

    fn apply_guided_to_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let next = self.build_model_from_inputs(cx)?;
        self.model = next;
        let current = self.json_editor.read(cx).value().to_string();
        let updated = self
            .model
            .apply_to_json_str(&current)
            .map_err(|e| e.to_string())?;
        self.json_editor
            .update(cx, |s, cx| s.set_value(updated.clone(), window, cx));
        Ok(updated)
    }

    fn collect_effective_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        match self.active_tab {
            EditorTab::Guided => self.apply_guided_to_json(window, cx),
            EditorTab::Json => Ok(self.json_editor.read(cx).value().to_string()),
        }
    }

    pub fn trigger_save(
        &mut self,
        and_activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.loaded_profile.clone() else {
            return;
        };
        let content = match self.collect_effective_content(window, cx) {
            Ok(c) => c,
            Err(err) => {
                self.store
                    .update(cx, |s, cx| s.set_banner(BannerTone::Error, err, cx));
                return;
            }
        };

        if and_activate {
            self.store.update(cx, |s, cx| {
                s.save_and_activate(name, content, cx);
            });
            return;
        }

        let is_active = self.store.read(cx).snapshot.active.as_deref() == Some(name.as_str());
        let has_todos = !placeholders_in(&content).is_empty();
        // Draft profiles can save incomplete TODO placeholders as long as they are not active.
        let allow_invalid = has_todos && !is_active;

        self.store.update(cx, |s, cx| {
            s.save_profile_content(name, content, allow_invalid, cx);
        });
    }

    fn validate_live(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.loaded_profile.clone() else {
            return;
        };
        match self.collect_effective_content(window, cx) {
            Ok(content) => {
                self.store.update(cx, |s, cx| {
                    s.validate_live_content(name, content, cx);
                });
            }
            Err(err) => {
                self.store
                    .update(cx, |s, cx| s.set_banner(BannerTone::Error, err, cx));
            }
        }
    }

    fn format_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.json_editor.read(cx).value().to_string();
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(val) => {
                if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                    let with_nl = format!("{pretty}\n");
                    self.json_editor
                        .update(cx, |s, cx| s.set_value(with_nl, window, cx));
                }
            }
            Err(err) => {
                self.store.update(cx, |s, cx| {
                    s.set_banner(BannerTone::Error, format!("Invalid JSON: {err}"), cx);
                });
            }
        }
    }
}

impl Render for ProfileEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_with_store(window, cx);

        let Some(profile_name) = self.loaded_profile.clone() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child("Select or create a profile to inspect and edit"),
                )
                .into_any_element();
        };

        let live_json = self.json_editor.read(cx).value().to_string();
        let live_todos = placeholders_in(&live_json);
        let syntax_status = if self.active_tab == EditorTab::Json {
            match serde_json::from_str::<serde_json::Value>(&live_json) {
                Ok(_) => Ok(()),
                Err(err) => Err(format!("JSON syntax error: {err}")),
            }
        } else {
            Ok(())
        };
        let dirty = self.is_dirty(cx);

        let (is_busy, is_active_running) = {
            let s = self.store.read(cx);
            (
                s.busy_phase.is_some(),
                s.snapshot.running && s.snapshot.active.as_deref() == Some(profile_name.as_str()),
            )
        };

        v_flex()
            .size_full()
            .gap_3()
            .when(self.external_change_pending, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .px_3()
                        .py_2()
                        .justify_between()
                        .items_center()
                        .gap_3()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().warning)
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().warning)
                                .child("This profile changed on disk outside the editor."),
                        )
                        .child(
                            Button::new("editor-reload-disk")
                                .outline()
                                .xsmall()
                                .label("Reload from Disk")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.force_reload_selected(window, cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            // Sub-toolbar for editor mode & file actions.
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                TabBar::new("editor-mode-tabs")
                                    .segmented()
                                    .selected_index(match self.active_tab {
                                        EditorTab::Guided => 0,
                                        EditorTab::Json => 1,
                                    })
                                    .child(Tab::new().label("Guided Setup"))
                                    .child(Tab::new().label("JSON Editor"))
                                    .on_click(cx.listener(|this, idx, window, cx| {
                                        if *idx == 0 {
                                            let raw = this.json_editor.read(cx).value().to_string();
                                            if let Ok(parsed) =
                                                ProfileFormModel::from_json_str(&raw)
                                            {
                                                this.model = parsed;
                                                this.push_model_to_inputs(window, cx);
                                            }
                                            this.active_tab = EditorTab::Guided;
                                        } else {
                                            let _ = this.apply_guided_to_json(window, cx);
                                            this.active_tab = EditorTab::Json;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(dirty, |t| t.child(Tag::info().xsmall().child("Modified"))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(self.active_tab == EditorTab::Json, |this| {
                                this.child(
                                    Button::new("editor-format-json")
                                        .outline()
                                        .small()
                                        .label("Format")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.format_json(window, cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new("editor-revert")
                                    .outline()
                                    .small()
                                    .label("Revert")
                                    .disabled(!dirty || is_busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.force_reload_selected(window, cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("editor-validate")
                                    .outline()
                                    .small()
                                    .icon(IconName::Check)
                                    .label("Validate")
                                    .disabled(is_busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.validate_live(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("editor-save")
                                    .outline()
                                    .small()
                                    .label("Save")
                                    .disabled(is_busy || !dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.trigger_save(false, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("editor-save-activate")
                                    .primary()
                                    .small()
                                    .icon(IconName::Play)
                                    .label(if is_active_running {
                                        "Save & Reconnect"
                                    } else {
                                        "Save & Connect"
                                    })
                                    .disabled(is_busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.trigger_save(true, window, cx);
                                    })),
                            ),
                    ),
            )
            // Live TODO_ placeholder pills.
            .when(!live_todos.is_empty(), |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().warning)
                                .child("Remaining placeholders:"),
                        )
                        .children(
                            live_todos
                                .into_iter()
                                .map(|tok| Tag::warning().xsmall().child(tok)),
                        ),
                )
            })
            // Body: either Guided form or full JSON code editor.
            .child(match self.active_tab {
                EditorTab::Guided => v_flex()
                    .flex_1()
                    .w_full()
                    .overflow_y_scrollbar()
                    .gap_4()
                    .p_4()
                    .rounded(cx.theme().radius_lg)
                    .bg(surface_card_bg(cx))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(section_heading("Proxy Server & Credentials", cx))
                    .child(form_row_2(
                        labeled_input(
                            "Outbound Type (vless, trojan, vmess, shadowsocks)",
                            &self.in_outbound_type,
                            cx,
                        ),
                        labeled_input(
                            "Log Level (info, warn, error, debug)",
                            &self.in_log_level,
                            cx,
                        ),
                    ))
                    .child(form_row_2(
                        labeled_input("Server Address", &self.in_server, cx),
                        labeled_input("Server Port", &self.in_server_port, cx),
                    ))
                    .child(form_row_2(
                        labeled_input("UUID", &self.in_uuid, cx),
                        labeled_input("Password", &self.in_password, cx),
                    ))
                    .child(labeled_input(
                        "Flow (for example xtls-rprx-vision)",
                        &self.in_flow,
                        cx,
                    ))
                    .child(section_heading("TLS & Reality Transport", cx))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_6()
                            .flex_wrap()
                            .child(
                                Switch::new("ed-tls")
                                    .checked(self.model.tls_enabled)
                                    .label("TLS enabled")
                                    .on_change(cx.listener(|this, v, _, cx| {
                                        this.model.tls_enabled = *v;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Switch::new("ed-utls")
                                    .checked(self.model.utls_enabled)
                                    .label("uTLS enabled")
                                    .on_change(cx.listener(|this, v, _, cx| {
                                        this.model.utls_enabled = *v;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Switch::new("ed-reality")
                                    .checked(self.model.reality_enabled)
                                    .label("Reality enabled")
                                    .on_change(cx.listener(|this, v, _, cx| {
                                        this.model.reality_enabled = *v;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(form_row_2(
                        labeled_input("Server Name (SNI)", &self.in_server_name, cx),
                        labeled_input(
                            "uTLS Fingerprint (chrome, firefox, safari)",
                            &self.in_utls_fp,
                            cx,
                        ),
                    ))
                    .child(form_row_2(
                        labeled_input("Reality Public Key", &self.in_reality_pubkey, cx),
                        labeled_input("Reality Short ID", &self.in_reality_short_id, cx),
                    ))
                    .child(section_heading("TUN & Routing", cx))
                    .child(labeled_input(
                        "TUN Interface Name (leave empty for OS default)",
                        &self.in_tun_iface,
                        cx,
                    ))
                    .child(form_row_2(
                        labeled_input("TUN Stack (gvisor, system, mixed)", &self.in_tun_stack, cx),
                        labeled_input("TUN MTU", &self.in_tun_mtu, cx),
                    ))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_6()
                            .flex_wrap()
                            .child(
                                Switch::new("ed-auto-route")
                                    .checked(self.model.tun_auto_route)
                                    .label("Auto route")
                                    .on_change(cx.listener(|this, v, _, cx| {
                                        this.model.tun_auto_route = *v;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Switch::new("ed-strict-route")
                                    .checked(self.model.tun_strict_route)
                                    .label("Strict route")
                                    .on_change(cx.listener(|this, v, _, cx| {
                                        this.model.tun_strict_route = *v;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Switch::new("ed-reject-ipv6")
                                    .checked(self.model.reject_ipv6)
                                    .label("Reject IPv6 DNS/traffic")
                                    .on_change(cx.listener(|this, v, _, cx| {
                                        this.model.reject_ipv6 = *v;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element(),
                EditorTab::Json => v_flex()
                    .flex_1()
                    .w_full()
                    .min_h(px(300.0))
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .rounded(cx.theme().radius_lg)
                            .overflow_hidden()
                            .border_1()
                            .border_color(if syntax_status.is_err() {
                                cx.theme().danger
                            } else {
                                cx.theme().border
                            })
                            .child(Editor::new(&self.json_editor).h(relative(1.0))),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .text_xs()
                            .child(match syntax_status {
                                Ok(()) => div()
                                    .text_color(cx.theme().success)
                                    .child("Valid JSON syntax"),
                                Err(msg) => {
                                    div().text_color(cx.theme().danger).truncate().child(msg)
                                }
                            })
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Cmd/Ctrl+S to Save"),
                            ),
                    )
                    .into_any_element(),
            })
            .into_any_element()
    }
}

fn section_heading(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

fn labeled_input(label: &'static str, state: &Entity<InputState>, cx: &App) -> gpui_kit::Div {
    v_flex()
        .flex_1()
        .min_w(px(180.0))
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().foreground)
                .child(label),
        )
        .child(Input::new(state).cleanable(true).small())
}

fn form_row_2(left: impl IntoElement, right: impl IntoElement) -> impl IntoElement {
    h_flex().w_full().gap_4().child(left).child(right)
}
