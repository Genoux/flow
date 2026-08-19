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
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"

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

say "Installing the service, desktop entry and icon"
mkdir -p "$units" "$apps" "$icons"
install -m644 "$repo/packaging/flow.service" "$units/flow.service"
install -m644 "$repo/packaging/flow-console.desktop" "$apps/flow-console.desktop"
install -m644 "$repo/packaging/flow-console.png" "$icons/flow-console.png"
systemctl --user daemon-reload

# Without these the launcher shows the entry only after the next login, which
# reads as the install having silently failed. Both are optional tools and
# neither is fatal: the caches are a speed-up, not the source of truth.
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null &&
  gtk-update-icon-cache -qtf "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" 2>/dev/null || true

# Models, config and vocabulary. Run through the freshly installed binary so a
# stale one in the repo cannot be what seeds the config.
say "Fetching models"
"$bin_dir/flow" install "$@"

# The question is whether this user can open /dev/uinput, not whether our rule
# file exists. Many setups already grant it - a logind uaccess ACL, an existing
# input-group membership, or a rule some other tool installed. Checking for the
# file demanded three sudo commands from people who needed none of them.
rule=/etc/udev/rules.d/99-flow-uinput.rules
if [ ! -w /dev/uinput ]; then
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

# An update that leaves the old process running is not an update. Only when it
# is already up: starting a daemon nobody asked for is the installer making a
# decision that belongs to the user.
if systemctl --user is-active --quiet flow.service; then
  say "Restarting the running daemon onto the new build"
  systemctl --user restart flow.service
fi

say "Done"
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf '\n\033[1;33m%s\033[0m\n' "Warning: $bin_dir is not on your PATH - \`flow\` will not be found."
     echo "  Add it in your shell's rc file, or the desktop entry will work and the terminal will not."
     ;;
esac
cat <<EOF
Start it now:         systemctl --user start flow.service
Start it at login:    systemctl --user enable flow.service
Settings and history:  flow-console  (or "Flow" in your launcher)
Everything else:       flow help
EOF
