use gpui_kit::{Action, actions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NavTab {
    #[default]
    Profiles,
    Logs,
    Settings,
}

actions!(
    singswitch_app,
    [
        ToggleService,
        ConnectActive,
        DisconnectService,
        SaveCurrentProfile,
        SaveAndActivateProfile,
        SyncAll,
        RefreshIp,
        OpenMainWindow,
        OpenLogsView,
        OpenSettingsView,
        NewProfileDialog,
        ImportUrlDialog,
        CloseWindow,
        QuitApp,
        Noop,
        BackToConnections,
        EditSelectedProfile,
        DeleteSelectedProfile,
        RestartService,
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = singswitch_app, no_json)]
pub struct SwitchProfile {
    pub name: String,
}

/// Open a named profile's editor, used by the tray's "Set up …" row.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = singswitch_app, no_json)]
pub struct EditProfile {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = singswitch_app, no_json)]
pub struct TestLatency {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = singswitch_app, no_json)]
pub struct SelectTab {
    pub tab: NavTab,
}

/// Point a Clash-style `selector` group at another member.
///
/// Runtime-only: the switch goes to the profile's own experimental Clash API, so
/// nothing about the stored profile changes.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = singswitch_app, no_json)]
pub struct SwitchGroup {
    pub group: String,
    pub member: String,
}
