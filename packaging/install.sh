#!/usr/bin/env bash
# Build Flow and put it where a desktop session can find it.
#
# Everything lands under $HOME - no sudo, nothing outside the user's own
# directories - except the udev rule, which cannot live there and is the one
# step this script tells you to run yourself rather than doing behind your
# back.
set -euo pipefail

# Flow is Linux-only and the daemon cannot even be compiled elsewhere: it opens
# /dev/input and /dev/uinput, talks Wayland, and links a Vulkan llama.cpp. Run
# on a Mac this used to spend two minutes reaching cmake and then failing inside
# llama.cpp's Vulkan backend, which reads as a missing dependency rather than as
# the wrong machine. The console alone does build here, but installing it
# without a daemon or a systemd unit would be installing a window onto nothing.
if [ "$(uname -s)" != "Linux" ]; then
  echo "Flow is Linux-only - this is $(uname -s)." >&2
  echo "The daemon needs /dev/uinput, Wayland and a Vulkan llama.cpp, none of" >&2
  echo "which exist here. Run this on the machine you dictate on." >&2
  exit 1
fi

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
# Rewritten to the absolute path of the icon just installed, rather than left
# as a theme name. A name is resolved through the user's icon theme, so the
# launcher shows Flow's own icon on a desktop whose theme happens to carry that
# name and a blank tile on one that does not - and ~/.local/share/icons/hicolor
# has no index.theme, so `flow-console` as a bare name is skipped too. A path is
# read straight off disk by every loader.
sed -i "s|^Icon=.*|Icon=$icons/flow-console.png|" "$apps/flow-console.desktop"
systemctl --user daemon-reload

# Without these the launcher shows the entry only after the next login, which
# reads as the install having silently failed. Both are optional tools and
# neither is fatal: the caches are a speed-up, not the source of truth.
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null &&
  gtk-update-icon-cache -qtf "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" 2>/dev/null || true

# Models are deliberately NOT fetched here. They are ~3 GB, and a terminal that
# sits on a progress bar for twenty minutes is the worst first impression this
# tool can make. Opening Flow shows a setup screen that downloads them, says
# which GPU it found while it does, and starts the daemon at the end.
#
# `flow install` still exists and still does the whole job, for a scripted or
# headless install that wants it: run it yourself, or pass --models here.
if [ "${1:-}" = "--models" ]; then
  shift
  say "Fetching models"
  "$bin_dir/flow" install "$@"
fi

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
# The one instruction that matters is first and on its own. Everything under
# it is for later; the models are not downloaded yet, so anything that suggests
# starting the daemon before opening the window would only start a daemon with
# nothing to load.
cat <<EOF
Open Flow to finish setting up - it downloads the models and starts the daemon.

  flow-console          or "Flow" in your launcher

Start it at login:    systemctl --user enable flow.service
Everything else:      flow help
EOF
