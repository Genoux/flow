#!/usr/bin/env bash
# Build Flow.app and install it for this user. Reached through
# packaging/install.sh on macOS; the flags are the same.
#
# Everything lands under $HOME - no sudo.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
build_channel="$(cat "$repo/packaging/channel")"
channel="$build_channel"
activate=true
restart=true
models=false
while [ "$#" -gt 0 ]; do
  case "$1" in
    --channel) channel="${2:?--channel needs a value}"; shift 2 ;;
    --no-activate) activate=false; shift ;;
    --no-restart) restart=false; shift ;;
    --models) models=true; shift ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done
if [ "$channel" != "$build_channel" ]; then
  echo "This is a $build_channel build; it cannot replace $channel." >&2
  exit 1
fi
case "$channel" in
  stable | experimental) ;;
  *)
    echo "unknown channel $channel - stable or experimental" >&2
    exit 1
    ;;
esac

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
apps="$HOME/Applications"
app="$apps/Flow.app"
agents="$HOME/Library/LaunchAgents"
label=com.genoux.flow
tray_label=com.genoux.flow.tray
# ponytail: launchd appends here forever and nothing rotates it; a few lines per
# dictation makes that years of headroom. newsyslog.d can rotate it if not.
log="$HOME/Library/Logs/Flow/flow.log"
domain="gui/$(id -u)"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# Same reason as the Linux script: build.target-dir can put binaries anywhere.
target_dir() {
  cargo metadata --format-version 1 --no-deps --manifest-path "$1" |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'
}

# bootstrap exits 5 for a label disabled for login, which is how the console's
# Launch at login switch is stored. Enabled only for the instant of loading, so
# that choice survives.
load_agent() {
  local agent="$1" disabled=false status=0
  [[ "$(launchctl print-disabled "$domain")" != *"\"$agent\" => disabled"* ]] || disabled=true
  if "$disabled"; then launchctl enable "$domain/$agent"; fi
  launchctl bootstrap "$domain" "$agents/$agent.plist" || status=$?
  if "$disabled"; then launchctl disable "$domain/$agent"; fi
  return "$status"
}

# bootout then bootstrap is also what restarts a loaded agent onto the new
# build.
reload_agent() {
  local agent="$1"
  launchctl bootout "$domain/$agent" 2>/dev/null || true
  # bootout returns before the job is gone, and bootstrapping the label
  # while it lingers fails with EIO.
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    launchctl print "$domain/$agent" >/dev/null 2>&1 || break
    sleep 0.5
  done
  load_agent "$agent"
}

mkdir -p "$apps" "$bin_dir" "$agents" "$(dirname "$log")"
# Staged beside the destination so the swap below is a rename on one filesystem.
stage="$(mktemp -d "$apps/.flow-install.XXXXXXXX")"
trap 'rm -rf "$stage"' EXIT
if [ -d "$repo/Flow.app" ]; then
  # --noqtn: a tarball fetched by a browser marks everything in it quarantined,
  # and Gatekeeper refuses a quarantined app that Apple did not sign.
  ditto --noqtn "$repo/Flow.app" "$stage/Flow.app"
else
  say "Building (this takes a few minutes the first time)"
  cargo build --release --manifest-path "$repo/Cargo.toml"
  cargo build --release --manifest-path "$repo/crates/console/Cargo.toml"
  daemon="$(target_dir "$repo/Cargo.toml")/release/flow"
  console="$(target_dir "$repo/crates/console/Cargo.toml")/release/flow-console"
  for binary in "$daemon" "$console"; do
    if [ ! -x "$binary" ]; then
      echo "built, but $binary is not there - cargo put it somewhere unexpected" >&2
      exit 1
    fi
  done
  version="$("$daemon" --version | awk '{print $2}')"
  say "Bundling and signing Flow.app"
  bash "$repo/packaging/macos/bundle.sh" "$daemon" "$console" "$version" "$stage"
fi

say "Installing Flow.app into $apps"
rm -rf "$app.old"
[ ! -e "$app" ] || mv "$app" "$app.old"
mv "$stage/Flow.app" "$app" || { [ ! -e "$app.old" ] || mv "$app.old" "$app"; exit 1; }
rm -rf "$app.old"

ln -sfn "$app/Contents/MacOS/flow" "$bin_dir/flow"

# launchd expands neither ~ nor $HOME, so the template gets absolute paths.
sed -e "s|@APP@|$app|g" -e "s|@LOG@|$log|g" \
  "$repo/packaging/macos/com.genoux.flow.plist" > "$agents/$label.plist"
plutil -lint "$agents/$label.plist" >/dev/null
sed -e "s|@APP@|$app|g" -e "s|@LOG@|$log|g" \
  "$repo/packaging/macos/$tray_label.plist" > "$agents/$tray_label.plist"
plutil -lint "$agents/$tray_label.plist" >/dev/null

if "$models"; then
  "$bin_dir/flow" install
fi

loaded=false
launchctl print "$domain/$label" >/dev/null 2>&1 && loaded=true
tray_loaded=false
launchctl print "$domain/$tray_label" >/dev/null 2>&1 && tray_loaded=true
if "$activate"; then
  # --no-restart leaves a loaded agent alone.
  if ! "$loaded" || "$restart"; then
    say "Starting the daemon"
    reload_agent "$label"
  fi
  # The tray is its own agent, so the menu bar item outlives the daemon.
  if ! "$tray_loaded" || "$restart"; then
    say "Starting the menu bar item"
    reload_agent "$tray_label"
  fi
elif "$restart" && "$loaded"; then
  say "Restarting the running daemon onto the new build"
  launchctl kickstart -k "$domain/$label"
  if "$tray_loaded"; then launchctl kickstart -k "$domain/$tray_label"; fi
fi

say "Done"
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf '\n\033[1;33m%s\033[0m\n' "Warning: $bin_dir is not on your PATH - \`flow\` will not be found."
     ;;
esac
cat <<EOF
Open Flow from $apps to finish setting up.

macOS will ask for three permissions the first time each is needed:
Microphone, Input Monitoring and Accessibility. They are tied to Flow's
signing certificate, so they persist across updates.

Logs:  flow logs
EOF
