#!/usr/bin/env bash
# Remove what install.sh put in place.
#
# Deliberately does NOT touch your config, vocabulary or history.
# Those are yours and they survive a reinstall; deleting them is a separate
# decision and this script prints how rather than making it for you.
set -euo pipefail

if [ "$(uname -s)" = Darwin ]; then
  exec bash "$(dirname "${BASH_SOURCE[0]}")/macos/uninstall.sh" "$@"
fi

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
config="${XDG_CONFIG_HOME:-$HOME/.config}/offhand"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

say "Stopping the daemon"
systemctl --user disable --now offhand.service 2>/dev/null || true
systemctl --user disable --now offhand-tray.service 2>/dev/null || true
# What an install left behind from when this was called Flow.
systemctl --user disable --now flow.service flow-tray.service 2>/dev/null || true

say "Removing binaries, service, desktop entry and icon"
rm -fv "$bin_dir/offhand" "$bin_dir/offhand-console" \
       "$bin_dir/offhand-stable" "$bin_dir/offhand-console-stable" \
       "$bin_dir/offhand-experimental" "$bin_dir/offhand-console-experimental" \
       "$units/offhand.service" \
       "$units/offhand-tray.service" \
       "$data/applications/offhand-console.desktop" \
       "$data/icons/hicolor/512x512/apps/offhand-console.png" \
       "$bin_dir/flow" "$bin_dir/flow-console" \
       "$bin_dir/flow-stable" "$bin_dir/flow-console-stable" \
       "$bin_dir/flow-experimental" "$bin_dir/flow-console-experimental" \
       "$units/flow.service" \
       "$units/flow-tray.service" \
       "$data/applications/flow-console.desktop" \
       "$data/icons/hicolor/512x512/apps/flow-console.png"

systemctl --user daemon-reload
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" 2>/dev/null || true

say "Done"
cat <<EOF
Kept, because they are yours:

  $config              config.toml and vocabulary.txt
  $data/offhand           history and recordings

Remove those too with:

  rm -rf "$config" "$data/offhand"

The udev rule and your 'input' group membership were installed with sudo and
are left alone - other tools may rely on both:

  sudo rm /etc/udev/rules.d/99-offhand-uinput.rules
EOF
