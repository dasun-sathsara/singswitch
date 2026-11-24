# SingSwitch engineering standards

This document is a **binding contract** for every change to this repository,
whether made by a human or by an AI agent. It carries over the conventions
Zed's codebase observes (precision in error handling, ruthless consistency,
small reviewable diffs, no silent failures), adapted to this project's size.
Where Zed's practices don't fit, we adapt rather than copy.

## Enforcement

These rules are not optional and not advisory. `make check` (format check,
clippy `-D warnings`, icon audit) must pass before every commit. CI runs the
same gate on every push. The lints below are workspace-level and cannot be
relaxed per-crate except through the documented exception process.

## Hard constraints

1. **`make check` must pass on `main` at all times.** A red `main` is a bug in
   the last commit, not a follow-up item.
2. **Warnings are errors.** The workspace sets `#![deny(warnings)]` semantics
   through Clippy `-D warnings` in `make lint`. If Clippy or `rustc` emits a
   warning, fix the code; do not add `#[allow]`. Adding `#[allow(...)]`
   requires an inline comment naming the concrete reason the lint is wrong
   here, and the check must still pass with the allow removed and the
   rationale verified.
3. **Privileged paths are reviewed, not bypassed.** Sudoers rules (`core/
   sudoers.rs`), privilege elevation (`core/elevation.rs`), and the install
   scripts in `scripts/` change what root executes. Any diff that touches
   them must call this out explicitly in its commit message under a
   `security:` trailer. CI diff-checks these paths on every push and flags
   changes in the job summary.
4. **No `unwrap()` on values that can be `None`/`Err` at runtime.** Use the
   error combinators or propagate. `unwrap()` is acceptable only where the
   invariant is compile-time provable (e.g., slicing a known-length array,
   static regex) and needs an inline comment.
5. **Blocked operations stay off the UI thread.** Anything that talks to a
   process, the network, or the disk goes through the existing async layer.
6. **No new panics on input paths.** Profile names, config files, daemon
   output, and environment variables are untrusted. Parse to `Result`, never
   `expect`.
7. **Preserve behavior.** Refactors must not change user-visible behavior
   unless the change is the task. UI polish changes geometry and color, not
   what the app does.

## Error handling

This codebase's pattern (keep it): values flow as `Result<T, SingSwitchError>`
(or `anyhow`-style context at the CLI boundary only), errors carry exit codes
and hints because users act on them (`error.rs`).

- Never `unwrap()`, `expect()`, `panic!()` on untrusted input or fallible I/O.
- Error messages must state what failed and what to do next (`hints`), since
  the failure is shown in a GUI banner or CLI output, not in a debugger.
- Don't create new error flavours. Extend `CoreErrorKind` or the
  `SingSwitchError` hints.
- Log errors to `~/Library/Logs/singswitch/app.log` via `diag::log` only on
  paths where stderr is not visible (bundled app). Never swallow errors
  silently, and never log **and** show the same error to the user.

## Code organization

- Workspace: `crates/singswitch-core` (headless, no GPUI), `singswitch-cli`,
  `singswitch-app` (GUI). The dependency direction is app → core, cli → core.
  Never import from the GUI in core.
- One responsibility per module. Keep it where it is; don't restructure for
  restructure's sake.
- Keep functions under ~60 lines where possible. The GPUI view files tolerate
  longer builder chains; logic functions do not.
- Prefer `impl Trait` returns and the existing GPUI builder style. Match the
  surrounding code first, apply Rust idioms second.

## Style

- `cargo fmt` is the authority. Hand-formatted files are a build violation.
- No dead code. Public items in `core` are the public API; app/CLI items are
  internal and get deleted when their last use goes.
- Units go in the type or name: `DevicePixels`, `px()`, `Duration`. Never a
  bare `u32` that means pixels.
- Comments say **why**. History says what; git blame answers "what changed".
  Keep comments focused on the non-obvious invariant.

## Concurrency

- The store updates synchronously on its GPUI context. Keep user/spawned work
  in `smol` tasks; every `.await` must be inside a task, never in context
  callbacks.
- If you add shared state, it goes in `Entity`/`App` state (GPUI's model), not
  a `static Mutex`, unless there is no GPUI context.
- Re-entrancy through `dispatch_action` must be audited: an unhandled action
  is silently dropped (we fixed one instance). Interactive view code calls
  state transitions directly; the global handler is for menu/tray/key paths.

## Performance

- No new dependencies without a workspace-level justification in the PR.
- The log views are virtualized against a cache; keep list rendering O(visible).
  Reverting to per-row `format!` allocation or rebuilding filters on every
  keystroke is a regression.
- No synchronous I/O in `render`. No allocations in hover/blink paths (the
  editor's derived-JSON cache pattern is the reference).
- Release builds are `lto = "thin"`; do not add heavy fit-to-link features
  that blow up the bundle.

## Testing and verification

- There is no test suite, by decision: the reasoning lives in git history
  (the mock seams the tests needed distorted the design). What exists instead:
  - `make check` gates format, Clippy, icon coverage.
  - `scripts/check-macos.sh` / `check-windows.sh` cross-check.
  - `snap.py` renders deterministic UI scenes against fixed snapshots.
- Do not add unit tests to this codebase; a change is verified by the gate,
  by `snap.py` captures, and by adversarial review, not by tests.
- Verify UI changes **visually** with `snap.py` before pushing. Do not
  describe screenshots you did not render.

## Commit messages

- Subject line: imperative mood, lowercase, no period.
- Body: explain **why** the change exists. Reference the user-visible
  behavior if there is one.
- Committing directly to `main` is allowed in this repo, but never mix
  unrelated changes into one commit. UI polish and refactor groundwork are
  two commits even if made in one sitting.

## Exception process

Any rule except the security review rule may be relaxed **locally** with an
inline `#[allow]` + comment, or **globally** only through a change to this
file plus the lint threshold in `Cargo.toml` in the same commit, explaining
the why in detail. CI for a push that weakens `make check` without touching
this file is a red build by design.

## UI feedback rules

- No hover tooltips in the window. Controls explain themselves through their
  label, icon, and disabled state; a tooltip that only repeats the label is
  noise. (The tray's `set_tooltip` is the exception: a menu-bar item has no
  window surface, so its tooltip is the only place its status text can live.)
- No success or info banner strips. Operations that succeed are visible in
  the state they change (the connection card, the profile row, the LATENCY
  column); a green strip saying "Connected to X" on top of a card that already
  says "Connected" is redundant. Errors and warnings keep the notice strip,
  because a failure is not otherwise visible in the UI.
- No desktop notifications for routine state changes (switch, stop, reload).
  They are reserved for failures the user cannot see while the window is
  hidden (an unexpected service drop, an activation failure).
