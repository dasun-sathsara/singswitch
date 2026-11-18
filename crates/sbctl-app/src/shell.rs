use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{App, Menu, MenuItem, OsAction};
use sbctl_core::error::{Result, SbctlError};

use crate::actions::{
    CloseWindow, ConnectActive, DisconnectService, ImportUrlDialog, NavTab, NewProfileDialog,
    OpenMainWindow, OpenSettingsView, QuitApp, RefreshIp, RunDoctor, SaveAndActivateProfile,
    SaveCurrentProfile, SelectTab, SyncAll,
};

#[cfg(target_os = "macos")]
const LAUNCH_AGENT_LABEL: &str = "app.lexiflix.sbctl";

/// Switch macOS dock visibility when windows open or close while the tray stays alive.
pub fn set_dock_visible(visible: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            let policy = if visible {
                NSApplicationActivationPolicy::Regular
            } else {
                NSApplicationActivationPolicy::Accessory
            };
            app.setActivationPolicy(policy);
            if visible {
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = visible;
    }
}

/// Install standard application menus (most visible in the macOS top menu bar).
pub fn install_app_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "sbctl".into(),
            items: vec![
                MenuItem::action("Open Control Center", OpenMainWindow),
                MenuItem::action("Settings...", OpenSettingsView),
                MenuItem::separator(),
                MenuItem::action("Run Doctor", RunDoctor),
                MenuItem::separator(),
                MenuItem::action("Quit sbctl", QuitApp),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Profile...", NewProfileDialog),
                MenuItem::action("Import Profile from URL...", ImportUrlDialog),
                MenuItem::separator(),
                MenuItem::action("Save Profile", SaveCurrentProfile),
                MenuItem::action("Save & Activate", SaveAndActivateProfile),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
            disabled: false,
        },
        Menu {
            name: "Connection".into(),
            items: vec![
                MenuItem::action("Connect", ConnectActive),
                MenuItem::action("Disconnect", DisconnectService),
                MenuItem::separator(),
                MenuItem::action("Sync Remote Profiles", SyncAll),
                MenuItem::action("Refresh Public IP", RefreshIp),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action(
                    "Dashboard",
                    SelectTab {
                        tab: NavTab::Dashboard,
                    },
                ),
                MenuItem::action(
                    "Profiles",
                    SelectTab {
                        tab: NavTab::Profiles,
                    },
                ),
                MenuItem::action("Logs", SelectTab { tab: NavTab::Logs }),
                MenuItem::action(
                    "Doctor",
                    SelectTab {
                        tab: NavTab::Doctor,
                    },
                ),
                MenuItem::action(
                    "Settings",
                    SelectTab {
                        tab: NavTab::Settings,
                    },
                ),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
            disabled: false,
        },
    ]);
}

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library")
            .join("LaunchAgents")
            .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
    })
}

/// Inspect whether launch-at-login is currently registered for the active OS user.
pub fn is_launch_at_login_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        launch_agent_path().is_some_and(|p| p.exists())
    }
    #[cfg(target_os = "windows")]
    {
        windows_reg_run_exists()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Enable or disable per-user background launch at login (`--background`).
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = launch_agent_path().ok_or_else(|| {
            SbctlError::general("could not locate user home directory for LaunchAgents")
        })?;
        if !enabled {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| {
                    SbctlError::general(format!(
                        "failed to remove LaunchAgent {}: {e}",
                        path.display()
                    ))
                })?;
            }
            return Ok(());
        }

        let exe = std::env::current_exe().map_err(|e| {
            SbctlError::general(format!("could not locate current executable: {e}"))
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SbctlError::general(format!("failed to create LaunchAgents directory: {e}"))
            })?;
        }
        let escaped_exe = xml_escape(&exe.display().to_string());
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
                 <key>Label</key>\n\
                 <string>{LAUNCH_AGENT_LABEL}</string>\n\
                 <key>ProgramArguments</key>\n\
                 <array>\n\
                     <string>{escaped_exe}</string>\n\
                     <string>--background</string>\n\
                 </array>\n\
                 <key>RunAtLoad</key>\n\
                 <true/>\n\
                 <key>KeepAlive</key>\n\
                 <false/>\n\
             </dict>\n\
             </plist>\n"
        );
        std::fs::write(&path, plist).map_err(|e| {
            SbctlError::general(format!(
                "failed to write LaunchAgent {}: {e}",
                path.display()
            ))
        })?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        windows_set_reg_run(enabled)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = enabled;
        Err(SbctlError::general(
            "launch-at-login is supported on macOS and Windows",
        ))
    }
}

#[cfg(target_os = "macos")]
fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(target_os = "windows")]
fn to_wide_null(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_reg_run_exists() -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    let subkey = to_wide_null("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let value_name = to_wide_null("sbctl");
    let mut hkey: HKEY = std::ptr::null_mut();

    unsafe {
        let rc = RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey);
        if rc != 0 {
            return false;
        }
        let q = RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        RegCloseKey(hkey);
        q == 0
    }
}

#[cfg(target_os = "windows")]
fn windows_set_reg_run(enabled: bool) -> Result<()> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
    };

    let subkey = to_wide_null("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let value_name = to_wide_null("sbctl");
    let mut hkey: HKEY = std::ptr::null_mut();

    unsafe {
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(SbctlError::general(format!(
                "failed to open Windows Run registry key (code {rc})"
            )));
        }

        let op_rc = if enabled {
            let exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    RegCloseKey(hkey);
                    return Err(SbctlError::general(format!(
                        "could not locate current executable: {e}"
                    )));
                }
            };
            let cmd = format!("\"{}\" --background", exe.display());
            let wide = to_wide_null(&cmd);
            let byte_len = (wide.len() * std::mem::size_of::<u16>()) as u32;
            RegSetValueExW(
                hkey,
                value_name.as_ptr(),
                0,
                REG_SZ,
                wide.as_ptr().cast(),
                byte_len,
            )
        } else {
            let del = RegDeleteValueW(hkey, value_name.as_ptr());
            // ERROR_FILE_NOT_FOUND (2) is fine when disabling an absent entry.
            if del == 2 { 0 } else { del }
        };

        RegCloseKey(hkey);
        if op_rc != 0 {
            return Err(SbctlError::general(format!(
                "failed to update Windows Run registry entry (code {op_rc})"
            )));
        }
        Ok(())
    }
}
