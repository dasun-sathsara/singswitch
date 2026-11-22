use std::process::Command;

use serde_json::Value;

fn sbctl_bin() -> &'static str {
    env!("CARGO_BIN_EXE_sbctl")
}

#[test]
fn version_json_outputs_schema_v1() {
    let out = Command::new(sbctl_bin())
        .args(["version", "--json"])
        .output()
        .expect("run sbctl version --json");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let body: Value = serde_json::from_slice(&out.stdout).expect("valid JSON output");
    assert_eq!(body["schema"], 1);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["platform"].as_str().unwrap_or("").contains('/'));
}

#[test]
fn logs_without_follow_exits_and_honours_json() {
    // `--follow` used to default to true, so `sbctl logs` never returned.
    // There is no log-path override to point at a temp dir, so this reads the
    // real path and accepts either outcome; what it pins is that neither hangs.
    let out = Command::new(sbctl_bin())
        .args(["logs", "-n", "5", "--json"])
        .output()
        .expect("run sbctl logs -n 5 --json");

    // Either it has no log file yet (refused with a hint) or it printed schema 1
    // and exited. What must never happen is hanging.
    if out.status.success() {
        let body: Value = serde_json::from_slice(&out.stdout).expect("valid JSON output");
        assert_eq!(body["schema"], 1);
        assert!(body["lines"].is_array());
    } else {
        // Errors are reported as JSON on stdout in --json mode, with a
        // non-zero exit. Either way it must not hang.
        let body: Value = serde_json::from_slice(&out.stdout).expect("valid JSON error output");
        assert!(body["error"].is_string(), "a failure must explain itself");
    }
}

#[test]
fn print_sudoers_renders_scoped_darwin_rules() {
    let out = Command::new(sbctl_bin())
        .args(["print-sudoers", "--user", "alice"])
        .output()
        .expect("run sbctl print-sudoers");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("alice ALL=(root) NOPASSWD:"));
    // No globbed grant: sudo matches the whole argv as one string, so `*`
    // crossed `/` and the old activate rule permitted `../` traversal.
    assert!(
        !stdout.contains("*.json"),
        "sudoers must not grant a globbed path:\n{stdout}"
    );
    assert!(stdout.contains("/bin/launchctl kickstart -k system/app.lexiflix.singbox"));
    assert!(!stdout.contains(".."));
}

#[test]
fn unsupported_host_status_json_emits_structured_error() {
    let out = Command::new(sbctl_bin())
        .args(["status", "--json"])
        .output()
        .expect("run sbctl status --json");

    if cfg!(any(target_os = "macos", target_os = "windows")) {
        // On supported hosts, status outputs JSON schema 1 (ok or structured error).
        let body: Value = serde_json::from_slice(&out.stdout).expect("valid JSON output");
        assert_eq!(body["schema"], 1);
    } else {
        assert!(!out.status.success());
        let body: Value = serde_json::from_slice(&out.stdout).expect("structured error JSON");
        assert_eq!(body["schema"], 1);
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .contains("macOS and Windows")
        );
    }
}

/// The layout guard is a compile-time `cfg`, so on a host that is neither macOS
/// nor Windows every command fails before reaching its own logic. These two
/// tests only assert the contract that survives that: a non-zero exit with a
/// schema-1 JSON error and no prose on stdout. Reaching the `edit` argument
/// checks themselves needs a macOS or Windows host.
#[test]
fn failures_are_machine_readable_and_never_silent() {
    for args in [
        vec!["edit", "anything", "--json"],
        vec!["edit", "anything", "--set", "not_a_field=1", "--json"],
    ] {
        let out = Command::new(sbctl_bin())
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("run sbctl {args:?}: {e}"));
        let body: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("sbctl {args:?} printed non-JSON: {e}"));
        assert_eq!(body["schema"], 1, "{args:?}");
        assert!(
            body["error"].is_string(),
            "unsilent failure for {args:?}: {body}"
        );
        assert!(!out.status.success(), "{args:?} must exit non-zero");
    }
}
