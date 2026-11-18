#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: install-macos.sh must run on macOS" >&2
    exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_SRC="${ROOT_DIR}/dist/sbctl.app"
TARGET_USER="${SUDO_USER:-$USER}"
SING_BOX_BIN="$(command -v sing-box || echo /opt/homebrew/bin/sing-box)"

if [[ ! -d "$APP_SRC" ]]; then
    echo "error: missing ${APP_SRC}; build first with cargo build --release && ./scripts/bundle-macos.sh" >&2
    exit 1
fi
if [[ ! -x "$SING_BOX_BIN" ]]; then
    echo "error: sing-box binary not found on PATH or /opt/homebrew/bin/sing-box" >&2
    exit 1
fi

echo "==> Installing /Applications/sbctl.app and /usr/local/bin/sbctl..."
sudo rm -rf /Applications/sbctl.app
sudo cp -R "$APP_SRC" /Applications/sbctl.app
sudo install -d -m 755 /usr/local/bin
sudo install -m 755 /Applications/sbctl.app/Contents/MacOS/sbctl /usr/local/bin/sbctl

echo "==> Creating sing-box configuration and log directories..."
sudo install -d -m 755 /usr/local/etc/sing-box/profiles
sudo chown -R "${TARGET_USER}":staff /usr/local/etc/sing-box
sudo install -d -m 755 /var/log/sing-box
sudo touch /var/log/sing-box/error.log /var/log/sing-box/access.log
sudo chown -R "${TARGET_USER}":staff /var/log/sing-box

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

echo "==> Done. Run 'sbctl doctor' or open /Applications/sbctl.app."
