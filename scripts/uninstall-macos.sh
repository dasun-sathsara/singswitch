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

AGENT_PLIST="${HOME}/Library/LaunchAgents/app.lexiflix.singswitch.plist"
if [[ -f "$AGENT_PLIST" ]]; then
    launchctl bootout "gui/$(id -u)/app.lexiflix.singswitch" 2>/dev/null || true
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
