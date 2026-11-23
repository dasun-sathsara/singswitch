use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Result, SingSwitchError};
use crate::profile::name_for;

/// State captured before activation so failed switches can restore the previous profile.
pub trait Rollback: Send {
    fn rollback(&self) -> Result<()>;
    fn description(&self) -> String;
    fn known(&self) -> bool;
}

/// Reports and swaps which profile the sing-box service loads.
///
/// Both implementations return `Ok(None)` when no profile is active so fresh installs
/// are handled uniformly without platform checks.
pub trait Activator: Send + Sync {
    fn active_name(&self) -> Result<Option<String>>;
    fn active_path(&self) -> Result<Option<PathBuf>>;
    fn activate(&self, target: &Path) -> Result<Box<dyn Rollback>>;
    /// Forget the active profile. Called when the active profile is deleted, so
    /// the marker never names a file that no longer exists.
    fn clear(&self) -> Result<()>;
}

/// Hook for performing the symlink swap when a caller must not do it directly.
///
/// Production uses none: activating is an unprivileged atomic rename inside the
/// config directory, which the installer hands to the console user.
pub type SymlinkHook = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// Map an activation failure to a message the user can act on.
///
/// A bare "permission denied" gives no clue that the config directory has to be
/// handed to the console user, which is what the installer does.
#[cfg(unix)]
fn activation_error(err: std::io::Error, path: &Path) -> SingSwitchError {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return SingSwitchError::permission(format!(
            "not allowed to update {}",
            path.display()
        ))
        .with_hint("re-run scripts/install-macos.sh so the installer can take ownership of the config directory");
    }
    SingSwitchError::service(format!("could not update {}: {err}", path.display()))
}

/// Symlink-based activator used on macOS where root-owned `/usr/local/etc/sing-box/config.json`
/// points into `/usr/local/etc/sing-box/profiles/`.
pub struct SymlinkActivator {
    pub active_config_path: PathBuf,
    pub link_hook: Option<SymlinkHook>,
}

impl SymlinkActivator {
    pub fn new(active_config_path: PathBuf, link_hook: Option<SymlinkHook>) -> Self {
        Self {
            active_config_path,
            link_hook,
        }
    }

    fn perform_link(&self, target: &Path) -> Result<()> {
        if let Some(hook) = &self.link_hook {
            return hook(target);
        }
        if let Some(parent) = self.active_config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            let staging = PathBuf::from(format!(
                "{}.singswitch-staging",
                self.active_config_path.display()
            ));
            let _ = fs::remove_file(&staging);
            std::os::unix::fs::symlink(target, &staging)?;
            if let Err(err) = fs::rename(&staging, &self.active_config_path) {
                let _ = fs::remove_file(&staging);
                return Err(activation_error(err, &self.active_config_path));
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let _ = target;
            Err(SingSwitchError::general(
                "SymlinkActivator without link_hook is Unix-only",
            ))
        }
    }
}

impl Activator for SymlinkActivator {
    fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.active_config_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    fn active_path(&self) -> Result<Option<PathBuf>> {
        let target = match fs::read_link(&self.active_config_path) {
            Ok(target) => target,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                ) || is_not_symlink_err(&err) =>
            {
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        };

        if target.is_absolute() {
            Ok(Some(target))
        } else {
            let parent = self
                .active_config_path
                .parent()
                .unwrap_or_else(|| Path::new(""));
            Ok(Some(clean_join(parent, &target)))
        }
    }

    fn active_name(&self) -> Result<Option<String>> {
        let Some(path) = self.active_path()? else {
            return Ok(None);
        };
        let name = name_for(&path);
        if name.is_empty() {
            Ok(None)
        } else {
            Ok(Some(name))
        }
    }

    fn activate(&self, target: &Path) -> Result<Box<dyn Rollback>> {
        let previous = self.active_path()?;
        self.perform_link(target)?;
        Ok(Box::new(SymlinkRollback {
            active_config_path: self.active_config_path.clone(),
            link_hook: self.link_hook.clone(),
            previous,
        }))
    }
}

struct SymlinkRollback {
    active_config_path: PathBuf,
    link_hook: Option<SymlinkHook>,
    previous: Option<PathBuf>,
}

impl Rollback for SymlinkRollback {
    fn known(&self) -> bool {
        self.previous.is_some()
    }

    fn description(&self) -> String {
        self.previous
            .as_deref()
            .map(name_for)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "the previous configuration".to_string())
    }

    fn rollback(&self) -> Result<()> {
        let Some(prev) = &self.previous else {
            return Err(SingSwitchError::service(
                "there was no previous active profile to restore",
            ));
        };
        SymlinkActivator::new(self.active_config_path.clone(), self.link_hook.clone())
            .perform_link(prev)
    }
}

/// Copy-based activator used on Windows where `LocalSystem` reads `%ProgramData%\sing-box\config.json`
/// and `%ProgramData%\singswitch\active-profile` records the profile name.
#[derive(Debug, Clone)]
pub struct CopyActivator {
    pub active_config_path: PathBuf,
    pub active_name_path: PathBuf,
}

impl CopyActivator {
    pub fn new(active_config_path: PathBuf, active_name_path: PathBuf) -> Self {
        Self {
            active_config_path,
            active_name_path,
        }
    }

    /// Clear the active profile marker file.
    pub fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.active_name_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl Activator for CopyActivator {
    fn clear(&self) -> Result<()> {
        CopyActivator::clear(self)
    }

    fn active_name(&self) -> Result<Option<String>> {
        match fs::read_to_string(&self.active_name_path) {
            Ok(text) => {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(trimmed))
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn active_path(&self) -> Result<Option<PathBuf>> {
        if self.active_name()?.is_none() {
            Ok(None)
        } else {
            Ok(Some(self.active_config_path.clone()))
        }
    }

    fn activate(&self, target: &Path) -> Result<Box<dyn Rollback>> {
        let old_config = match fs::read(&self.active_config_path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        let old_name = match fs::read(&self.active_name_path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        let new_bytes = fs::read(target)?;
        write_file_atomic(&self.active_config_path, &new_bytes, 0o600)?;

        let name = name_for(target);
        if let Err(err) = write_file_atomic(
            &self.active_name_path,
            format!("{name}\n").as_bytes(),
            0o644,
        ) {
            if let Some(prev_cfg) = &old_config {
                let _ = write_file_atomic(&self.active_config_path, prev_cfg, 0o600);
            } else {
                let _ = fs::remove_file(&self.active_config_path);
            }
            return Err(err);
        }

        Ok(Box::new(CopyRollback {
            activator: self.clone(),
            config: old_config,
            name: old_name,
        }))
    }
}

struct CopyRollback {
    activator: CopyActivator,
    config: Option<Vec<u8>>,
    name: Option<Vec<u8>>,
}

impl Rollback for CopyRollback {
    fn known(&self) -> bool {
        self.config.is_some()
    }

    fn description(&self) -> String {
        if let Some(bytes) = &self.name {
            let s = String::from_utf8_lossy(bytes).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
        "the previous configuration".to_string()
    }

    fn rollback(&self) -> Result<()> {
        let Some(prev_cfg) = &self.config else {
            return Err(SingSwitchError::service(
                "there was no previous active config to restore",
            ));
        };
        write_file_atomic(&self.activator.active_config_path, prev_cfg, 0o600)?;
        match &self.name {
            Some(prev_name) if !prev_name.is_empty() => {
                write_file_atomic(&self.activator.active_name_path, prev_name, 0o644)
            }
            _ => self.activator.clear(),
        }
    }
}

/// Write `data` via a sibling temp file, fsync, and atomic rename (with Windows fallback).
pub fn write_file_atomic(path: &Path, data: &[u8], unix_mode: u32) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;

    let mut tmp = tempfile::Builder::new()
        .prefix(".singswitch-")
        .tempfile_in(dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(unix_mode))?;
    }
    #[cfg(not(unix))]
    let _ = unix_mode;

    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;

    let (_, tmp_path) = tmp.keep().map_err(|e| {
        SingSwitchError::general(format!("could not persist temp file: {}", e.error))
    })?;

    if fs::rename(&tmp_path, path).is_ok() {
        return Ok(());
    }

    // The rename failed because something holds the destination (an AV scan or
    // a sharing violation). Read the old bytes before touching anything, so a
    // failed retry can put the live file back. Deleting it, as this used to,
    // left the caller with no config at all.
    let previous = fs::read(path).ok();
    match fs::copy(&tmp_path, path) {
        Ok(_) => {
            let _ = fs::remove_file(&tmp_path);
            Ok(())
        }
        Err(err) => {
            if let Some(bytes) = previous
                && let Err(restore) = fs::write(path, bytes)
            {
                return Err(SingSwitchError::general(format!(
                    "could not move new file into place ({err}) and could not restore {} ({restore})",
                    path.display()
                )));
            }
            let _ = fs::remove_file(&tmp_path);
            Err(err.into())
        }
    }
}

/// Create a new file strictly with `create_new(true)` (`O_EXCL`) so concurrent creates never clobber.
pub fn write_new_file_exclusive(path: &Path, data: &[u8], unix_mode: u32) -> std::io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(unix_mode);
    }
    #[cfg(not(unix))]
    let _ = unix_mode;

    let mut file = opts.open(path)?;
    if let Err(err) = file.write_all(data).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(err);
    }
    Ok(())
}

fn is_not_symlink_err(err: &std::io::Error) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("invalid argument")
        || msg.contains("not a reparse point")
        || msg.contains("incorrect function")
        || msg.contains("not a symbolic link")
}

fn clean_join(base: &Path, rel: &Path) -> PathBuf {
    let joined = base.join(rel);
    let mut stack = Vec::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !stack.is_empty() {
                    stack.pop();
                }
            }
            other => stack.push(other),
        }
    }
    stack.iter().collect()
}
