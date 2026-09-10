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
  echo "The daemon opens /dev/uinput and talks Wayland, neither of which" >&2
  echo "exists here. Run this on the machine you dictate on." >&2
  exit 1
fi

# Which build this is. Both live side by side under their own names and a
# symlink decides which one runs, so switching is repointing a link rather than
# reinstalling - and going back is possible because stable never left the disk.
channel=stable
if [ "${1:-}" = "--channel" ]; then
  channel="$2"
  shift 2
fi
case "$channel" in
  stable | experimental) ;;
  *)
    echo "unknown channel $channel - stable or experimental" >&2
    exit 1
    ;;
esac

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# A release tarball ships the binaries already built, under bin/. A git
# checkout does not. Same script for both rather than a second install path
# that drifts from this one.
# Asked rather than assumed: `build.target-dir` in a cargo config puts the
# binaries somewhere else entirely, and a shared target dir is a common setup.
# Guessing $repo/target failed the install after a successful build.
target_dir() {
  cargo metadata --format-version 1 --no-deps --manifest-path "$1" |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'
}

if [ -x "$repo/bin/flow" ]; then
  daemon="$repo/bin/flow"
  console="$repo/bin/flow-console"
else
  say "Building (this takes a few minutes the first time)"
  cargo build --release --manifest-path "$repo/Cargo.toml"
  cargo build --release --manifest-path "$repo/crates/console/Cargo.toml"
  daemon="$(target_dir "$repo/Cargo.toml")/release/flow"
  console="$(target_dir "$repo/crates/console/Cargo.toml")/release/flow-console"
fi

for binary in "$daemon" "$console"; do
  if [ ! -x "$binary" ]; then
    echo "built, but $binary is not there - cargo put it somewhere unexpected" >&2
    exit 1
  fi
done

say "Installing the $channel build into $bin_dir"
mkdir -p "$bin_dir"
install -m755 "$daemon" "$bin_dir/flow-$channel"
install -m755 "$console" "$bin_dir/flow-console-$channel"

# The service runs `flow`, never `flow-stable`, so the unit file never has to
# know which channel is live. Older installs put a real binary at this path;
# ln -sfn will not replace a regular file, so it goes first.
for name in flow flow-console; do
  link="$bin_dir/$name"
  if [ -e "$link" ] && [ ! -L "$link" ]; then
    rm -f "$link"
  fi
  # Only claim the link if nothing has it yet, or if it already points at this
  # channel. Reinstalling stable must not drag someone off experimental.
  current="$(readlink "$link" 2>/dev/null || true)"
  if [ -z "$current" ] || [ "$current" = "$name-$channel" ]; then
    ln -sfn "$name-$channel" "$link"
  else
    echo "left $name pointing at $current - switch channels in Settings"
  fi
done

say "Installing the service, desktop entry and icon"
mkdir -p "$units" "$apps" "$icons"
install -m644 "$repo/packaging/flow.service" "$units/flow.service"
install -m644 "$repo/packaging/flow-tray.service" "$units/flow-tray.service"
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
# The tray is a lightweight controller and recovery path, not the dictation
# engine. Keep it available at login even when Flow itself is stopped; its own
# config decides whether an icon is published.
systemctl --user enable --now flow-tray.service

# Without these the launcher shows the entry only after the next login, which
# reads as the install having silently failed. Both are optional tools and
# neither is fatal: the caches are a speed-up, not the source of truth.
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null &&
  gtk-update-icon-cache -qtf "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" 2>/dev/null || true

# Seeds the config templates. There are no models to fetch any more:
# transcription and refining are OpenRouter requests, and the key that pays for
# them is typed into the console's Settings screen.
say "Seeding config"
"$bin_dir/flow-$channel" install

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
if systemctl --user is-active --quiet flow-tray.service; then
  say "Restarting the tray onto the new build"
  systemctl --user restart flow-tray.service
fi

say "Done"
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf '\n\033[1;33m%s\033[0m\n' "Warning: $bin_dir is not on your PATH - \`flow\` will not be found."
     echo "  Add it in your shell's rc file, or the desktop entry will work and the terminal will not."
     ;;
esac
# The one instruction that matters is first and on its own. Everything under
# it is for later; the key is entered in the console before the daemon starts.
cat <<EOF
Open Flow to finish setting up - add your OpenRouter key and start the daemon.

  flow-console          or "Flow" in your launcher

Start it at login:    systemctl --user enable flow.service
Everything else:      flow help
EOF
