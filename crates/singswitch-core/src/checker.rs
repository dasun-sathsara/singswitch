use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{Result, SingSwitchError};
use crate::layout::resolve_singbox_binary;

/// Default validation timeout preventing a hung `sing-box` child process.
const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Cap on a latency probe, which is a single round trip and nothing more.
///
/// The 20s validation budget would make an unreachable server look like a hang
/// rather than the answer "unreachable".
const LATENCY_TIMEOUT: Duration = Duration::from_secs(5);

/// Endpoint the latency test fetches. A 204 with no body, so the measured time
/// is the round trip and not the download.
pub const DEFAULT_LATENCY_PROBE_URL: &str = "https://www.gstatic.com/generate_204";

/// How much of sing-box's complaint to keep in a probe error.
///
/// `tools fetch` writes the fetched body to stdout, so the useful line is at the
/// end of the combined output and the head is response HTML.
const PROBE_ERROR_TAIL: usize = 400;

/// Validates sing-box configuration files and reports the `sing-box` binary version.
pub trait Checker: Send + Sync {
    fn check(&self, path: &Path) -> Result<()>;
    fn version(&self) -> Result<String>;
}

/// Validates configurations by executing the real `sing-box` binary.
#[derive(Debug, Clone)]
pub struct ExecChecker {
    pub binary: Option<PathBuf>,
    pub timeout: Duration,
}

impl Default for ExecChecker {
    fn default() -> Self {
        Self {
            binary: None,
            timeout: DEFAULT_CHECK_TIMEOUT,
        }
    }
}

impl ExecChecker {
    fn resolve(&self) -> Result<PathBuf> {
        resolve_singbox_binary(self.binary.as_deref()).ok_or_else(SingSwitchError::sing_box_missing)
    }

    fn run_singbox(&self, args: &[&str]) -> Result<(bool, String)> {
        self.run_singbox_with_timeout(args, self.timeout)
    }

    /// Run the managed binary with a caller-chosen deadline.
    ///
    /// The probe cannot use `self.timeout`: 20s of waiting on a dead server is
    /// indistinguishable from a hung app.
    fn run_singbox_with_timeout(&self, args: &[&str], timeout: Duration) -> Result<(bool, String)> {
        let bin = self.resolve()?;
        let mut cmd = Command::new(&bin);
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    SingSwitchError::sing_box_missing()
                } else {
                    SingSwitchError::general(format!("could not run {}: {err}", bin.display()))
                }
            })?;

        let (status, combined) = crate::runner::wait_with_output(child, timeout, || {
            SingSwitchError::general(format!("sing-box timed out after {}s", timeout.as_secs()))
                .with_hint("check that the profile's server is reachable")
        })?;
        Ok((status.success(), combined))
    }

    /// Measure one round trip through `outbound_tag` without activating the
    /// profile.
    ///
    /// `sing-box tools fetch` builds the box and runs `PreStart()` only: no
    /// inbounds, so no TUN device, no listening ports, and no root. It then
    /// fetches `url` through the named outbound, which is exactly a latency
    /// test. The profile is read from `config_path` into a temp copy first; a
    /// profile can set `log.output`, and running the user's own file in place
    /// must never happen.
    pub fn probe_latency(
        &self,
        config_path: &Path,
        outbound_tag: &str,
        url: &str,
    ) -> Result<Duration> {
        let bytes = std::fs::read(config_path).map_err(|err| {
            SingSwitchError::general(format!(
                "could not read {} for a latency test: {err}",
                config_path.display()
            ))
            .with_hint("reload the profile list and try again")
        })?;
        self.probe_latency_bytes(&bytes, outbound_tag, url)
    }

    /// As [`probe_latency`](Self::probe_latency), for a document that is not on
    /// disk yet: the editor tests what is in the buffer, not the last save.
    pub fn probe_latency_bytes(
        &self,
        config: &[u8],
        outbound_tag: &str,
        url: &str,
    ) -> Result<Duration> {
        let tag = outbound_tag.trim();
        if tag.is_empty() {
            return Err(SingSwitchError::validation(
                "this profile has no outbound tag to test".to_string(),
            )
            .with_hint("give the outbound a tag in the JSON tab"));
        }
        // A measurement must not write anywhere. The temp copy protects the
        // config file itself, but sing-box still obeys `log.output` and creates
        // `experimental.cache_file` from inside the copied document: a probe on
        // a profile whose log points at another profile's JSON appends log
        // lines to it and corrupts it, and a probe on a config sharing a
        // cache_file with the running daemon deadlocks on the file lock until
        // the timeout. Both keys are forced inert in the probe copy.
        let inert = neutralise_side_effects(config)?;
        let temp = temp_profile_copy("singswitch-latency-", &inert)?;
        let path = temp.path().to_string_lossy().into_owned();
        let started = Instant::now();
        let (ok, output) = self
            .run_singbox_with_timeout(
                &["tools", "fetch", "-c", &path, "-o", tag, url],
                LATENCY_TIMEOUT,
            )
            .map_err(|err| {
                err.with_hint(
                    "the latency test needs sing-box 1.14 or newer and a reachable server",
                )
            })?;
        if ok {
            return Ok(started.elapsed());
        }
        let cleaned = crate::logs::strip_ansi(&output);
        let trimmed = cleaned.trim();
        let tail: String = trimmed
            .chars()
            .rev()
            .take(PROBE_ERROR_TAIL)
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        Err(SingSwitchError::general(format!(
            "latency test through {tag} failed: {}",
            tail.trim()
        ))
        .with_hint("check the outbound's server and that it is reachable from this machine"))
    }
}

/// Force every writing config key inert, so a probe writes nowhere.
///
/// `log.output` is removed (sing-box then logs to stderr only, which the probe
/// discards) and `experimental.cache_file` is removed (a probe shares nothing
/// with the running daemon, least of all its cache database). Everything else
/// in the document passes through untouched, including keys this version of
/// sing-box does not know.
fn neutralise_side_effects(config: &[u8]) -> Result<Vec<u8>> {
    // Fast path: the parse below exists only to remove `log.output` and
    // `experimental.cache_file`. Unless both quoted key names appear in the
    // document the copy is already inert, and the parse plus re-serialize of
    // the whole config (run on every latency probe) can be skipped outright.
    // If the key does appear the full path still runs, so odd spellings or
    // escapes never bypass the removal.
    let may_touch_log = contains_subslice(config, b"\"output\"");
    let may_touch_cache = contains_subslice(config, b"\"cache_file\"");
    if !may_touch_log && !may_touch_cache {
        return Ok(config.to_vec());
    }

    let mut root: serde_json::Value = serde_json::from_slice(config).map_err(|err| {
        SingSwitchError::validation(format!("the config is not valid JSON: {err}"))
    })?;
    if let Some(log) = root
        .get_mut("log")
        .and_then(serde_json::Value::as_object_mut)
    {
        log.remove("output");
    }
    if let Some(experimental) = root
        .get_mut("experimental")
        .and_then(serde_json::Value::as_object_mut)
    {
        experimental.remove("cache_file");
    }
    serde_json::to_vec(&root).map_err(|err| {
        SingSwitchError::general(format!("could not re-serialize the config: {err}"))
    })
}

/// Byte-slice substring test, used only as a cheap pre-check.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

impl Checker for ExecChecker {
    fn check(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();
        let (ok, output) = self.run_singbox(&["check", "-c", &path_str])?;
        if ok {
            Ok(())
        } else {
            Err(SingSwitchError::invalid_config(path_str.as_ref(), output))
        }
    }

    fn version(&self) -> Result<String> {
        let (ok, output) = self.run_singbox(&["version"])?;
        if !ok {
            return Err(SingSwitchError::general(format!(
                "sing-box version failed: {}",
                output.trim()
            )));
        }
        let first = output.lines().next().unwrap_or("").trim();
        let parts: Vec<&str> = first.split_whitespace().collect();
        if parts.len() >= 3 && parts[1] == "version" {
            Ok(parts[2].to_string())
        } else {
            Ok(first.to_string())
        }
    }
}

/// Write `bytes` to a temporary `.json` file and validate it through `checker`.
pub(crate) fn check_temp_bytes(checker: &dyn Checker, prefix: &str, bytes: &[u8]) -> Result<()> {
    let tmp = temp_profile_copy(prefix, bytes)?;
    checker.check(tmp.path())
}

/// Copy `bytes` into a private temp file that removes itself on drop.
///
/// Both the validator and the latency probe hand the file to sing-box instead
/// of the profile's own path: a profile can set `log.output`, so running it in
/// place would write into the user's log from a read-only check, and a probe
/// must not touch the profiles directory at all.
fn temp_profile_copy(prefix: &str, bytes: &[u8]) -> Result<tempfile::NamedTempFile> {
    let mut tmp = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".json")
        .tempfile()?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    Ok(tmp)
}
