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
    let dir = std::env::temp_dir().join(format!("sbctl-logs-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let out = Command::new(sbctl_bin())
        .args(["logs", "-n", "5", "--json"])
        .env("SBCTL_LOG_DIR", &dir)
        .output()
        .expect("run sbctl logs -n 5 --json");
    let _ = std::fs::remove_dir_all(&dir);

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
    assert!(stdout.contains(
        "/bin/ln -sfn /usr/local/etc/sing-box/profiles/*.json /usr/local/etc/sing-box/config.json"
    ));
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
