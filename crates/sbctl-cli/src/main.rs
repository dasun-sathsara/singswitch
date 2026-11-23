mod render;

use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use serde_json::{Value, json};

use render::{RenderOptions, Theme};
use sbctl_core::controller::Controller;
use sbctl_core::elevation;
use sbctl_core::error::{ExitCode, SbctlError};
use sbctl_core::ip::fetch_ip_info;
use sbctl_core::layout::PlatformKind;
use sbctl_core::logs::{read_appended_lines, read_tail_lines};
use sbctl_core::profile::{list_profiles, path_for, validate_name};
use sbctl_core::runner::TraceHook;
use sbctl_core::service::win_rpc::{PipeRequest, PipeResponse};

const SCHEMA_VERSION: u32 = 1;

#[derive(Parser, Debug)]
#[command(
    name = "sbctl",
    version,
    about = "Switch sing-box profiles and control the service that runs them",
    long_about = "sbctl manages sing-box client profiles and the privileged service that runs sing-box in TUN mode.\n\nRun with no subcommand to inspect status or pick a profile interactively."
)]
struct Cli {
    /// Emit machine-readable JSON with schema version 1
    #[arg(long, global = true)]
    json: bool,

    /// Use ASCII symbols and no borders or colour
    #[arg(long, global = true)]
    plain: bool,

    /// Disable colour while keeping Unicode symbols
    #[arg(long, global = true)]
    no_color: bool,

    /// Suppress informational output; errors still print
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Print commands and paths sbctl uses
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Hidden elevated RPC request input file (Windows UAC fallback)
    #[arg(long, hide = true)]
    elevated_op: Option<PathBuf>,

    /// Hidden elevated RPC response output file (Windows UAC fallback)
    #[arg(long, hide = true)]
    elevated_out: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List available profiles
    #[command(alias = "ls")]
    List,
    /// Switch to a profile
    Use {
        /// Profile name; omit to pick interactively
        name: Option<String>,
    },
    /// Stop sing-box
    Off,
    /// Show what sing-box is currently doing
    Status,
    /// Follow sing-box output
    Logs {
        /// Number of trailing lines to print initially
        #[arg(short = 'n', long, default_value_t = 80)]
        lines: usize,
        /// Follow new log lines until Ctrl-C
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Create a profile from the template and edit it
    Add {
        /// New profile name
        name: String,
    },
    /// Download a profile from a URL and validate it
    Pull {
        /// Local profile name
        name: String,
        /// HTTP(S) URL of the raw sing-box JSON config
        url: String,
    },
    /// Re-fetch remote profiles
    Sync {
        /// Sync only this profile; omit to sync every remote profile
        name: Option<String>,
    },
    /// Edit a profile and validate it
    Edit {
        /// Profile name
        name: String,
        /// Read the new configuration from a file ("-" for stdin) instead of
        /// opening $EDITOR. Required for scripts and agents.
        #[arg(long, value_name = "PATH")]
        from: Option<String>,
        /// Set a field as key=value (repeatable): server, port, uuid,
        /// server_name, public_key, short_id, tun_mtu, ...
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Delete a profile
    #[command(alias = "remove", alias = "delete")]
    Rm {
        /// Profile name
        name: String,
        /// Delete even if currently in use, stopping sing-box first
        #[arg(long)]
        force: bool,
    },
    /// Validate a profile (or the active one when omitted)
    Check {
        /// Profile name
        name: Option<String>,
    },
    /// Show the public IP address traffic is leaving from
    Ip,
    /// Show version information
    Version,
    /// Generate shell completions
    Completion {
        /// Target shell
        shell: Shell,
    },
    /// Print the scoped macOS sudoers rules sbctl requires (installer plumbing)
    #[command(hide = true)]
    PrintSudoers {
        /// Account to grant rules to (defaults to SUDO_USER / current user)
        #[arg(long)]
        user: Option<String>,
    },
    /// Run the Windows privileged Named Pipe helper service (Windows only)
    #[command(hide = true)]
    Agent {
        /// Run in the foreground instead of registering with the Windows SCM
        #[arg(long)]
        foreground: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let theme = Theme::new(RenderOptions::resolve(cli.plain, cli.no_color));

    if let Some(req_path) = &cli.elevated_op {
        let code = run_elevated_op_file(req_path, cli.elevated_out.as_deref());
        std::process::exit(code.as_i32());
    }

    let verbose = cli.verbose;
    let trace: Option<TraceHook> = if verbose {
        Some(Arc::new(|prog, args| {
            eprintln!("[debug] exec: {prog} {}", args.join(" "));
        }))
    } else {
        None
    };

    if let Some(Commands::Completion { shell }) = &cli.command {
        let mut cmd = Cli::command();
        generate(*shell, &mut cmd, "sbctl", &mut io::stdout());
        std::process::exit(0);
    }

    if let Some(Commands::PrintSudoers { user }) = &cli.command {
        let layout = sbctl_core::layout::Layout::darwin();
        let code = match cmd_print_sudoers_for_layout(&layout, user.as_deref()) {
            Ok(()) => ExitCode::Ok,
            Err(err) => report_error(&cli, &theme, &err),
        };
        std::process::exit(code.as_i32());
    }

    if let Some(Commands::Version) = &cli.command {
        let code = match cmd_version_standalone(&cli, &theme) {
            Ok(()) => ExitCode::Ok,
            Err(err) => report_error(&cli, &theme, &err),
        };
        std::process::exit(code.as_i32());
    }

    if let Some(Commands::Agent { foreground }) = &cli.command {
        match run_agent_command(*foreground) {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                eprintln!("{}", theme.render_error(&err));
                std::process::exit(err.code.as_i32());
            }
        }
    }

    let code = match Controller::from_system(trace) {
        Ok(ctrl) => match run(&cli, &ctrl, &theme) {
            Ok(()) => ExitCode::Ok,
            Err(err) => report_error(&cli, &theme, &err),
        },
        Err(err) => report_error(&cli, &theme, &err),
    };
    std::process::exit(code.as_i32());
}

fn run(cli: &Cli, ctrl: &Controller, theme: &Theme) -> sbctl_core::error::Result<()> {
    match &cli.command {
        None | Some(Commands::Use { name: None }) => run_root(cli, ctrl, theme),
        Some(Commands::List) => cmd_list(cli, ctrl, theme),
        Some(Commands::Use { name: Some(n) }) => cmd_use(cli, ctrl, theme, n),
        Some(Commands::Off) => cmd_off(cli, ctrl, theme),
        Some(Commands::Status) => cmd_status(cli, ctrl, theme),
        Some(Commands::Logs { lines, follow }) => cmd_logs(cli, ctrl, *lines, *follow),
        Some(Commands::Add { name }) => cmd_add(cli, ctrl, theme, name),
        Some(Commands::Pull { name, url }) => cmd_pull(cli, ctrl, theme, name, url),
        Some(Commands::Sync { name }) => cmd_sync(cli, ctrl, theme, name.as_deref()),
        Some(Commands::Edit { name, from, set }) => {
            cmd_edit(cli, ctrl, theme, name, from.as_deref(), set)
        }
        Some(Commands::Rm { name, force }) => cmd_rm(cli, ctrl, theme, name, *force),
        Some(Commands::Check { name }) => cmd_check(cli, ctrl, theme, name.as_deref()),
        Some(Commands::Ip) => cmd_ip(cli, theme),
        Some(Commands::Version)
        | Some(Commands::Completion { .. })
        | Some(Commands::PrintSudoers { .. })
        | Some(Commands::Agent { .. }) => unreachable!("handled above"),
    }
}

fn run_root(cli: &Cli, ctrl: &Controller, theme: &Theme) -> sbctl_core::error::Result<()> {
    let snap = ctrl.snapshot()?;
    let interactive =
        !cli.json && !cli.quiet && io::stdout().is_terminal() && io::stdin().is_terminal();

    if snap.profiles.is_empty() || !interactive {
        return print_status_view(cli, theme, &snap.status_view, snap.profiles.is_empty());
    }

    println!("{}", theme.render_status(&snap.status_view));
    println!("Profiles:");
    for (idx, p) in snap.profiles.iter().enumerate() {
        let marker = if !p.ready() {
            "missing"
        } else if snap.active.as_deref() == Some(p.name.as_str()) && snap.running {
            "active"
        } else if snap.active.as_deref() == Some(p.name.as_str()) {
            "configured"
        } else {
            ""
        };
        let suffix = if marker.is_empty() {
            String::new()
        } else {
            format!(" ({marker})")
        };
        println!("  [{}] {}{}", idx + 1, p.name, suffix);
    }
    println!("  [0] Turn off sing-box");
    print!("Select number (Enter to cancel): ");
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("q") {
        return Ok(());
    }
    let choice: usize = trimmed
        .parse()
        .map_err(|_| SbctlError::general(format!("invalid selection {trimmed:?}")))?;
    if choice == 0 {
        return cmd_off(cli, ctrl, theme);
    }
    let Some(selected) = snap.profiles.get(choice - 1) else {
        return Err(SbctlError::general("selection out of range"));
    };
    cmd_use(cli, ctrl, theme, &selected.name)
}

fn cmd_list(cli: &Cli, ctrl: &Controller, theme: &Theme) -> sbctl_core::error::Result<()> {
    let snap = ctrl.snapshot()?;
    if cli.json {
        let entries: Vec<Value> = snap
            .profiles
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "active": snap.active.as_deref() == Some(p.name.as_str()) && snap.running,
                    "configured": snap.active.as_deref() == Some(p.name.as_str()),
                    "ready": p.ready(),
                    "missing": field_keys(&p.missing),
                    "server": p.facts.endpoint,
                    "tun": p.facts.tun_interface,
                    "source_url": p.source_url,
                })
            })
            .collect();
        return emit_json(&json!({ "profiles": entries }));
    }
    if cli.quiet {
        return Ok(());
    }
    if snap.profiles.is_empty() {
        println!("{}", theme.muted("There are no profiles yet."));
        println!("{}", theme.hint("create one with: sbctl add <name>"));
        return Ok(());
    }
    print!(
        "{}",
        theme.render_profile_list(&snap.profiles, snap.active.as_deref(), snap.running)
    );
    if let Some(broken) = &snap.broken {
        eprintln!("{}", theme.warn(broken));
    }
    Ok(())
}

fn cmd_status(cli: &Cli, ctrl: &Controller, theme: &Theme) -> sbctl_core::error::Result<()> {
    let snap = ctrl.snapshot()?;
    print_status_view(cli, theme, &snap.status_view, false)
}

fn print_status_view(
    cli: &Cli,
    theme: &Theme,
    view: &sbctl_core::controller::StatusView,
    empty_hint: bool,
) -> sbctl_core::error::Result<()> {
    if cli.json {
        let mut payload = serde_json::Map::new();
        payload.insert("state".into(), json!(view.state));
        payload.insert("profile".into(), json!(view.profile));
        payload.insert("running".into(), json!(view.running));
        payload.insert("tun".into(), json!(view.tun));
        payload.insert("server".into(), json!(view.server));
        payload.insert("missing".into(), json!(field_keys(&view.missing)));
        if let Some(pid) = view.pid {
            payload.insert("pid".into(), json!(pid));
        }
        if let Some(broken) = &view.broken {
            payload.insert("broken".into(), json!(broken));
        }
        return emit_json(&Value::Object(payload));
    }
    if cli.quiet {
        return Ok(());
    }
    print!("{}", theme.render_status(view));
    if view.broken.is_some() {
        println!(
            "{}",
            theme.hint("choose a working profile with: sbctl use <name>")
        );
    }
    if empty_hint {
        println!("\n{}", theme.muted("There are no profiles yet."));
        println!("{}", theme.hint("create one with: sbctl add <name>"));
    }
    Ok(())
}

fn cmd_use(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: &str,
) -> sbctl_core::error::Result<()> {
    if !elevate_if_needed(&ctrl.layout, &["use", name])? {
        ctrl.activate(name)?;
    }
    if cli.json {
        return emit_json(&json!({ "profile": name, "state": "running" }));
    }
    if !cli.quiet {
        println!("{}", theme.ok(&format!("now using {name}")));
    }
    Ok(())
}

fn cmd_off(cli: &Cli, ctrl: &Controller, theme: &Theme) -> sbctl_core::error::Result<()> {
    if !elevate_if_needed(&ctrl.layout, &["off"])? {
        ctrl.stop()?;
    }
    if cli.json {
        return emit_json(&json!({ "state": "stopped" }));
    }
    if !cli.quiet {
        println!("{}", theme.ok("sing-box stopped"));
    }
    Ok(())
}

fn cmd_logs(
    cli: &Cli,
    ctrl: &Controller,
    lines: usize,
    follow: bool,
) -> sbctl_core::error::Result<()> {
    let path = &ctrl.layout.error_log_path;
    if !path.exists() {
        return Err(
            SbctlError::general(format!("there is no log file at {} yet", path.display()))
                .with_hints([
                    "sing-box writes it once it has run; start it with: sbctl use <name>",
                    "check the service state with: sbctl status",
                ]),
        );
    }

    let (initial, mut offset) = read_tail_lines(path, lines)?;
    if cli.json {
        if !follow {
            // One document for a bounded read.
            emit_json(&json!({ "lines": initial }))?;
            return Ok(());
        }
        // Following is unbounded, so emit JSON Lines: one object per line, each
        // a complete document. A single header plus raw lines would be
        // unparseable.
        for line in &initial {
            emit_json_line(&json!({ "line": line }))?;
        }
    } else {
        for line in &initial {
            println!("{line}");
        }
    }
    if !follow {
        return Ok(());
    }

    loop {
        std::thread::sleep(Duration::from_millis(350));
        let (appended, new_offset, jumped) = read_appended_lines(path, offset)?;
        offset = new_offset;
        if jumped {
            // The file was rotated or truncated, so lines were skipped between
            // what was already printed and what follows. Say so instead of
            // splicing two unrelated eras of the log silently.
            if cli.json {
                emit_json_line(&json!({ "gap": "log rotated or truncated" }))?;
            } else {
                eprintln!("-- log rotated or truncated; earlier lines were skipped --");
            }
        }
        for line in appended {
            if cli.json {
                emit_json_line(&json!({ "line": line }))?;
            } else {
                println!("{line}");
            }
        }
    }
}

fn cmd_add(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: &str,
) -> sbctl_core::error::Result<()> {
    let path = ctrl.create_from_skeleton(name)?;
    // `--json` and a non-tty both mean "no interactive editor", and the machine
    // output must come first: a caller that asked for JSON never wants the
    // editor, however the process was launched.
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() || cli.json {
        if cli.json {
            return emit_json(&json!({ "profile": name, "path": path }));
        }
        if !cli.quiet {
            println!("{}", theme.ok(&format!("created {name}")));
            println!("{}", theme.hint(&format!("edit it at {}", path.display())));
        }
        return Ok(());
    }

    let kept = edit_loop(ctrl, theme, &path, || {
        let _ = fs::remove_file(&path);
    })?;
    if !kept {
        return Err(SbctlError::validation(format!(
            "{name} was not created because it is not valid"
        )));
    }
    if !cli.quiet {
        println!("{}", theme.ok(&format!("created {name}")));
        let missing = list_profiles(&ctrl.layout.profiles_dir)
            .ok()
            .and_then(|ps| ps.into_iter().find(|p| p.name == name))
            .map(|p| p.missing)
            .unwrap_or_default();
        print_readiness(theme, name, &missing);
    }
    Ok(())
}

/// The "what is left to do" tail shared by `add` and `pull`.
fn print_readiness(theme: &Theme, name: &str, missing: &[sbctl_core::readiness::Field]) {
    if missing.is_empty() {
        println!(
            "{}",
            theme.hint(&format!("activate it with: sbctl use {name}"))
        );
        return;
    }
    println!(
        "{}",
        theme.warn(&format!(
            "it still needs: {}",
            sbctl_core::readiness::join_labels(missing)
        ))
    );
    println!(
        "{}",
        theme.hint(&format!("fill them in with: sbctl edit {name}"))
    );
}

fn cmd_edit(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: &str,
    from: Option<&str>,
    set: &[String],
) -> sbctl_core::error::Result<()> {
    validate_name(name)?;
    let profiles = list_profiles(&ctrl.layout.profiles_dir)?;
    if !profiles.iter().any(|p| p.name == name) {
        let available: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
        return Err(SbctlError::profile_not_found(name, &available));
    }

    let path = path_for(&ctrl.layout.profiles_dir, name);
    let original = fs::read_to_string(&path)?;

    // Non-interactive path: an agent supplies the whole document, or field
    // values against the existing one. No $EDITOR, no prompt.
    if from.is_some() || !set.is_empty() {
        let mut content = match from {
            Some("-") => {
                use std::io::Read as _;
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            }
            Some(src) => fs::read_to_string(src)?,
            None => original.clone(),
        };
        if !set.is_empty() {
            let mut model = sbctl_core::form_model::ProfileFormModel::from_json_str(&content)?;
            for pair in set {
                let (key, value) = pair.split_once('=').ok_or_else(|| {
                    SbctlError::validation(format!("--set expects key=value, got {pair:?}"))
                })?;
                model.set_field(key.trim(), value)?;
            }
            let (applied, skipped) = model.apply_to_json_str_reporting(&content)?;
            content = applied;
            if !skipped.is_empty() {
                // Exit non-zero: the caller asked for a change that this config
                // has nowhere to put, so claiming success would be a lie.
                return Err(SbctlError::validation(format!(
                    "{name} has no {} section, so those values were not written",
                    skipped.join(" or ")
                ))
                .with_hint("edit the JSON directly with: sbctl edit --from -"));
            }
        }
        let reloaded = ctrl.save_profile(name, &content, false)?;
        if cli.json {
            let p = list_profiles(&ctrl.layout.profiles_dir)?
                .into_iter()
                .find(|p| p.name == name);
            return emit_json(&json!({
                "profile": name,
                "reloaded": reloaded,
                "ready": p.as_ref().is_some_and(|p| p.ready()),
                "missing": field_keys(&p.map(|p| p.missing).unwrap_or_default()),
            }));
        }
        if !cli.quiet {
            println!("{}", theme.ok(&format!("{name} is valid")));
            if reloaded {
                println!(
                    "{}",
                    theme.ok(&format!("reloaded sing-box with the updated {name}"))
                );
            }
        }
        return Ok(());
    }

    if cli.json {
        // Interactive editing cannot produce machine output.
        return Err(SbctlError::validation(
            "edit needs --from or --set when --json is used",
        ));
    }

    let kept = edit_loop(ctrl, theme, &path, || {
        let _ = fs::write(&path, &original);
    })?;
    if !kept {
        return Err(SbctlError::validation(format!(
            "{name} is not a valid configuration"
        )));
    }

    let updated = fs::read_to_string(&path)?;
    let reloaded = ctrl.save_profile(name, &updated, false)?;
    if !cli.quiet {
        println!("{}", theme.ok(&format!("{name} is valid")));
        if reloaded {
            println!(
                "{}",
                theme.ok(&format!("reloaded sing-box with the updated {name}"))
            );
        }
    }
    Ok(())
}

fn edit_loop(
    ctrl: &Controller,
    theme: &Theme,
    path: &Path,
    discard: impl Fn(),
) -> sbctl_core::error::Result<bool> {
    loop {
        launch_editor(path)?;
        match ctrl.checker.check(path) {
            Ok(()) => return Ok(true),
            Err(err) => {
                if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
                    return Err(err);
                }
                eprintln!("{}", theme.render_error(&err));
                print!("Validation failed: [r]etry, [d]iscard, or [k]eep as-is? ");
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().lock().read_line(&mut answer)?;
                match answer.trim().to_ascii_lowercase().as_str() {
                    "r" | "retry" | "" => continue,
                    "d" | "discard" => {
                        discard();
                        println!("{}", theme.ok("changes discarded"));
                        return Ok(false);
                    }
                    "k" | "keep" => {
                        eprintln!(
                            "{}",
                            theme.warn("keeping an invalid configuration; it cannot be activated until it validates")
                        );
                        return Ok(false);
                    }
                    other => {
                        return Err(SbctlError::general(format!(
                            "unrecognised choice {other:?}"
                        )));
                    }
                }
            }
        }
    }
}

fn launch_editor(path: &Path) -> sbctl_core::error::Result<()> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "nano".to_string()
            }
        });
    let mut parts = editor.split_whitespace();
    let prog = parts
        .next()
        .ok_or_else(|| SbctlError::general("EDITOR is empty"))?;
    let mut cmd = Command::new(prog);
    for arg in parts {
        cmd.arg(arg);
    }
    cmd.arg(path);
    let status = cmd.status().map_err(|e| {
        SbctlError::general(format!("could not open editor {prog:?}: {e}"))
            .with_hint("set $EDITOR to an installed editor")
    })?;
    if !status.success() {
        return Err(SbctlError::general(format!(
            "editor {prog:?} exited with status {status}"
        )));
    }
    Ok(())
}

fn cmd_pull(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: &str,
    url: &str,
) -> sbctl_core::error::Result<()> {
    let profile = ctrl.pull_profile(name, url)?;
    if cli.json {
        return emit_json(&json!({
            "profile": profile.name,
            "url": profile.source_url,
            "missing": field_keys(&profile.missing),
        }));
    }
    if !cli.quiet {
        println!("{}", theme.ok(&format!("pulled {}", profile.name)));
        print_readiness(theme, &profile.name, &profile.missing);
    }
    Ok(())
}

fn cmd_sync(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: Option<&str>,
) -> sbctl_core::error::Result<()> {
    if let Some(target) = name {
        let reloaded = ctrl.sync_one(target)?;
        if cli.json {
            return emit_json(&json!({ "synced": [target], "reloaded": reloaded }));
        }
        if !cli.quiet {
            println!("{}", theme.ok(&format!("synced {target}")));
        }
        return Ok(());
    }

    let report = ctrl.sync_all()?;
    if cli.json {
        let failed: Vec<Value> = report
            .failed
            .iter()
            .map(|(n, err)| json!({ "name": n, "error": err }))
            .collect();
        emit_json(&json!({ "synced": report.synced, "failed": failed }))?;
        if !report.failed.is_empty() {
            return Err(SbctlError::general("one or more profiles failed to sync").mark_reported());
        }
        return Ok(());
    }
    if !cli.quiet {
        if report.synced.is_empty() && report.failed.is_empty() {
            println!("{}", theme.muted("No profiles have a recorded source URL."));
            println!(
                "{}",
                theme.hint("fetch one first with: sbctl pull <name> <url>")
            );
            return Ok(());
        }
        for name in &report.synced {
            println!("{}", theme.ok(&format!("synced {name}")));
        }
        for (name, err) in &report.failed {
            eprintln!("{}", theme.warn(&format!("could not sync {name}: {err}")));
        }
    }
    if !report.failed.is_empty() {
        let count = report.failed.len();
        let noun = if count == 1 { "profile" } else { "profiles" };
        return Err(
            SbctlError::general(format!("{count} {noun} failed to sync"))
                .with_hint("inspect the service output with: sbctl logs"),
        );
    }
    Ok(())
}

fn cmd_rm(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: &str,
    force: bool,
) -> sbctl_core::error::Result<()> {
    validate_name(name)?;
    let interactive =
        !cli.json && !cli.quiet && io::stdout().is_terminal() && io::stdin().is_terminal();
    if !force {
        if interactive {
            print!("Delete profile {name}? [y/N]: ");
            io::stdout().flush()?;
            let mut ans = String::new();
            io::stdin().lock().read_line(&mut ans)?;
            let ans = ans.trim();
            if !(ans.eq_ignore_ascii_case("y") || ans.eq_ignore_ascii_case("yes")) {
                println!("{}", theme.muted("nothing was deleted"));
                return Ok(());
            }
        } else {
            return Err(SbctlError::general(
                "deleting a profile needs confirmation, and there is no terminal to ask on",
            )
            .with_hint(format!("re-run with: sbctl rm {name} --force")));
        }
    }

    let was_active = ctrl.delete_profile(name, force)?;
    if cli.json {
        return emit_json(&json!({ "deleted": name, "was_active": was_active }));
    }
    if !cli.quiet {
        println!("{}", theme.ok(&format!("deleted {name}")));
        if was_active {
            eprintln!(
                "{}",
                theme.warn(&format!(
                    "the active configuration still refers to {name}, which no longer exists"
                ))
            );
            println!(
                "{}",
                theme.hint("point it somewhere valid with: sbctl use <name>")
            );
        }
    }
    Ok(())
}

fn cmd_check(
    cli: &Cli,
    ctrl: &Controller,
    theme: &Theme,
    name: Option<&str>,
) -> sbctl_core::error::Result<()> {
    match ctrl.check_profile(name) {
        Ok((checked_name, _)) => {
            if cli.json {
                return emit_json(&json!({
                    "profile": checked_name,
                    "valid": true,
                    "missing": [],
                }));
            }
            if !cli.quiet {
                println!(
                    "{}",
                    theme.ok(&format!("{checked_name} is a valid configuration"))
                );
            }
            Ok(())
        }
        Err(err) => {
            if cli.json {
                emit_json(&json!({
                    "profile": name.unwrap_or(""),
                    "valid": false,
                    "error": err.message,
                }))?;
                return Err(err.mark_reported());
            }
            Err(err)
        }
    }
}

fn cmd_ip(cli: &Cli, theme: &Theme) -> sbctl_core::error::Result<()> {
    let info = fetch_ip_info(None)?;
    if cli.json {
        return emit_json(
            &serde_json::to_value(&info).map_err(|e| SbctlError::general(e.to_string()))?,
        );
    }
    if !cli.quiet {
        print!("{}", theme.render_ip(&info));
    }
    Ok(())
}

fn cmd_version_standalone(cli: &Cli, theme: &Theme) -> sbctl_core::error::Result<()> {
    let checker = sbctl_core::checker::ExecChecker::default();
    emit_version_with_checker(cli, &checker, theme)
}

fn emit_version_with_checker(
    cli: &Cli,
    checker: &dyn sbctl_core::checker::Checker,
    theme: &Theme,
) -> sbctl_core::error::Result<()> {
    let sbctl_ver = env!("CARGO_PKG_VERSION");
    let sing_box = checker.version().ok();
    let platform = format!("{}/{}", env::consts::OS, env::consts::ARCH);

    if cli.json {
        return emit_json(&json!({
            "version": sbctl_ver,
            "sing_box": sing_box.as_deref().unwrap_or(""),
            "platform": platform,
        }));
    }
    if !cli.quiet {
        let reported = sing_box.unwrap_or_else(|| "not installed".to_string());
        print!(
            "{}",
            theme.kv_block(&[
                ("sbctl", sbctl_ver.to_string()),
                ("sing-box", reported),
                ("platform", platform),
            ])
        );
    }
    Ok(())
}

fn cmd_print_sudoers_for_layout(
    layout: &sbctl_core::layout::Layout,
    user: Option<&str>,
) -> sbctl_core::error::Result<()> {
    let target_user = match user {
        Some(u) => u.to_string(),
        None => env::var("SUDO_USER")
            .or_else(|_| env::var("USER"))
            .map_err(|_| {
                SbctlError::general("could not determine which user to grant access to")
                    .with_hint("name it explicitly with: sbctl print-sudoers --user <name>")
            })?,
    };
    let rules = layout.sudoers(&target_user)?;
    print!("{rules}");
    Ok(())
}

/// Hand the command to an elevated child when this process cannot do it.
///
/// Returns whether the child ran. Callers must still print their normal output
/// afterwards: returning early here meant `--json` emitted nothing at all on
/// Windows.
fn elevate_if_needed(
    layout: &sbctl_core::layout::Layout,
    args: &[&str],
) -> sbctl_core::error::Result<bool> {
    let Some(code) = maybe_elevate_cli(layout, args)? else {
        return Ok(false);
    };
    if code == 0 {
        return Ok(true);
    }
    Err(SbctlError::new(
        ExitCode::from_i32(code),
        "the elevated sbctl process reported a failure",
    ))
}

fn maybe_elevate_cli(
    layout: &sbctl_core::layout::Layout,
    args: &[&str],
) -> sbctl_core::error::Result<Option<i32>> {
    if layout.platform != PlatformKind::Windows || elevation::is_elevated() {
        return Ok(None);
    }
    let exe = env::current_exe()
        .map_err(|e| SbctlError::general(format!("cannot determine executable path: {e}")))?;
    let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let code = elevation::run_elevated_executable(&exe, &owned, false)?;
    Ok(Some(code))
}

fn run_elevated_op_file(req_path: &Path, out_path: Option<&Path>) -> ExitCode {
    let raw = match fs::read_to_string(req_path) {
        Ok(s) => s,
        Err(err) => {
            write_elevated_out(
                out_path,
                &PipeResponse::from_error(&SbctlError::general(err.to_string())),
            );
            return ExitCode::General;
        }
    };
    let req: PipeRequest = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(err) => {
            write_elevated_out(
                out_path,
                &PipeResponse::from_error(&SbctlError::general(format!("invalid request: {err}"))),
            );
            return ExitCode::General;
        }
    };
    let resp = match Controller::from_system(None) {
        Ok(ctrl) => ctrl.handle_pipe_request(req),
        Err(err) => PipeResponse::from_error(&err),
    };
    let code = ExitCode::from_i32(resp.code);
    write_elevated_out(out_path, &resp);
    code
}

fn write_elevated_out(out_path: Option<&Path>, resp: &PipeResponse) {
    if let Some(path) = out_path
        && let Ok(json) = serde_json::to_vec(resp)
    {
        let _ = fs::write(path, json);
    }
}

/// Machine names for the fields a profile still needs, so a script can map each
/// back to the `--set` key that fills it.
fn field_keys(fields: &[sbctl_core::readiness::Field]) -> Vec<&'static str> {
    fields.iter().map(|f| f.key()).collect()
}

fn emit_json(val: &Value) -> sbctl_core::error::Result<()> {
    emit_json_with(val, true)
}

/// Compact form for streams: one complete object per line, so a reader can parse
/// each line independently. Pretty-printing a follow stream produced multi-line
/// objects that no line-oriented parser could read.
fn emit_json_line(val: &Value) -> sbctl_core::error::Result<()> {
    emit_json_with(val, false)
}

fn emit_json_with(val: &Value, pretty: bool) -> sbctl_core::error::Result<()> {
    let mut obj = match val {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    obj.insert("schema".into(), json!(SCHEMA_VERSION));
    let body = if pretty {
        serde_json::to_string_pretty(&Value::Object(obj))
    } else {
        serde_json::to_string(&Value::Object(obj))
    }
    .map_err(|e| SbctlError::general(e.to_string()))?;
    println!("{body}");
    Ok(())
}

fn report_error(cli: &Cli, theme: &Theme, err: &SbctlError) -> ExitCode {
    if err.reported {
        return err.code;
    }
    if cli.json {
        let mut payload = serde_json::Map::new();
        payload.insert("error".into(), json!(err.message));
        payload.insert("code".into(), json!(err.code.as_i32()));
        if !err.hints.is_empty() {
            payload.insert("hints".into(), json!(err.hints));
        }
        let _ = emit_json(&Value::Object(payload));
        return err.code;
    }
    eprint!("{}", theme.render_error(err));
    if cli.verbose
        && let Some(cause) = &err.cause
    {
        eprintln!("[debug] cause: {cause}");
    }
    err.code
}

#[cfg(windows)]
fn run_agent_command(foreground: bool) -> sbctl_core::error::Result<()> {
    use sbctl_core::service::win_rpc::{HELPER_SERVICE_NAME, run_pipe_server};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration as StdDuration;

    if foreground {
        let ctrl = Controller::from_system(None)?;
        let stop = AtomicBool::new(false);
        return run_pipe_server(&ctrl.layout.pipe_name, &stop, |req| {
            ctrl.handle_pipe_request(req)
        });
    }

    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
    };

    static STOP_FLAG: AtomicBool = AtomicBool::new(false);

    define_windows_service!(ffi_service_main, sbctl_agent_service_main);

    fn sbctl_agent_service_main(_args: Vec<std::ffi::OsString>) {
        STOP_FLAG.store(false, Ordering::SeqCst);
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    STOP_FLAG.store(true, Ordering::SeqCst);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let Ok(status_handle) =
            service_control_handler::register(HELPER_SERVICE_NAME, event_handler)
        else {
            return;
        };

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: StdDuration::default(),
            process_id: None,
        });

        if let Ok(ctrl) = Controller::from_system(None) {
            let _ = run_pipe_server(&ctrl.layout.pipe_name, &STOP_FLAG, |req| {
                ctrl.handle_pipe_request(req)
            });
        }

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: StdDuration::default(),
            process_id: None,
        });
    }

    service_dispatcher::start(HELPER_SERVICE_NAME, ffi_service_main).map_err(|err| {
        SbctlError::service(format!("Windows service dispatcher failed: {err}"))
            .with_hint("pass --foreground when running manually outside SCM")
    })
}

#[cfg(not(windows))]
fn run_agent_command(_foreground: bool) -> sbctl_core::error::Result<()> {
    Err(SbctlError::general(
        "sbctl agent is a Windows-only named pipe helper service",
    ))
}
