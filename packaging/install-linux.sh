#!/bin/sh
set -eu
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
install_dir="$data_home/host-agent"
mkdir -p "$install_dir" "$data_home/applications" "$data_home/icons/hicolor/scalable/apps"
# Replace atomically: a running process can finish using the previous executable.
install -m 755 "$source_dir/sidekick" "$install_dir/sidekick.new"
mv -f "$install_dir/sidekick.new" "$install_dir/sidekick"
rm -f "$install_dir/host-agent"
install -m 644 "$source_dir/host-agent.svg" "$data_home/icons/hicolor/scalable/apps/host-agent.svg"
install -m 755 "$source_dir/uninstall.sh" "$install_dir/uninstall.sh"
# Exec is parsed twice: desktop string unescaping, then quoted argument parsing.
exec_path=$(printf '%s' "$install_dir/sidekick" | sed -e 's/\\/\\\\\\\\/g' -e 's/"/\\\\"/g' -e 's/`/\\\\`/g' -e 's/\$/\\\\$/g' -e 's/%/%%/g')
cat > "$data_home/applications/host-agent.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Sidekick
Comment=Local Home Assistant metrics and actions
Exec="$exec_path"
Icon=host-agent
Terminal=false
Categories=Utility;System;
EOF
printf 'Installed to %s\nLaunch Sidekick from your applications menu. Enable Start at login from its tray.\n' "$install_dir"
