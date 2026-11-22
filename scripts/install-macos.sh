#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: install-macos.sh must run on macOS" >&2
    exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_SRC="${ROOT_DIR}/dist/sbctl.app"
TARGET_USER="${SUDO_USER:-$USER}"
SING_BOX_SRC="${APP_SRC}/Contents/Resources/sing-box/sing-box"
MANAGED_BIN_DIR="/usr/local/libexec/sbctl"
SING_BOX_BIN="${MANAGED_BIN_DIR}/sing-box"

if [[ ! -d "$APP_SRC" ]]; then
    echo "error: missing ${APP_SRC}; build first with cargo build --release && ./scripts/bundle-macos.sh" >&2
    exit 1
fi
if [[ ! -x "$SING_BOX_SRC" ]]; then
    echo "error: missing sing-box inside the app bundle at ${SING_BOX_SRC}" >&2
    echo "hint: rebuild the bundle with scripts/build-mac.sh" >&2
    exit 1
fi

echo "==> Installing /Applications/sbctl.app and /usr/local/bin/sbctl..."
sudo rm -rf /Applications/sbctl.app
sudo cp -R "$APP_SRC" /Applications/sbctl.app
sudo install -d -m 755 /usr/local/bin
sudo install -m 755 /Applications/sbctl.app/Contents/MacOS/sbctl /usr/local/bin/sbctl

echo "==> Installing the managed sing-box binary..."
sudo install -d -m 755 -o root -g wheel "$MANAGED_BIN_DIR"
sudo install -m 755 -o root -g wheel "$SING_BOX_SRC" "$SING_BOX_BIN"

echo "==> Creating sing-box configuration and log directories..."
sudo install -d -m 755 /usr/local/etc/sing-box/profiles
sudo chown -R "${TARGET_USER}":staff /usr/local/etc/sing-box
sudo install -d -m 755 -o root -g wheel /var/log/sing-box
sudo touch /var/log/sing-box/error.log /var/log/sing-box/access.log
sudo chown root:wheel /var/log/sing-box/error.log /var/log/sing-box/access.log
sudo chmod 644 /var/log/sing-box/error.log /var/log/sing-box/access.log

# Rotate the service logs. sing-box writes at info level with no output cap, so
# without this they grow without bound for as long as the daemon runs.
NEWSYSLOG=/etc/newsyslog.d/sbctl.conf
sudo tee "$NEWSYSLOG" >/dev/null <<'NEWSYSLOG_EOF'
# Managed by sbctl. sing-box service logs, rotated weekly or at 10 MB, 5 kept.
/var/log/sing-box/error.log   root:wheel  644  5  10240  *  J
/var/log/sing-box/access.log  root:wheel  644  5  10240  *  J
NEWSYSLOG_EOF
sudo chmod 644 "$NEWSYSLOG"

echo "==> Installing LaunchDaemon /Library/LaunchDaemons/app.lexiflix.singbox.plist..."
PLIST_TMP="$(mktemp)"
cat > "$PLIST_TMP" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>app.lexiflix.singbox</string>
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
sudo install -m 644 -o root -g wheel "$PLIST_TMP" /Library/LaunchDaemons/app.lexiflix.singbox.plist
rm -f "$PLIST_TMP"

echo "==> Installing scoped sudoers rules (visudo checked)..."
SUDOERS_TMP="$(mktemp)"
/usr/local/bin/sbctl print-sudoers --user "$TARGET_USER" > "$SUDOERS_TMP"
chmod 0440 "$SUDOERS_TMP"
visudo -cf "$SUDOERS_TMP"
sudo install -m 0440 -o root -g wheel "$SUDOERS_TMP" /etc/sudoers.d/sbctl
rm -f "$SUDOERS_TMP"

echo "==> Registering the LaunchDaemon..."
PLIST_PATH="/Library/LaunchDaemons/app.lexiflix.singbox.plist"
# Writing the plist does not register the service; without this the daemon is
# never loaded and re-running the installer cannot fix anything.
sudo launchctl bootout system/app.lexiflix.singbox >/dev/null 2>&1 || true
sudo launchctl bootstrap system "$PLIST_PATH"

echo "==> Done. Run 'sbctl doctor' or open /Applications/sbctl.app."
