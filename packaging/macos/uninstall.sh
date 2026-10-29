#!/usr/bin/env bash
# Remove what macos/install.sh put in place.
#
# Like the Linux script, keeps your config, vocabulary, history and models.
set -euo pipefail

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
label=com.genoux.flow
tray_label=com.genoux.flow.tray
agent="$HOME/Library/LaunchAgents/$label.plist"
tray_agent="$HOME/Library/LaunchAgents/$tray_label.plist"
config="${XDG_CONFIG_HOME:-$HOME/.config}/flow"
data="${XDG_DATA_HOME:-$HOME/.local/share}/flow"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

say "Stopping the daemon and the menu bar item"
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootout "gui/$(id -u)/$tray_label" 2>/dev/null || true

say "Removing Flow.app, the launch agent and the flow command"
rm -rfv "$HOME/Applications/Flow.app"
rm -fv "$agent" "$tray_agent"
# Only the symlink this installer made; a hand-installed binary is not ours.
if [ -L "$bin_dir/flow" ]; then rm -fv "$bin_dir/flow"; fi

say "Done"
cat <<EOF
Kept, because they are yours:

  $config              config.toml and vocabulary.txt
  $data           history, recordings and models
  $HOME/Library/Logs/Flow

Remove those too with:

  rm -rf "$config" "$data" "$HOME/Library/Logs/Flow"

Microphone, Input Monitoring and Accessibility entries stay in System
Settings > Privacy & Security until you remove them there.
EOF
