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

/// Measure one round trip through a named profile's outbound, from the tray.
///
/// A distinct action rather than a payload on `SwitchProfile`: testing latency
/// must not activate anything, and the tray needs to say so in its own row.
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
