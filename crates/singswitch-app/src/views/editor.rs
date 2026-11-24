use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    WeakEntity, Window, div, prelude::FluentBuilder as _, px, relative,
};
use singswitch_core::checker::{DEFAULT_LATENCY_PROBE_URL, ExecChecker};
use singswitch_core::form_model::{DEFAULT_SERVER_PORT, DEFAULT_TUN_MTU, ProfileFormModel};
use singswitch_core::readiness::{Field, tun_advisory};
use std::rc::Rc;
use std::time::Duration;

use crate::fonts;
use crate::state::{AppStore, BannerTone, EditorValidationState, NoticeOrigin};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum EditorTab {
    #[default]
    Guided,
    Json,
}

/// The last latency probe, as the editor shows it.
///
/// Remembers what was measured, not only the number: "312 ms" is a property of
/// one server on one config, and showing it after the address changed would
/// describe a document that no longer exists. The guided form is compared as a
/// model (the render already builds one for readiness, so the check is free);
/// the JSON tab is compared by document revision, because its buffer is what
/// changes a keystroke at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LatencyState {
    Measuring {
        profile: String,
    },
    Measured {
        profile: String,
        /// Boxed: a form model is eight heap fields, and `Measuring` would carry
        /// that size for a state that holds one string.
        model: Box<ProfileFormModel>,
        doc_revision: u64,
        result: Result<Duration, String>,
    },
}

impl LatencyState {
    fn profile(&self) -> &str {
        match self {
            Self::Measuring { profile } | Self::Measured { profile, .. } => profile,
        }
    }
}

/// One reading of a latency probe, ready to draw.
struct LatencyReadout {
    busy: bool,
    text: String,
    failed: bool,
}

impl LatencyReadout {
    /// The placeholder shown while the probe runs.
    fn measuring() -> Self {
        Self {
            busy: true,
            text: "Testing latency…".to_string(),
            failed: false,
        }
    }
}

pub struct ProfileEditorPanel {
    store: Entity<AppStore>,
    loaded_profile: Option<String>,
    seen_disk_revision: u64,
    disk_json: String,
    disk_model: Option<ProfileFormModel>,
    external_change_pending: bool,
    disk_read_failed: bool,
    validated_snapshot: Option<String>,
    active_tab: EditorTab,
    /// The exact text the last save submitted.
    ///
    /// The disk will become this text, not necessarily what the editor holds by
    /// the time the save lands: a keystroke during the round trip would
    /// otherwise look like an outside change and raise a false banner.
    last_submitted: Option<String>,
    /// The buffer text and its parse result, recomputed only when it changes.
    ///
    /// `render` used to materialize the whole editor Rope three times and parse
    /// it as JSON on every frame — twice a second while a caret blinked.
    derived_json: SharedString,
    derived_syntax: Option<String>,
    /// Fields still missing from `derived_json`; `None` when it does not parse.
    ///
    /// Readiness re-parsed the whole document on every frame for this answer.
    derived_missing: Option<Vec<Field>>,
    /// The TUN routing advisory for `derived_json`, cached with it.
    derived_advisory: Option<&'static str>,
    /// The probe's last answer, pinned to a generation so a reply for a profile
    /// the user has left cannot paint the one they are looking at.
    latency: Option<LatencyState>,
    latency_generation: u64,
    latency_task: Option<Task<()>>,
    /// Bumped whenever the JSON buffer's text changes, so a reading taken
    /// against the old text can be dropped without comparing whole documents.
    doc_revision: u64,

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
        // The doc revision is the timestamp the latency readout compares
        // against (`latency_readout`); it must move on every keystroke in the
        // JSON buffer, or a measurement taken against old text repaints as
        // current after the buffer stops parsing. Loading a profile bumps it
        // separately in `sync_with_store`.
        cx.subscribe_in(&json_editor, window, {
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.doc_revision = this.doc_revision.wrapping_add(1);
                    this.latency_generation = this.latency_generation.wrapping_add(1);
                    this.latency = None;
                    cx.notify();
                }
            }
        })
        .detach();
        let port_ph = DEFAULT_SERVER_PORT.to_string();
        let mtu_ph = DEFAULT_TUN_MTU.to_string();

        let disk_rev = store.read(cx).disk_revision;
        let mut panel = Self {
            store: store.clone(),
            loaded_profile: None,
            seen_disk_revision: disk_rev,
            disk_json: String::new(),
            disk_model: None,
            external_change_pending: false,
            disk_read_failed: false,
            validated_snapshot: None,
            last_submitted: None,
            derived_json: SharedString::default(),
            derived_syntax: None,
            derived_missing: None,
            derived_advisory: None,
            latency: None,
            latency_generation: 0,
            latency_task: None,
            doc_revision: 0,
            active_tab: if store.read(cx).editor_json_mode {
                EditorTab::Json
            } else {
                EditorTab::Guided
            },
            json_editor,
            model: ProfileFormModel::default(),
            in_outbound_type: cx.new(|cx| InputState::new(window, cx).placeholder("vless")),
            in_server: cx.new(|cx| InputState::new(window, cx).placeholder("vpn.example.com")),
            in_server_port: cx.new(|cx| InputState::new(window, cx).placeholder(port_ph)),
            in_uuid: cx.new(|cx| InputState::new(window, cx).placeholder("UUID")),
            in_password: cx.new(|cx| InputState::new(window, cx).placeholder("Password")),
            in_flow: cx.new(|cx| InputState::new(window, cx).placeholder("xtls-rprx-vision")),
            in_server_name: cx
                .new(|cx| InputState::new(window, cx).placeholder("TLS / Reality SNI")),
            in_utls_fp: cx.new(|cx| InputState::new(window, cx).placeholder("chrome")),
            in_reality_pubkey: cx
                .new(|cx| InputState::new(window, cx).placeholder("Reality public key")),
            in_reality_short_id: cx
                .new(|cx| InputState::new(window, cx).placeholder("Reality short ID")),
            in_tun_iface: cx.new(|cx| InputState::new(window, cx).placeholder("OS default")),
            in_tun_stack: cx.new(|cx| InputState::new(window, cx).placeholder("gvisor")),
            in_tun_mtu: cx.new(|cx| InputState::new(window, cx).placeholder(mtu_ph)),
            in_log_level: cx.new(|cx| InputState::new(window, cx).placeholder("info")),
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
            // Stash whatever the profile being left holds. Fold the guided form
            // into the buffer first: a form-only edit never touches the JSON
            // buffer, so keying the stash on the buffer alone lost it. If the
            // form cannot be converted (the file has nowhere to put a field),
            // the buffer is still stashed when dirty rather than dropped.
            if let Some(prev) = self.loaded_profile.clone() {
                if self.active_tab == EditorTab::Guided {
                    // Convert the form into the buffer so it is stashed with the
                    // draft. A failure here means the file has no section for a
                    // field; say so, or the edit disappears with no explanation.
                    if let Err(msg) = self.apply_guided_to_json(window, cx) {
                        self.store.update(cx, |s, cx| {
                            s.set_notice(
                                NoticeOrigin::Editor,
                                BannerTone::Warning,
                                format!("This config cannot hold the form values: {msg}"),
                                cx,
                            );
                        });
                    }
                }
                let current = self.json_editor.read(cx).value().to_string();
                let dirty = self.is_dirty_against(&current, cx);
                let failed = self.disk_read_failed;
                self.store.update(cx, |s, _| {
                    if dirty {
                        s.drafts.insert(prev, current);
                    } else if !failed {
                        s.drafts.remove(&prev);
                    }
                });
            }

            self.loaded_profile = target.clone();
            self.seen_disk_revision = disk_rev;
            self.external_change_pending = false;
            self.disk_read_failed = false;
            self.validated_snapshot = None;
            self.last_submitted = None;
            // A probe measures one profile's config. Carrying the number across
            // a switch would label the new server with the old server's time.
            self.latency_generation = self.latency_generation.wrapping_add(1);
            self.latency = None;
            self.doc_revision = self.doc_revision.wrapping_add(1);

            if let Some(name) = target {
                let content = self.store.read(cx).read_profile_content(&name);
                if let Some(content) = content {
                    self.load_json_into_views(&content, window, cx);
                    if let Some(draft) = self.store.read(cx).drafts.get(&name).cloned() {
                        self.set_editor_content(&draft, window, cx);
                    }
                } else {
                    self.load_json_into_views("", window, cx);
                    self.disk_read_failed = true;
                }
            } else {
                self.load_json_into_views("", window, cx);
            }
            return;
        }

        if disk_rev != self.seen_disk_revision {
            // The profile set may have changed: forget drafts for profiles that
            // no longer exist, since nothing can select them again.
            let stale: Vec<String> = {
                let s = self.store.read(cx);
                if s.drafts.is_empty() {
                    Vec::new()
                } else {
                    let live: std::collections::HashSet<&str> = s
                        .snapshot
                        .profiles
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect();
                    s.drafts
                        .keys()
                        .filter(|name| !live.contains(name.as_str()))
                        .cloned()
                        .collect()
                }
            };
            if !stale.is_empty() {
                self.store.update(cx, |s, _| {
                    for name in stale {
                        s.drafts.remove(&name);
                    }
                });
            }
            self.seen_disk_revision = disk_rev;
            if let Some(name) = &self.loaded_profile {
                let Some(latest) = self.store.read(cx).read_profile_content(name) else {
                    self.disk_read_failed = true;
                    return;
                };
                self.disk_read_failed = false;
                let editor_now = self.json_editor.read(cx).value().to_string();
                if latest == self.disk_json {
                    self.external_change_pending = false;
                } else if self.last_submitted.as_deref() == Some(latest.as_str()) {
                    // Our own save landed. Adopt it as the baseline; the buffer
                    // keeps any keystrokes typed while it was in flight, so they
                    // stay dirty instead of being called an outside change.
                    self.disk_json = latest.clone();
                    self.disk_model = ProfileFormModel::from_json_str(&latest).ok();
                    self.last_submitted = None;
                    self.external_change_pending = false;
                } else if latest == editor_now {
                    // The disk now holds exactly what the editor shows, which is
                    // what our own successful save produces. Adopt it as the new
                    // baseline instead of claiming a phantom outside change.
                    self.disk_json = latest.clone();
                    self.disk_model = ProfileFormModel::from_json_str(&latest).ok();
                    self.external_change_pending = false;
                } else if !self.is_dirty(cx) {
                    self.load_json_into_views(&latest, window, cx);
                    self.external_change_pending = false;
                } else {
                    // Keep the baseline the user is editing against. Adopting
                    // the outside version here silently redefined "Revert" as
                    // "discard my work and take theirs".
                    self.external_change_pending = true;
                }
            }
        }
    }

    pub fn force_reload_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = self.loaded_profile.clone() {
            let Some(content) = self.store.read(cx).read_profile_content(&name) else {
                self.disk_read_failed = true;
                return;
            };
            self.disk_read_failed = false;
            self.load_json_into_views(&content, window, cx);
            self.external_change_pending = false;
            self.validated_snapshot = None;
            self.store.update(cx, |s, cx| {
                // The user asked for the disk copy, so drop the stashed edits or
                // reopening the profile brings back the text they just discarded.
                s.drafts.remove(&name);
                s.clear_editor_validation(&name, cx);
            });
        }
    }

    fn load_json_into_views(&mut self, content: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.disk_json = content.to_owned();
        self.disk_model = self.set_editor_content(content, window, cx);
    }

    fn set_editor_content(
        &mut self,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ProfileFormModel> {
        self.json_editor
            .update(cx, |s, cx| s.set_value(content.to_owned(), window, cx));
        match ProfileFormModel::from_json_str(content) {
            Ok(parsed) => {
                self.model = parsed.clone();
                self.push_model_to_inputs(window, cx);
                Some(parsed)
            }
            Err(_) => {
                self.active_tab = EditorTab::Json;
                None
            }
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
            DEFAULT_SERVER_PORT
        } else {
            port_raw
                .parse()
                .map_err(|_| format!("invalid server port: {port_raw:?}"))?
        };
        let mtu_raw = self.in_tun_mtu.read(cx).value().trim().to_string();
        let mtu: u32 = if mtu_raw.is_empty() {
            DEFAULT_TUN_MTU
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

    /// Drop the stashed draft once the profile it belongs to is saved.
    fn clear_draft(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.store.update(cx, |s, _| {
            s.drafts.remove(&name);
        });
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.is_dirty_against(&self.json_editor.read(cx).value(), cx)
    }

    /// As [`is_dirty`], for callers that already hold the materialized text.
    fn is_dirty_against(&self, text: &str, cx: &App) -> bool {
        if text != self.disk_json {
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

    fn current_validation_key(&self, cx: &App) -> String {
        match self.active_tab {
            EditorTab::Json => self.json_editor.read(cx).value().to_string(),
            EditorTab::Guided => format!("{:?}", self.build_model_from_inputs(cx)),
        }
    }

    fn apply_guided_to_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let next = self.build_model_from_inputs(cx)?;
        self.model = next;
        let current = self.json_editor.read(cx).value().to_string();
        let mut root: serde_json::Value =
            serde_json::from_str(&current).map_err(|e| format!("invalid JSON: {e}"))?;
        let skipped = self.model.apply_to_value(&mut root);
        if !skipped.is_empty() {
            // Say so rather than accepting the edit and dropping it: Save would
            // otherwise report success over a field the file cannot hold.
            return Err(format!(
                "This config has no {} section to write to. Edit the JSON directly.",
                skipped.join(" or ")
            ));
        }
        let updated = serde_json::to_string_pretty(&root)
            .map(|s| format!("{s}\n"))
            .map_err(|e| format!("could not serialize JSON: {e}"))?;
        self.json_editor
            .update(cx, |s, cx| s.set_value(updated.clone(), window, cx));
        Ok(updated)
    }

    fn collect_effective_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let content = match self.active_tab {
            EditorTab::Guided => self.apply_guided_to_json(window, cx)?,
            EditorTab::Json => self.json_editor.read(cx).value().to_string(),
        };
        if let Err(err) = serde_json::from_str::<serde_json::Value>(&content) {
            return Err(format!("JSON syntax error: {err}"));
        }
        Ok(content)
    }

    /// Stash the buffer being edited, so leaving the editor or closing the
    /// window cannot discard it. Stashing used to happen only when switching
    /// to a different profile, which left the profile you were actually editing
    /// unprotected.
    pub fn stash_draft(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.loaded_profile.clone() else {
            return;
        };
        if self.disk_read_failed {
            return;
        }
        // A form-only edit never touches the JSON buffer, so fold it in first or
        // the stash holds unchanged text and the edit is lost on unmount.
        let content = self.collect_stash_content(cx);
        let dirty = self.is_dirty_against(&content, cx);
        if dirty {
            self.store.update(cx, |s, _| {
                s.drafts.insert(name, content);
            });
        }
    }

    /// The buffer plus any guided-form values, without needing a `Window`.
    ///
    /// `apply_guided_to_json` writes through the editor's `InputState`s, which
    /// take a window; the stash also runs from window-close paths, so this folds
    /// the form into a copy of the text instead.
    fn collect_stash_content(&mut self, cx: &App) -> String {
        let current = self.json_editor.read(cx).value().to_string();
        if self.active_tab != EditorTab::Guided {
            return current;
        }
        let Ok(next) = self.build_model_from_inputs(cx) else {
            return current;
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&current) else {
            return current;
        };
        if !next.apply_to_value(&mut root).is_empty() {
            return current;
        }
        serde_json::to_string_pretty(&root)
            .map(|s| format!("{s}\n"))
            .unwrap_or(current)
    }

    pub fn trigger_save(
        &mut self,
        and_activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Re-sync first. `loaded_profile` is otherwise only refreshed inside
        // render, so a keyboard save straight after changing the selection wrote
        // the previous profile's buffer over the newly selected file.
        self.sync_with_store(window, cx);
        if self.disk_read_failed {
            return;
        }
        let Some(name) = self.loaded_profile.clone() else {
            return;
        };
        let content = match self.collect_effective_content(window, cx) {
            Ok(c) => c,
            Err(err) => {
                self.validated_snapshot = Some(self.current_validation_key(cx));
                self.store.update(cx, |s, cx| {
                    s.editor_validation = Some((name, EditorValidationState::Invalid(err.clone())));
                    s.set_notice(NoticeOrigin::Editor, BannerTone::Error, err, cx);
                });
                return;
            }
        };

        self.validated_snapshot = Some(self.current_validation_key(cx));
        if and_activate {
            self.store
                .update(cx, |s, cx| s.save_and_activate(name, content, cx));
            return;
        }

        let is_active = self.store.read(cx).snapshot.active.as_deref() == Some(name.as_str());
        // A profile with fields still to fill saves as a draft, so an inactive
        // one can be kept while the user finds the values.
        let incomplete = singswitch_core::form_model::ProfileFormModel::from_json_str(&content)
            .map(|m| !m.missing().is_empty())
            .unwrap_or(true);
        let allow_invalid = incomplete && !is_active;

        self.clear_draft(&name, cx);
        // Record what we submitted so the resulting disk change is recognised
        // as ours rather than an outside edit.
        self.last_submitted = Some(content.clone());
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
                self.validated_snapshot = Some(self.current_validation_key(cx));
                self.store
                    .update(cx, |s, cx| s.validate_live_content(name, content, cx));
            }
            Err(err) => {
                self.validated_snapshot = Some(self.current_validation_key(cx));
                self.store.update(cx, |s, cx| {
                    s.editor_validation = Some((name, EditorValidationState::Invalid(err.clone())));
                    s.set_notice(NoticeOrigin::Editor, BannerTone::Error, err, cx);
                });
            }
        }
    }

    /// Whether a latency test can run for this document.
    ///
    /// The probe needs an outbound with a server and a tag to name it by:
    /// without those, `tools fetch` has nothing to measure, and the failure
    /// would read as a network problem rather than as an unfinished profile.
    fn latency_testable(model: &ProfileFormModel) -> bool {
        model.has_managed_outbound
            && !model.server.trim().is_empty()
            && !model.outbound_tag.trim().is_empty()
    }

    /// The model the document on screen describes, for the probe.
    ///
    /// The guided tab is read from the inputs rather than from `self.model`,
    /// which only catches up when the form is folded into the buffer or the tabs
    /// are switched. A form that does not parse cannot be tested.
    fn probe_model(&self, cx: &App) -> Option<ProfileFormModel> {
        match self.active_tab {
            EditorTab::Guided => self.build_model_from_inputs(cx).ok(),
            EditorTab::Json => Some(self.model.clone()),
        }
    }

    /// Measure one round trip through this profile's outbound, without
    /// activating it.
    ///
    /// Runs on the background executor like the live validation does: the probe
    /// spawns a process and waits on the network, neither of which may happen on
    /// the UI thread.
    fn test_latency(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.loaded_profile.clone() else {
            return;
        };
        let Some(model) = self.probe_model(cx).filter(Self::latency_testable) else {
            return;
        };
        let doc_revision = self.doc_revision;
        let content = match self.collect_content_for_probe(cx) {
            Ok(content) => content,
            Err(err) => {
                self.latency = Some(LatencyState::Measured {
                    profile: name,
                    model: Box::new(model),
                    doc_revision,
                    result: Err(err),
                });
                cx.notify();
                return;
            }
        };
        let model = Box::new(model);
        let tag = model.outbound_tag.clone();

        self.latency_generation = self.latency_generation.wrapping_add(1);
        let generation = self.latency_generation;
        self.latency = Some(LatencyState::Measuring {
            profile: name.clone(),
        });
        cx.notify();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (name, result) = cx
                .background_executor()
                .spawn(async move {
                    // Resolved inside the task: resolving stats the filesystem,
                    // and a missing binary belongs in the readout like any other
                    // probe failure instead of being swallowed here.
                    let result = ExecChecker::default()
                        .probe_latency_bytes(content.as_bytes(), &tag, DEFAULT_LATENCY_PROBE_URL)
                        .map_err(|err| match err.hints.is_empty() {
                            true => err.message.clone(),
                            false => format!("{} — {}", err.message, err.hints.join("; ")),
                        });
                    (name, result)
                })
                .await;

            let _ = this.update(cx, |panel, cx| {
                // A reply for a probe the user has since replaced, or for a
                // profile they have left, must not paint this row.
                if panel.latency_generation != generation
                    || panel.loaded_profile.as_deref() != Some(name.as_str())
                {
                    return;
                }
                panel.latency = Some(LatencyState::Measured {
                    profile: name,
                    model,
                    doc_revision,
                    result,
                });
                cx.notify();
            });
        });
        self.latency_task = Some(task);
    }

    /// The document the probe would run on, without writing anything.
    ///
    /// Deliberately not `collect_effective_content`: that one pushes the guided
    /// form into the editor buffer to return the merged text, and taking a
    /// measurement must not edit what the user is typing.
    fn collect_content_for_probe(&mut self, cx: &App) -> Result<String, String> {
        let current = self.json_editor.read(cx).value().to_string();
        if self.active_tab != EditorTab::Guided {
            return Ok(current);
        }
        let next = self.build_model_from_inputs(cx)?;
        let mut root: serde_json::Value =
            serde_json::from_str(&current).map_err(|e| format!("invalid JSON: {e}"))?;
        if !next.apply_to_value(&mut root).is_empty() {
            return Err("this config has no outbound section to test".to_string());
        }
        let merged = serde_json::to_string_pretty(&root)
            .map(|s| format!("{s}\n"))
            .map_err(|e| format!("could not serialize JSON: {e}"))?;
        // The probe names its outbound from the merged form, so the model it
        // reads that tag from has to be the one that was merged.
        self.model = next;
        Ok(merged)
    }

    /// The reading to draw for this profile, if it still describes the document
    /// on screen.
    ///
    /// `form` is the guided model the caller has already built for readiness, or
    /// `None` on the JSON tab, where the buffer itself is what was measured.
    fn latency_readout(
        &self,
        profile: &str,
        form: Option<&ProfileFormModel>,
    ) -> Option<LatencyReadout> {
        let state = self.latency.as_ref()?;
        if state.profile() != profile {
            return None;
        }
        let LatencyState::Measured {
            model,
            doc_revision,
            result,
            ..
        } = state
        else {
            return Some(LatencyReadout::measuring());
        };
        let still_current = match form {
            Some(live) => live == model.as_ref(),
            None => *doc_revision == self.doc_revision,
        };
        if !still_current {
            return None;
        }
        Some(match result {
            Ok(elapsed) => LatencyReadout {
                busy: false,
                text: format_latency(*elapsed),
                failed: false,
            },
            Err(msg) => LatencyReadout {
                busy: false,
                text: msg.clone(),
                failed: true,
            },
        })
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
                    s.set_notice(
                        NoticeOrigin::Editor,
                        BannerTone::Error,
                        format!("Invalid JSON: {err}"),
                        cx,
                    );
                });
            }
        }
    }

    fn render_disk_change_banner(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .px(px(12.0))
            .py(px(6.0))
            .justify_between()
            .items_center()
            .gap(px(10.0))
            .rounded(px(8.0))
            .bg(cx.theme().warning.opacity(0.08))
            .border_1()
            .border_color(cx.theme().warning.opacity(0.28))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .small()
                            .text_color(cx.theme().warning),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(fonts::EMPHASIS)
                            .text_color(cx.theme().foreground)
                            .child("Modified on disk outside SingSwitch."),
                    ),
            )
            .child(
                Button::new("editor-reload-disk")
                    .secondary()
                    .xsmall()
                    .label("Reload")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.force_reload_selected(window, cx);
                        cx.notify();
                    })),
            )
    }

    fn render_validation_banner(
        &self,
        profile_name: &str,
        validation: &EditorValidationState,
        is_active: bool,
        is_busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let (border, text_col, icon, label) = match validation {
            EditorValidationState::Idle => (
                cx.theme().border,
                cx.theme().foreground,
                IconName::Info,
                String::new(),
            ),
            EditorValidationState::Validating => (
                cx.theme().border,
                cx.theme().muted_foreground,
                IconName::Info,
                "Checking config...".to_string(),
            ),
            EditorValidationState::Valid => (
                crate::theme::success_text(cx),
                crate::theme::success_text(cx),
                IconName::CircleCheck,
                "Config valid.".to_string(),
            ),
            EditorValidationState::Invalid(msg) => (
                cx.theme().danger,
                cx.theme().danger,
                IconName::CircleX,
                msg.clone(),
            ),
        };

        let can_force_draft =
            matches!(validation, EditorValidationState::Invalid(_)) && !is_active && !is_busy;
        let profile_for_draft = profile_name.to_string();

        h_flex()
            .w_full()
            .px(px(12.0))
            .py(px(6.0))
            .justify_between()
            .items_center()
            .gap(px(10.0))
            .rounded(px(8.0))
            .bg(border.opacity(0.08))
            .border_1()
            .border_color(border.opacity(0.28))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(8.0))
                    .child(Icon::new(icon).small().text_color(text_col).flex_none())
                    .child(
                        div()
                            .id("editor-validation-message")
                            .flex_1()
                            .min_w(px(0.0))
                            .max_h(px(60.0))
                            .overflow_y_scroll()
                            .text_xs()
                            .font_weight(fonts::EMPHASIS)
                            .text_color(cx.theme().foreground)
                            .child(label),
                    ),
            )
            .when(can_force_draft, |row| {
                row.child(
                    Button::new("editor-save-draft-anyway")
                        .outline()
                        .xsmall()
                        .label("Save Draft...")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let name = profile_for_draft.clone();
                            let Ok(content) = this.collect_effective_content(window, cx) else {
                                return;
                            };
                            let store = this.store.clone();
                            open_save_draft_dialog(name, content, store, window, cx);
                        })),
                )
            })
    }

    fn render_toolbar(&self, name: String, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .gap(px(12.0))
            .flex_none()
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        Button::new("editor-back")
                            .ghost()
                            .h(px(28.0))
                            .icon(IconName::ChevronLeft)
                            .label("Profiles")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stash_draft(cx);
                                this.store.update(cx, |s, cx| s.end_edit(cx));
                            })),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(fonts::EMPHASIS)
                            .truncate()
                            .child(name),
                    ),
            )
            .child(
                h_flex().gap(px(8.0)).items_center().child(
                    TabBar::new("editor-mode-tabs")
                        .segmented()
                        .selected_index(match self.active_tab {
                            EditorTab::Guided => 0,
                            EditorTab::Json => 1,
                        })
                        .child(Tab::new().label("Form"))
                        .child(Tab::new().label("JSON"))
                        .on_click(cx.listener(|this, idx, window, cx| {
                            let next = if *idx == 0 {
                                EditorTab::Guided
                            } else {
                                EditorTab::Json
                            };
                            if next == this.active_tab {
                                return;
                            }
                            let result = match next {
                                EditorTab::Guided => {
                                    let raw = this.json_editor.read(cx).value();
                                    ProfileFormModel::from_json_str(&raw)
                                        .map(|model| {
                                            this.model = model;
                                            this.push_model_to_inputs(window, cx);
                                        })
                                        .map_err(|err| {
                                            format!(
                                                "Fix the JSON before switching to the form. {err}"
                                            )
                                        })
                                }
                                EditorTab::Json => {
                                    this.apply_guided_to_json(window, cx).map(|_| ())
                                }
                            };
                            match result {
                                Ok(()) => {
                                    this.active_tab = next;
                                    this.store.update(cx, |s, cx| {
                                        if s.notice_for(NoticeOrigin::Editor).is_some() {
                                            s.dismiss_notice(cx);
                                        }
                                    });
                                }
                                Err(err) => {
                                    this.store.update(cx, |s, cx| {
                                        s.set_notice(
                                            NoticeOrigin::Editor,
                                            BannerTone::Error,
                                            err,
                                            cx,
                                        );
                                    });
                                }
                            }
                            cx.notify();
                        })),
                ),
            )
    }

    fn render_save_bar(
        &self,
        dirty: bool,
        is_busy: bool,
        is_active_running: bool,
        can_activate: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let activation_label = if is_active_running {
            "Save and reconnect"
        } else if self.store.read(cx).snapshot.running {
            "Save and switch"
        } else {
            "Save and connect"
        };

        h_flex()
            .w_full()
            .pt(px(10.0))
            .flex_none()
            .gap(px(8.0))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if dirty {
                        "Unsaved changes"
                    } else {
                        "All changes saved"
                    }),
            )
            .child(self.render_overflow(dirty, is_busy, cx))
            .when(!is_active_running, |row| {
                row.child(
                    Button::new("editor-save")
                        .primary()
                        .h(px(32.0))
                        .label("Save")
                        .loading(is_busy)
                        .disabled(is_busy || !dirty)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.trigger_save(false, window, cx);
                        })),
                )
            })
            .when(is_active_running || can_activate, |row| {
                row.child(
                    Button::new("editor-save-live")
                        .when(is_active_running, Button::primary)
                        .when(!is_active_running, Button::secondary)
                        .h(px(32.0))
                        .label(activation_label)
                        .loading(is_busy)
                        .disabled(is_busy || !can_activate || (is_active_running && !dirty))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.trigger_save(true, window, cx);
                        })),
                )
            })
    }

    fn render_overflow(&self, dirty: bool, is_busy: bool, cx: &Context<Self>) -> impl IntoElement {
        let revert = Rc::new(cx.listener(|this, _, window, cx| {
            this.force_reload_selected(window, cx);
            cx.notify();
        }));
        let validate = Rc::new(cx.listener(|this, _, window, cx| this.validate_live(window, cx)));
        let format = Rc::new(cx.listener(|this, _, window, cx| this.format_json(window, cx)));
        let can_save_draft = self.loaded_profile != self.store.read(cx).snapshot.active;
        let draft = Rc::new(cx.listener(|this, _, window, cx| {
            if let Some(name) = this.loaded_profile.clone() {
                let content = match this.active_tab {
                    EditorTab::Json => this.json_editor.read(cx).value().to_string(),
                    EditorTab::Guided => match this.apply_guided_to_json(window, cx) {
                        Ok(content) => content,
                        Err(err) => {
                            this.store.update(cx, |s, cx| {
                                s.set_notice(NoticeOrigin::Editor, BannerTone::Error, err, cx);
                            });
                            return;
                        }
                    },
                };
                open_save_draft_dialog(name, content, this.store.clone(), window, cx);
            }
        }));

        Button::new("editor-overflow")
            .ghost()
            .h(px(32.0))
            .w(px(32.0))
            .icon(IconName::Ellipsis)
            .accessibility_label("Editor options")
            .dropdown_menu(move |menu, _, _| {
                let r = revert.clone();
                let v = validate.clone();
                let f = format.clone();
                let d = draft.clone();
                menu.item(
                    PopupMenuItem::new("Validate")
                        .disabled(is_busy)
                        .on_click(move |ev, w, cx| v(ev, w, cx)),
                )
                .item(
                    PopupMenuItem::new("Format JSON")
                        .disabled(is_busy)
                        .on_click(move |ev, w, cx| f(ev, w, cx)),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Revert changes")
                        .disabled(!dirty || is_busy)
                        .on_click(move |ev, w, cx| r(ev, w, cx)),
                )
                .item(
                    PopupMenuItem::new("Save as draft…")
                        .disabled(is_busy || !can_save_draft)
                        .on_click(move |ev, w, cx| d(ev, w, cx)),
                )
            })
    }

    fn render_guided_form(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        // Built once for the form and the latency row: both ask what the inputs
        // currently say, and the tunnel row is only ever drawn from this pass.
        // A parse failure is surfaced by `render`'s own readiness pass, not here.
        let form = self.build_model_from_inputs(cx);
        // Field rows whose section switch is off render inert and dimmed: the
        // writer honestly serialises only enabled sections, so a live-looking
        // input there would invite edits the saved file cannot carry.
        let tls_fields_live = self.model.tls_enabled;
        let reality_fields_live = self.model.reality_enabled;
        v_flex()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scrollbar()
            .pb(px(8.0))
            .gap_3()
            .child(
                form_section("Server", cx)
                    .child(form_row_2(
                        labeled_input("Server address", &self.in_server, cx),
                        labeled_input("Port", &self.in_server_port, cx),
                    ))
                    .child(form_row_2(
                        labeled_input("Protocol", &self.in_outbound_type, cx),
                        labeled_input("Flow", &self.in_flow, cx),
                    ))
                    .child(form_row_2(
                        labeled_input("UUID", &self.in_uuid, cx),
                        labeled_input("Password", &self.in_password, cx),
                    )),
            )
            .child(
                form_section("TLS and Reality", cx)
                    .child(self.render_tls_switches(cx))
                    .child(form_row_2(
                        labeled_input_opt(
                            "Server name",
                            &self.in_server_name,
                            !tls_fields_live,
                            cx,
                        ),
                        labeled_input_opt(
                            "uTLS fingerprint",
                            &self.in_utls_fp,
                            !tls_fields_live,
                            cx,
                        ),
                    ))
                    .child(form_row_2(
                        labeled_input_opt(
                            "Reality public key",
                            &self.in_reality_pubkey,
                            !reality_fields_live,
                            cx,
                        ),
                        labeled_input_opt(
                            "Reality short ID",
                            &self.in_reality_short_id,
                            !reality_fields_live,
                            cx,
                        ),
                    )),
            )
            .child(
                form_section("Tunnel and routing", cx)
                    .child(form_row_2(
                        labeled_input("Interface", &self.in_tun_iface, cx),
                        labeled_input("Stack", &self.in_tun_stack, cx),
                    ))
                    .child(form_row_2(
                        labeled_input("MTU", &self.in_tun_mtu, cx),
                        labeled_input("Log level", &self.in_log_level, cx),
                    ))
                    .child(self.render_tun_switches(cx))
                    .child(self.render_latency_row(form, cx)),
            )
            .into_any_element()
    }

    /// The latency test's control and its reading, beside the switches that
    /// change what the probe would measure.
    ///
    /// Lives in the tunnel section rather than the save bar: it reports on the
    /// outbound's server, and the save bar is about the file. `form` is the
    /// guide model `render_guided_form` has already built, so the reading can be
    /// checked against the live form without a second pass over the inputs.
    fn render_latency_row(
        &self,
        form: Result<ProfileFormModel, String>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let readout = self
            .loaded_profile
            .as_deref()
            .and_then(|name| self.latency_readout(name, form.as_ref().ok()));
        let busy = readout.as_ref().is_some_and(|r| r.busy);
        let can_test = form.as_ref().is_ok_and(Self::latency_testable);
        let message = readout.as_ref().map(|r| r.text.clone()).unwrap_or_default();
        let failed = readout.as_ref().is_some_and(|r| r.failed);
        let colour = match failed {
            true => cx.theme().danger,
            false => cx.theme().muted_foreground,
        };

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                Button::new("editor-test-latency")
                    .secondary()
                    .xsmall()
                    .icon(IconName::Gauge)
                    .label("Test latency")
                    .loading(busy)
                    .disabled(busy || !can_test)
                    .on_click(cx.listener(|this, _, _, cx| this.test_latency(cx))),
            )
            .child(
                div()
                    .id("editor-latency-readout")
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_xs()
                    .font_weight(fonts::EMPHASIS)
                    .when(message.is_empty(), |el| {
                        el.text_color(cx.theme().muted_foreground.opacity(0.7))
                            .child("Not tested")
                    })
                    .when(!message.is_empty(), |el| {
                        el.text_color(colour).child(message)
                    }),
            )
    }

    fn render_tls_switches(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_6()
            .flex_wrap()
            .child(
                Switch::new("ed-tls")
                    .checked(self.model.tls_enabled)
                    .label("TLS")
                    .on_change(cx.listener(|this, v, _, cx| {
                        this.model.tls_enabled = *v;
                        cx.notify();
                    })),
            )
            .child(
                Switch::new("ed-utls")
                    .checked(self.model.utls_enabled)
                    .label("uTLS")
                    .on_change(cx.listener(|this, v, _, cx| {
                        this.model.utls_enabled = *v;
                        cx.notify();
                    })),
            )
            .child(
                Switch::new("ed-reality")
                    .checked(self.model.reality_enabled)
                    .label("Reality")
                    .on_change(cx.listener(|this, v, _, cx| {
                        this.model.reality_enabled = *v;
                        cx.notify();
                    })),
            )
    }

    fn render_tun_switches(&self, cx: &Context<Self>) -> impl IntoElement {
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
                    .label("Block IPv6")
                    .on_change(cx.listener(|this, v, _, cx| {
                        this.model.reject_ipv6 = *v;
                        cx.notify();
                    })),
            )
    }

    fn render_json_pane(
        &self,
        syntax_status: Result<(), String>,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        v_flex()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .gap_2()
            .child(
                v_flex()
                    .flex_1()
                    .w_full()
                    .rounded(px(10.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(if syntax_status.is_err() {
                        cx.theme().danger
                    } else {
                        cx.theme().border
                    })
                    .bg(crate::theme::surface(cx))
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .child(Editor::new(&self.json_editor).h(relative(1.0))),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .px(px(14.0))
                            .py(px(6.0))
                            .border_t_1()
                            .border_color(cx.theme().border.opacity(0.6))
                            .bg(cx.theme().muted.opacity(0.4))
                            .justify_between()
                            .items_center()
                            .text_xs()
                            .child(match syntax_status {
                                Ok(()) => h_flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        Icon::new(IconName::CircleCheck)
                                            .xsmall()
                                            .text_color(cx.theme().success),
                                    )
                                    .child(
                                        div()
                                            .font_weight(fonts::EMPHASIS)
                                            .text_color(crate::theme::success_text(cx))
                                            .child("Valid JSON"),
                                    ),
                                Err(msg) => h_flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        Icon::new(IconName::CircleX)
                                            .xsmall()
                                            .text_color(cx.theme().danger),
                                    )
                                    .child(
                                        div()
                                            .font_weight(fonts::EMPHASIS)
                                            .text_color(cx.theme().danger)
                                            .truncate()
                                            .child(msg),
                                    ),
                            })
                            .child(div().text_color(cx.theme().muted_foreground).child(
                                if cfg!(target_os = "macos") {
                                    "⌘S to save"
                                } else {
                                    "Ctrl+S to save"
                                },
                            )),
                    ),
            )
            .into_any_element()
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
                        .font_weight(fonts::EMPHASIS)
                        .text_color(cx.theme().foreground)
                        .child("Select or create a profile."),
                )
                .into_any_element();
        };

        if self.disk_read_failed {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_base()
                        .font_weight(fonts::EMPHASIS)
                        .child(format!("Cannot read {profile_name}")),
                )
                .child(
                    div()
                        .text_sm()
                        .child("Reload the profile to continue editing."),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("unreadable-back")
                                .ghost()
                                .label("Back to connections")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.stash_draft(cx);
                                    this.store.update(cx, |s, cx| s.end_edit(cx));
                                })),
                        )
                        .child(
                            Button::new("unreadable-reload")
                                .primary()
                                .label("Reload profile")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.force_reload_selected(window, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element();
        }

        // Materialize once. Todos, syntax, and readiness are cached against the
        // text, so a repaint for an unrelated reason reuses them instead of
        // re-scanning. The buffer is compared in place and only copied out of
        // the editor when it actually changed.
        let buffer_changed = {
            let editor = self.json_editor.read(cx);
            editor.text() != self.derived_json.as_str()
        };
        if buffer_changed {
            let live_json = self.json_editor.read(cx).value();
            match serde_json::from_str::<serde_json::Value>(&live_json) {
                Ok(root) => {
                    self.derived_syntax = None;
                    let model = ProfileFormModel::from_value(&root);
                    self.derived_missing = Some(model.missing());
                    self.derived_advisory = tun_advisory(&model);
                }
                Err(err) => {
                    self.derived_syntax = Some(format!("JSON syntax error: {err}"));
                    self.derived_missing = None;
                    self.derived_advisory = None;
                }
            }
            self.derived_json = live_json;
        }
        // Readiness comes from whichever document the user is looking at: the
        // guided form on that tab, the JSON buffer otherwise. Deriving it from
        // the buffer alone meant typing the last value into the form left the
        // strip listing it and "Save & Connect" disabled until a save.
        //
        // The JSON side is answered from the parse cached above, so a repaint
        // no longer re-parses the document to fill in this strip.
        let guided_readiness = (self.active_tab == EditorTab::Guided).then(|| {
            self.build_model_from_inputs(cx)
                .map(|model| model.missing())
        });
        let (form_error, live_missing, can_activate): (Option<String>, &[Field], bool) =
            if let Some(readiness) = &guided_readiness {
                match readiness {
                    Ok(missing) => (None, missing, missing.is_empty()),
                    Err(err) => (Some(err.clone()), &[], false),
                }
            } else {
                // No cached parse means the buffer does not parse, so nothing
                // about it is ready.
                match &self.derived_missing {
                    Some(missing) => (None, missing, missing.is_empty()),
                    None => (None, &[], false),
                }
            };
        let syntax_status = match (&self.derived_syntax, self.active_tab) {
            (Some(err), EditorTab::Json) => Err(err.clone()),
            _ => Ok(()),
        };
        // The advisory follows the same rule as readiness: whichever document
        // the user is looking at. The guided form's switches are what the
        // warning is usually about, so reading the buffer alone would leave it
        // stale exactly while the user toggles auto_route.
        let advisory = match self.active_tab {
            EditorTab::Guided => match self.build_model_from_inputs(cx) {
                Ok(model) => tun_advisory(&model),
                Err(_) => self.derived_advisory,
            },
            EditorTab::Json => self.derived_advisory,
        };
        let dirty = self.is_dirty_against(&self.derived_json, cx);
        let validation = self.active_validation_state(&profile_name, cx);

        let (is_busy, is_active, is_active_running) = {
            let s = self.store.read(cx);
            let is_act = s.snapshot.active.as_deref() == Some(profile_name.as_str());
            (s.busy_phase.is_some(), is_act, is_act && s.snapshot.running)
        };

        v_flex()
            .size_full()
            .gap_3()
            .when(self.external_change_pending, |this| {
                this.child(self.render_disk_change_banner(cx))
            })
            .child(self.render_toolbar(profile_name.clone(), cx))
            .when_some(
                crate::views::notice_strip(&self.store, crate::state::NoticeOrigin::Editor, cx),
                |el, notice| el.child(notice),
            )
            .when_some(validation, |this, val| {
                this.child(self.render_validation_banner(
                    &profile_name,
                    &val,
                    is_active,
                    is_busy,
                    cx,
                ))
            })
            .when_some(form_error, |this, error| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .when(!live_missing.is_empty(), |this| {
                this.child(render_missing_strip(live_missing, cx))
            })
            // An advisory, not a blocker: the strip above lists what stops the
            // profile connecting, this one warns about traffic that would escape
            // a tunnel that connects fine.
            .when_some(advisory, |this, warning| {
                this.child(render_leak_advisory(warning, cx))
            })
            .child(match self.active_tab {
                EditorTab::Guided => self.render_guided_form(cx),
                EditorTab::Json => self.render_json_pane(syntax_status, cx),
            })
            .child(self.render_save_bar(dirty, is_busy, is_active_running, can_activate, cx))
            .into_any_element()
    }
}

impl ProfileEditorPanel {
    fn active_validation_state(
        &self,
        profile_name: &str,
        cx: &Context<Self>,
    ) -> Option<EditorValidationState> {
        let s = self.store.read(cx);
        let (target, state) = s.editor_validation.as_ref()?;
        if target != profile_name || *state == EditorValidationState::Idle {
            return None;
        }
        if *state != EditorValidationState::Validating && !self.validation_snapshot_matches(cx) {
            return None;
        }
        Some(state.clone())
    }

    /// Whether the document still equals the one the last validation ran on.
    ///
    /// The JSON tab compares against `derived_json`, which `render` has already
    /// refreshed for this frame, instead of copying the whole buffer out of the
    /// editor on every frame the banner is on screen.
    fn validation_snapshot_matches(&self, cx: &App) -> bool {
        let Some(snapshot) = self.validated_snapshot.as_ref() else {
            return false;
        };
        match self.active_tab {
            EditorTab::Json => snapshot.as_str() == self.derived_json.as_str(),
            EditorTab::Guided => *snapshot == self.current_validation_key(cx),
        }
    }
}

fn open_save_draft_dialog(
    name: String,
    content: String,
    store: Entity<AppStore>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let n = name.clone();
        let body = content.clone();
        let st = store.clone();
        alert
            .confirm()
            .title(format!("Save {n} as draft?"))
            .description(
                "This saves the current changes without requiring the configuration to pass validation.",
            )
            .ok_text("Save Draft")
            .on_ok(move |_, _, cx| {
                st.update(cx, |s, cx| {
                    s.save_profile_content(n.clone(), body.clone(), true, cx)
                });
                true
            })
    });
}

fn render_missing_strip(missing: &[Field], cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_wrap()
        .gap_1()
        .items_center()
        .child(
            div()
                .text_xs()
                .font_weight(fonts::EMPHASIS)
                .text_color(cx.theme().warning)
                .child("Still needed:"),
        )
        .children(
            missing
                .iter()
                .map(|f| Tag::warning().xsmall().child(f.label())),
        )
}

fn render_leak_advisory(warning: &'static str, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_wrap()
        .gap(px(8.0))
        .items_center()
        .child(
            Icon::new(IconName::TriangleAlert)
                .small()
                .flex_none()
                .text_color(cx.theme().warning),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_xs()
                .font_weight(fonts::EMPHASIS)
                .text_color(cx.theme().foreground)
                .child(warning),
        )
}

/// Render a probe duration the way a latency figure is read: whole
/// milliseconds, and seconds only for a delay nobody waits out comfortably.
pub fn format_latency(elapsed: Duration) -> String {
    if elapsed >= Duration::from_secs(1) {
        format!("{:.1} s", elapsed.as_secs_f64())
    } else {
        format!("{} ms", elapsed.as_millis())
    }
}

fn form_section(title: &'static str, cx: &App) -> gpui_kit::Div {
    // Same card family as the settings screen: radius 10, hairline border,
    // surface fill. The editor previously used a borderless rounded-12 card that
    // grouped on a 5% fill delta alone (1.07:1 in dark), invisible next to the
    // bordered cards two screens over.
    v_flex()
        .w_full()
        .flex_none()
        .p(px(14.0))
        .gap(px(10.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(cx.theme().border)
        .bg(crate::theme::surface(cx))
        // Section titles use the settings screens' eyebrow grammar: small
        // caps in muted ink, so the page's 14px tier stays reserved for the
        // profile name in the toolbar and field labels stay one rung under
        // their values.
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(fonts::EMPHASIS)
                .text_color(cx.theme().muted_foreground.opacity(0.85))
                .pb(px(2.0))
                .child(title.to_uppercase()),
        )
}

fn labeled_input(label: &'static str, state: &Entity<InputState>, cx: &App) -> gpui_kit::Div {
    labeled_input_opt(label, state, false, cx)
}

/// `unavailable` marks the row's inputs as inert (their section's switch is
/// off): visibly dimmed, so a user cannot type into a field the written config
/// will not carry. The value stays visible for reference; only the control is
/// dead.
fn labeled_input_opt(
    label: &'static str,
    state: &Entity<InputState>,
    unavailable: bool,
    cx: &App,
) -> gpui_kit::Div {
    let label_style = if unavailable {
        div()
            .text_xs()
            .font_normal()
            .text_color(cx.theme().muted_foreground.opacity(0.6))
    } else {
        // Labels used to carry full foreground ink, brighter than the values
        // they labeled; hierarchy runs the other way round.
        div()
            .text_xs()
            .font_normal()
            .text_color(cx.theme().muted_foreground)
    };
    v_flex()
        .flex_1()
        .min_w(px(180.0))
        .gap_1()
        .when(unavailable, |el| el.opacity(0.55))
        .child(label_style.child(label))
        .child(
            gpui_kit::component::input::Input::new(state)
                .h(px(32.0))
                .cleanable(true)
                .disabled(unavailable),
        )
}

fn form_row_2(left: impl IntoElement, right: impl IntoElement) -> impl IntoElement {
    h_flex().w_full().gap(px(12.0)).child(left).child(right)
}
