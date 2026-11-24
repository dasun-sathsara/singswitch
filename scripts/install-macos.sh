#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: install-macos.sh must run on macOS" >&2
    exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_SRC="${ROOT_DIR}/dist/SingSwitch.app"
# $USER is unset in some sudo configurations, and `set -u` would abort on it;
# ask the kernel who we are instead.
TARGET_USER="${SUDO_USER:-$(id -un)}"
SING_BOX_SRC="${APP_SRC}/Contents/Resources/sing-box/sing-box"
MANAGED_BIN_DIR="/usr/local/libexec/singswitch"
SING_BOX_BIN="${MANAGED_BIN_DIR}/sing-box"

# Every privileged write below names this account, so reject a bogus value now
# rather than after half the install has landed.
if ! id -u "$TARGET_USER" >/dev/null 2>&1; then
    echo "error: ${TARGET_USER} (from SUDO_USER/USER) is not a local account" >&2
    echo "hint: run this installer with sudo from the account that will use SingSwitch" >&2
    exit 1
fi

if [[ ! -d "$APP_SRC" ]]; then
    echo "error: missing ${APP_SRC}; build first with cargo build --release && ./scripts/bundle-macos.sh" >&2
    exit 1
fi
if [[ ! -x "$SING_BOX_SRC" ]]; then
    echo "error: missing sing-box inside the app bundle at ${SING_BOX_SRC}" >&2
    echo "hint: rebuild the bundle with scripts/build-mac.sh" >&2
    exit 1
fi

# `sudo install` follows symlinks, so a link planted in dist/ would have root
# copy whatever it points at into a root-owned, world-readable path. Require the
# real file the bundler wrote. Rerun if the bundler ever switches to a symlink.
if [[ -L "$SING_BOX_SRC" ]]; then
    echo "error: ${SING_BOX_SRC} is a symlink; refusing to install it as root" >&2
    exit 1
fi

# Run it as the invoking user, never as root: this proves the binary executes
# before any privileged write, and an unverified binary never runs elevated.
if ! SING_BOX_VERSION_OUT="$("$SING_BOX_SRC" version 2>&1)"; then
    echo "error: ${SING_BOX_SRC} does not run" >&2
    echo "  ${SING_BOX_VERSION_OUT}" >&2
    exit 1
fi

echo "==> Installing /Applications/SingSwitch.app and /usr/local/bin/singswitch..."
# Replace, never overlay: `cp -R` onto an existing bundle keeps files the new
# build no longer ships. Stage the copy and swap it in, so a failed copy leaves
# the previous install intact instead of no install at all.
APP_STAGE="/Applications/.SingSwitch.app.staged"
sudo rm -rf "$APP_STAGE"
sudo cp -R "$APP_SRC" "$APP_STAGE"
sudo rm -rf /Applications/SingSwitch.app
sudo mv "$APP_STAGE" /Applications/SingSwitch.app
sudo install -d -m 755 /usr/local/bin
sudo install -m 755 /Applications/SingSwitch.app/Contents/MacOS/singswitch /usr/local/bin/singswitch

# The daemon may still be running the binary installed below. Unload it before
# touching that file and register it again at the end; unloading a service that
# was never loaded is the normal case on a first install. RunAtLoad is false, so
# loading it again starts nothing by itself.
sudo launchctl bootout system/app.lexiflix.singswitch-daemon >/dev/null 2>&1 || true

echo "==> Installing the managed sing-box binary..."
sudo install -d -m 755 -o root -g wheel "$MANAGED_BIN_DIR"
sudo install -m 755 -o root -g wheel "$SING_BOX_SRC" "$SING_BOX_BIN"

# Prove the root-owned copy is executable by the account that will run it. A
# checksum only says the bytes match the archive; this says the install works.
if ! sudo -u "$TARGET_USER" "$SING_BOX_BIN" version >/dev/null 2>&1; then
    echo "error: ${SING_BOX_BIN} is not runnable by ${TARGET_USER}" >&2
    exit 1
fi

echo "==> Creating sing-box configuration and log directories..."
sudo install -d -m 755 /usr/local/etc/sing-box/profiles
sudo chown -R "${TARGET_USER}":staff /usr/local/etc/sing-box
sudo install -d -m 755 -o root -g wheel /var/log/sing-box
sudo touch /var/log/sing-box/error.log /var/log/sing-box/access.log
sudo chown root:wheel /var/log/sing-box/error.log /var/log/sing-box/access.log
sudo chmod 644 /var/log/sing-box/error.log /var/log/sing-box/access.log

# Rotate the service logs. sing-box writes at info level with no output cap, so
# without this they grow without bound for as long as the daemon runs.
NEWSYSLOG=/etc/newsyslog.d/singswitch.conf
sudo tee "$NEWSYSLOG" >/dev/null <<'NEWSYSLOG_EOF'
# Managed by singswitch. sing-box service logs, rotated weekly or at 10 MB, 5 kept.
/var/log/sing-box/error.log   root:wheel  644  5  10240  *  J
/var/log/sing-box/access.log  root:wheel  644  5  10240  *  J
NEWSYSLOG_EOF
sudo chmod 644 "$NEWSYSLOG"

echo "==> Installing LaunchDaemon /Library/LaunchDaemons/app.lexiflix.singswitch-daemon.plist..."
PLIST_TMP="$(mktemp)"
cat > "$PLIST_TMP" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>app.lexiflix.singswitch-daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>${SING_BOX_BIN}</string>
        <string>run</string>
        <string>-c</string>
        <string>/usr/local/etc/sing-box/config.json</string>
    </array>
    <key>RunAtLoad</key>
    <false/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>/var/log/sing-box/access.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/sing-box/error.log</string>
</dict>
</plist>
PLIST
sudo install -m 644 -o root -g wheel "$PLIST_TMP" /Library/LaunchDaemons/app.lexiflix.singswitch-daemon.plist
rm -f "$PLIST_TMP"

echo "==> Installing scoped sudoers rules (visudo checked)..."
SUDOERS_TMP="$(mktemp)"
/usr/local/bin/singswitch print-sudoers --user "$TARGET_USER" > "$SUDOERS_TMP"
chmod 0440 "$SUDOERS_TMP"
visudo -cf "$SUDOERS_TMP"
# Re-check the root-owned copy that actually lands: the bytes `visudo` approved
# above still live in a file the invoking user owns, so they can change between
# the check and the install. sudoers skips dotted names, so the staging file is
# inert while it sits there, and the final rename is atomic.
SUDOERS_STAGE="/etc/sudoers.d/.singswitch.staged"
sudo install -m 0440 -o root -g wheel "$SUDOERS_TMP" "$SUDOERS_STAGE"
rm -f "$SUDOERS_TMP"
sudo visudo -cf "$SUDOERS_STAGE"
sudo mv -f "$SUDOERS_STAGE" /etc/sudoers.d/singswitch

echo "==> Registering the LaunchDaemon..."
PLIST_PATH="/Library/LaunchDaemons/app.lexiflix.singswitch-daemon.plist"
# Writing the plist does not register the service; without this the daemon is
# never loaded and re-running the installer cannot fix anything.
sudo launchctl bootstrap system "$PLIST_PATH"

echo "==> Done. Open /Applications/SingSwitch.app."
