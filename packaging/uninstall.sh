#!/usr/bin/env bash
# Remove what install.sh put in place.
#
# Deliberately does NOT touch your config, vocabulary, history or the models.
# Those are yours and they survive a reinstall; deleting them is a separate
# decision and this script prints how rather than making it for you.
set -euo pipefail

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
config="${XDG_CONFIG_HOME:-$HOME/.config}/flow"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

say "Stopping the daemon"
systemctl --user disable --now flow.service 2>/dev/null || true

say "Removing binaries, service, desktop entry and icon"
rm -fv "$bin_dir/flow" "$bin_dir/flow-console" \
       "$units/flow.service" \
       "$data/applications/flow-console.desktop" \
       "$data/icons/hicolor/512x512/apps/flow-console.png"

systemctl --user daemon-reload
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" 2>/dev/null || true

say "Done"
cat <<EOF
Kept, because they are yours:

  $config              config.toml and vocabulary.txt
  $data/flow           models (~3 GB), history and recordings

Remove those too with:

  rm -rf "$config" "$data/flow"

The udev rule and your 'input' group membership were installed with sudo and
are left alone - other tools may rely on both:

  sudo rm /etc/udev/rules.d/99-flow-uinput.rules
EOF
