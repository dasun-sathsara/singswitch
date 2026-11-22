use std::env;
use std::path::{Path, PathBuf};

use crate::error::Result;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use crate::error::SbctlError;

/// Target operating system modeled by a `Layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Darwin,
    Windows,
}

impl PlatformKind {
    /// Human-readable OS identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Darwin => "darwin",
            Self::Windows => "windows",
        }
    }
}

/// Host-specific paths, service identifiers, and privileged binary locations.
///
/// Constructing a `Layout` never touches the filesystem, so tests can point it at
/// temporary directories on any host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub platform: PlatformKind,
    /// Windows service name (`"sing-box"`). Empty on Darwin, which uses `launchd_label`.
    pub service_name: String,
    /// Launchd service target (`"system/app.lexiflix.singbox"`). Darwin only.
    pub launchd_label: String,
    /// LaunchDaemon plist path. Darwin only.
    pub plist_path: PathBuf,
    /// Directory storing `*.json` sing-box profiles and `.url` remote sidecars.
    pub profiles_dir: PathBuf,
    /// File sing-box actually reads (symlink on Darwin, atomic copy on Windows).
    pub active_config_path: PathBuf,
    /// File storing the active profile name on copy-based platforms (Windows).
    pub active_name_path: Option<PathBuf>,
    /// Service error log file path.
    pub error_log_path: PathBuf,
    /// Service stdout/access log file path.
    pub access_log_path: PathBuf,
    /// Path to `ln` invoked via `sudo` on Darwin.
    /// Path to `launchctl` invoked via `sudo` on Darwin.
    pub ctl_bin: PathBuf,
    /// Named pipe path for the Windows unprivileged-GUI helper service.
    pub pipe_name: String,
}

impl Layout {
    /// Detect the layout for the current compile target.
    pub fn detect() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self::darwin())
        }
        #[cfg(target_os = "windows")]
        {
            Ok(Self::windows())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Err(SbctlError::general(format!(
                "sbctl-rs targets macOS and Windows (Linux support is planned as future work); current OS is {}",
                env::consts::OS
            )))
        }
    }

    /// Canonical macOS layout matching `/usr/local/etc/sing-box` and `app.lexiflix.singbox`.
    pub fn darwin() -> Self {
        Self {
            platform: PlatformKind::Darwin,
            service_name: String::new(),
            launchd_label: "system/app.lexiflix.singbox".to_string(),
            plist_path: PathBuf::from("/Library/LaunchDaemons/app.lexiflix.singbox.plist"),
            profiles_dir: PathBuf::from("/usr/local/etc/sing-box/profiles"),
            active_config_path: PathBuf::from("/usr/local/etc/sing-box/config.json"),
            active_name_path: None,
            error_log_path: PathBuf::from("/var/log/sing-box/error.log"),
            access_log_path: PathBuf::from("/var/log/sing-box/access.log"),
            // Fixed absolute paths only. These end up inside NOPASSWD sudoers
            // rules, so resolving them from PATH would let a user-writable
            // directory (Homebrew, /usr/local/bin) nominate the binary that
            // root then runs.
            ctl_bin: privileged_binary(&["/bin/launchctl", "/usr/bin/launchctl"], "/bin/launchctl"),
            pipe_name: String::new(),
        }
    }

    /// Canonical Windows layout honouring `%ProgramData%`.
    pub fn windows() -> Self {
        let program_data = env::var_os("ProgramData")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        Self::windows_with_program_data(&program_data)
    }

    /// Construct a Windows layout rooted at an explicit `ProgramData` path.
    pub fn windows_with_program_data(program_data: &Path) -> Self {
        let sing_box = program_data.join("sing-box");
        let sbctl = program_data.join("sbctl");
        Self {
            platform: PlatformKind::Windows,
            service_name: "sing-box".to_string(),
            launchd_label: String::new(),
            plist_path: PathBuf::new(),
            profiles_dir: sing_box.join("profiles"),
            active_config_path: sing_box.join("config.json"),
            active_name_path: Some(sbctl.join("active-profile")),
            error_log_path: sing_box.join("logs").join("sing-box.err.log"),
            access_log_path: sing_box.join("logs").join("sing-box.out.log"),
            ctl_bin: PathBuf::new(),
            pipe_name: r"\\.\pipe\sbctl-control".to_string(),
        }
    }

    /// Reports whether privileged commands go through `sudo(8)`.
    pub fn uses_sudo(&self) -> bool {
        self.platform == PlatformKind::Darwin
    }

    /// Returns the concrete `ln` argv used to point `active_config_path` at `profile_path`.
    /// Directory where sbctl stores non-root UI settings and lock files.
    pub fn lock_path(&self) -> PathBuf {
        self.profiles_dir.join(".sbctl.lock")
    }

    /// Privileged copy of sing-box installed by the app's bundling installers.
    ///
    /// The app never runs a sing-box owned by Homebrew, WinGet, or a shell
    /// profile: a root daemon has to exec a binary that only root can replace.
    pub fn managed_singbox_bin(&self) -> PathBuf {
        match self.platform {
            PlatformKind::Darwin => PathBuf::from("/usr/local/libexec/sbctl/sing-box"),
            PlatformKind::Windows => self
                .active_name_path
                .as_ref()
                .and_then(|path| path.parent())
                .map(|dir| dir.join("bin").join("sing-box.exe"))
                .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData\sbctl\bin\sing-box.exe")),
        }
    }
}

/// Resolve an executable on `PATH`, checking explicit fallbacks (crucial for macOS GUI bundles
/// launched from Finder without shell `PATH` initialization).
/// Resolve a binary that will be named in a privileged (sudoers) rule.
///
/// Never consults PATH: the caller's PATH is attacker-controlled for this
/// purpose, and the result is executed as root.
pub fn privileged_binary(candidates: &[&str], default: &str) -> PathBuf {
    for candidate in candidates {
        let path = Path::new(candidate);
        if is_executable_file(path) {
            return path.to_path_buf();
        }
    }
    PathBuf::from(default)
}

pub fn lookup_binary(name: &str, fallbacks: &[&str], default: &str) -> PathBuf {
    if let Some(found) = find_in_path(name) {
        return found;
    }
    for candidate in fallbacks {
        let path = Path::new(candidate);
        if is_executable_file(path) {
            return path.to_path_buf();
        }
    }
    PathBuf::from(default)
}

/// Locate the `sing-box` binary across explicit override, `PATH`, and platform install paths.
pub fn resolve_singbox_binary(override_bin: Option<&Path>) -> Option<PathBuf> {
    if let Some(bin) = override_bin {
        if bin.as_os_str().is_empty() {
            return None;
        }
        if bin.is_absolute() || bin.components().count() > 1 {
            return is_executable_file(bin).then(|| bin.to_path_buf());
        }
        return find_in_path(&bin.to_string_lossy());
    }

    if let Some(env_bin) = env::var_os("SBCTL_SINGBOX_BIN").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(env_bin);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    let exe_name = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };

    // The privileged copy the installer stages is what the daemon actually
    // execs, so validate with that one whenever it exists. The bundled copy is
    // the fallback for an app that was launched before installation.
    if let Ok(layout) = Layout::detect() {
        let managed = layout.managed_singbox_bin();
        if is_executable_file(&managed) {
            return Some(managed);
        }
    }

    if let Some(bundled) = bundled_singbox_binary() {
        return Some(bundled);
    }

    if let Some(found) = find_in_path(exe_name) {
        return Some(found);
    }

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            "/opt/homebrew/bin/sing-box",
            "/usr/local/bin/sing-box",
            "/opt/local/bin/sing-box",
        ] {
            let p = Path::new(candidate);
            if is_executable_file(p) {
                return Some(p.to_path_buf());
            }
        }
    }

    #[cfg(windows)]
    {
        for base_var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(base) = env::var_os(base_var) {
                let direct = PathBuf::from(&base).join("sing-box").join("sing-box.exe");
                if is_executable_file(&direct) {
                    return Some(direct);
                }
            }
        }
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            let winget_dir = PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages");
            if let Some(found) = find_named_file_shallow(&winget_dir, "sing-box.exe", 3) {
                return Some(found);
            }
        }
    }

    None
}

/// Locate a sing-box copy shipped inside the app bundle next to this executable.
///
/// macOS: `sbctl.app/Contents/Resources/sing-box/sing-box`.
/// Windows: `sing-box\sing-box.exe` beside the app executable.
pub fn bundled_singbox_binary() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    let exe_name = if cfg!(windows) {
        "sing-box.exe"
    } else {
        "sing-box"
    };

    #[cfg(target_os = "macos")]
    let candidates = vec![dir.join("../Resources/sing-box").join(exe_name)];
    #[cfg(windows)]
    let candidates = vec![
        dir.join("sing-box").join(exe_name),
        dir.join("../sing-box").join(exe_name),
    ];
    #[cfg(not(any(target_os = "macos", windows)))]
    let candidates: Vec<PathBuf> = vec![dir.join("sing-box").join(exe_name)];

    candidates.into_iter().find(|path| is_executable_file(path))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        if !name.to_ascii_lowercase().ends_with(".exe") {
            let exe = dir.join(format!("{name}.exe"));
            if is_executable_file(&exe) {
                return Some(exe);
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_named_file_shallow(root: &Path, filename: &str, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 || !root.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(filename))
        {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_named_file_shallow(&path, filename, max_depth - 1)
        {
            return Some(found);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darwin_layout_preserves_canonical_paths() {
        let layout = Layout::darwin();
        assert_eq!(layout.platform, PlatformKind::Darwin);
        assert_eq!(layout.launchd_label, "system/app.lexiflix.singbox");
        assert_eq!(
            layout.plist_path,
            Path::new("/Library/LaunchDaemons/app.lexiflix.singbox.plist")
        );
        assert_eq!(
            layout.profiles_dir,
            Path::new("/usr/local/etc/sing-box/profiles")
        );
        assert_eq!(
            layout.active_config_path,
            Path::new("/usr/local/etc/sing-box/config.json")
        );
        assert_eq!(layout.active_name_path, None);
        assert!(layout.uses_sudo());
    }

    #[test]
    fn windows_layout_preserves_program_data_contract() {
        let layout = Layout::windows_with_program_data(Path::new(r"C:\ProgramData"));
        assert_eq!(layout.platform, PlatformKind::Windows);
        assert_eq!(layout.service_name, "sing-box");
        assert_eq!(
            layout.active_name_path.as_deref(),
            Some(
                Path::new(r"C:\ProgramData")
                    .join("sbctl")
                    .join("active-profile")
                    .as_path()
            )
        );
        assert!(!layout.uses_sudo());
    }

    #[test]
    fn managed_singbox_lives_outside_user_writable_roots() {
        let darwin = Layout::darwin();
        assert_eq!(
            darwin.managed_singbox_bin(),
            Path::new("/usr/local/libexec/sbctl/sing-box")
        );
        assert!(
            !darwin
                .managed_singbox_bin()
                .starts_with(&darwin.profiles_dir)
        );

        let windows = Layout::windows_with_program_data(Path::new(r"C:\ProgramData"));
        assert_eq!(
            windows.managed_singbox_bin(),
            Path::new(r"C:\ProgramData")
                .join("sbctl")
                .join("bin")
                .join("sing-box.exe")
        );
        assert!(
            !windows
                .managed_singbox_bin()
                .starts_with(&windows.profiles_dir)
        );
    }
}
