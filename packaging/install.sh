#!/usr/bin/env bash
# Build Flow and put it where a desktop session can find it.
#
# Everything lands under $HOME - no sudo, nothing outside the user's own
# directories - except the udev rule, which cannot live there and is the one
# step this script tells you to run yourself rather than doing behind your
# back.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# A release tarball ships the binaries already built, under bin/. A git
# checkout does not. Same script for both rather than a second install path
# that drifts from this one.
if [ -x "$repo/bin/flow" ]; then
  daemon="$repo/bin/flow"
  console="$repo/bin/flow-console"
else
  say "Building (this takes a few minutes the first time)"
  cargo build --release --manifest-path "$repo/Cargo.toml"
  cargo build --release --manifest-path "$repo/crates/console/Cargo.toml"
  daemon="$repo/target/release/flow"
  console="$repo/crates/console/target/release/flow-console"
fi

say "Installing binaries into $bin_dir"
mkdir -p "$bin_dir"
install -m755 "$daemon" "$bin_dir/flow"
install -m755 "$console" "$bin_dir/flow-console"

say "Installing the service and desktop entry"
mkdir -p "$units" "$apps"
install -m644 "$repo/packaging/flow.service" "$units/flow.service"
install -m644 "$repo/packaging/flow-console.desktop" "$apps/flow-console.desktop"
systemctl --user daemon-reload

# Models, config and vocabulary. Run through the freshly installed binary so a
# stale one in the repo cannot be what seeds the config.
say "Fetching models"
"$bin_dir/flow" install "$@"

rule=/etc/udev/rules.d/99-flow-uinput.rules
if [ ! -e "$rule" ]; then
  say "One step left, and it needs root"
  cat <<EOF
Flow types by opening /dev/uinput, which is not writable by your user by
default. Install the rule and reload it:

  echo 'KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"' \\
    | sudo tee $rule
  sudo udevadm control --reload-rules && sudo udevadm trigger
  sudo usermod -aG input "$USER"

Then log out and back in so the group takes effect.
EOF
fi

say "Done"
cat <<EOF
Start it now:        systemctl --user start flow.service
Start it at login:   systemctl --user enable flow.service
Settings and history: flow-console  (or "Flow" in your launcher)

$bin_dir must be on your PATH for those to work.
EOF
