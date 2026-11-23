#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: uninstall-macos.sh must run on macOS" >&2
    exit 1
fi

PURGE_PROFILES=0
if [[ "${1:-}" == "--purge" ]]; then
    PURGE_PROFILES=1
fi

echo "==> Stopping sing-box LaunchDaemon..."
sudo launchctl bootout system/app.lexiflix.singswitch-daemon 2>/dev/null || true

echo "==> Removing application, CLI, LaunchDaemon, and sudoers rules..."
sudo rm -rf /Applications/SingSwitch.app
sudo rm -f /usr/local/bin/singswitch
sudo rm -rf /usr/local/libexec/singswitch
sudo rm -f /Library/LaunchDaemons/app.lexiflix.singswitch-daemon.plist
sudo rm -f /etc/sudoers.d/singswitch
sudo rm -f /etc/newsyslog.d/singswitch.conf

# Enable-at-login writes a per-user LaunchAgent. Under sudo, HOME is root's, so
# resolve the console user's home instead of trusting $HOME.
TARGET_USER="${SUDO_USER:-$USER}"
TARGET_HOME="$(eval echo ~"${TARGET_USER}")"
AGENT_PLIST="${TARGET_HOME}/Library/LaunchAgents/app.lexiflix.singswitch.plist"
if [[ -f "$AGENT_PLIST" ]]; then
    # bootout has to run in the user's GUI domain, which needs the user's uid,
    # not the uid of the root shell running this script.
    TARGET_UID="$(id -u "$TARGET_USER")"
    launchctl bootout "gui/${TARGET_UID}/app.lexiflix.singswitch" 2>/dev/null || true
    rm -f "$AGENT_PLIST"
fi

if [[ "$PURGE_PROFILES" -eq 1 ]]; then
    echo "==> Removing /usr/local/etc/sing-box and /var/log/sing-box..."
    sudo rm -rf /usr/local/etc/sing-box /var/log/sing-box
else
    echo "==> Keeping /usr/local/etc/sing-box/profiles (pass --purge to remove)."
    # The installer handed this tree to the console user. With singswitch gone, that
    # ownership no longer comes from a validated install, so give it back to root.
    if [[ -d /usr/local/etc/sing-box ]]; then
        sudo chown -R root:wheel /usr/local/etc/sing-box
    fi
fi

echo "==> Uninstall complete."
