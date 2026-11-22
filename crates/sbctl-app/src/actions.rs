use gpui_kit::{Action, actions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NavTab {
    #[default]
    Profiles,
    Logs,
    Doctor,
    Settings,
}

actions!(
    sbctl_app,
    [
        ToggleService,
        ConnectActive,
        DisconnectService,
        SaveCurrentProfile,
        SaveAndActivateProfile,
        SyncAll,
        RefreshIp,
        RunDoctor,
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
        CopyPublicIp,
        RestartService,
        RefreshStatus,
        SyncSelectedProfile,
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = sbctl_app, no_json)]
pub struct SwitchProfile {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = sbctl_app, no_json)]
pub struct SyncProfile {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = sbctl_app, no_json)]
pub struct SelectTab {
    pub tab: NavTab,
}
