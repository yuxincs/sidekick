#!/bin/sh
set -eu
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
# Only this application's known files are removed. User config is retained.
rm -f "$config_home/autostart/dev.HostAgent.HostAgent.desktop"
rm -f "$data_home/applications/host-agent.desktop"
rm -f "$data_home/icons/hicolor/scalable/apps/host-agent.svg"
rm -f "$data_home/host-agent/sidekick" "$data_home/host-agent/host-agent" "$data_home/host-agent/uninstall.sh"
rmdir "$data_home/host-agent" 2>/dev/null || true
printf 'Sidekick removed. Pairing identity and exposure settings were retained.\n'
