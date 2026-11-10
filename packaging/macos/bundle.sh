#!/usr/bin/env bash
# Assemble and sign Offhand.app: bundle.sh <offhand> <offhand-console> <version> <out-dir>
#
# Signing input is a self-signed .p12 (OFFHAND_SIGNING_P12 and
# OFFHAND_SIGNING_PASSWORD_FILE, defaulting to ~/.config/offhand-signing/). The
# point of a stable certificate over ad-hoc is the designated requirement:
# ad-hoc signatures are keyed on the code hash, so macOS drops the
# Microphone/Input Monitoring/Accessibility grants on every rebuild, while
# `identifier ... and certificate root = H"..."` is the same for every build.
set -euo pipefail

[ "$#" -eq 4 ] || { echo "usage: bundle.sh <offhand> <offhand-console> <version> <out-dir>" >&2; exit 1; }
daemon="$1" console="$2" version="$3" out="$4"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$out/Offhand.app"
agent="$app/Contents/Helpers/Offhand Agent.app"

# apple-codesign 0.29.0, pinned. Apple's `codesign` refuses untrusted
# self-signed identities; rcodesign signs straight from the .p12 with no keychain.
rcodesign_url="https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F0.29.0/apple-codesign-0.29.0-macos-universal.tar.gz"
rcodesign_sha256=d98372d5524226ccf9dc0eda03d4e4f5826182dabb2fc3f2bd303ed9113a748d

p12="${OFFHAND_SIGNING_P12:-$HOME/.config/offhand-signing/offhand-signing.p12}"
password_file="${OFFHAND_SIGNING_PASSWORD_FILE:-$HOME/.config/offhand-signing/p12.pass}"

fetch_rcodesign() {
  local cache="${XDG_CACHE_HOME:-$HOME/Library/Caches}/offhand-build"
  local tool="$cache/rcodesign-$rcodesign_sha256"
  # The directory name carries the checksum of the archive it was unpacked
  # from, so its existence is the verification.
  if [ ! -x "$tool/rcodesign" ]; then
    mkdir -p "$cache"
    local scratch
    scratch="$(mktemp -d "$cache/.fetch.XXXXXXXX")"
    curl -fsSL "$rcodesign_url" -o "$scratch/rcodesign.tar.gz"
    echo "$rcodesign_sha256  $scratch/rcodesign.tar.gz" | shasum -a 256 -c - >&2
    tar -xzf "$scratch/rcodesign.tar.gz" -C "$scratch" --strip-components 1
    rm -f "$scratch/rcodesign.tar.gz"
    rm -rf "$tool"
    mv "$scratch" "$tool"
  fi
  echo "$tool/rcodesign"
}

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" \
  "$agent/Contents/MacOS" "$agent/Contents/Resources"
# CFBundleVersion must be dot-separated integers, so a prerelease tag is cut.
sed -e "s|@VERSION@|$version|g" -e "s|@BUILD@|${version%%-*}|g" \
  "$here/Info.plist" > "$app/Contents/Info.plist"
sed -e "s|@VERSION@|$version|g" -e "s|@BUILD@|${version%%-*}|g" \
  "$here/AgentInfo.plist" > "$agent/Contents/Info.plist"
# The daemon, tray and CLI are a bundle of their own so that Offhand.app's
# identity belongs to the console alone; see offhand_paths::AGENT_PROGRAM.
install -m755 "$daemon" "$agent/Contents/MacOS/offhand"
install -m755 "$console" "$app/Contents/MacOS/offhand-console"
# Which build this app is. There is one Offhand.app, so the channel is a property
# of the bundle and not of a link beside it as on Linux; the console reads it
# from here, and the signature seals it so it cannot drift from the binaries.
install -m644 "$here/../channel" "$app/Contents/Resources/channel"

iconset="$(mktemp -d)/offhand.iconset"
trap 'rm -rf "$(dirname "$iconset")"' EXIT
mkdir "$iconset"
# Not the Linux icon: that tile fills its canvas, and macOS draws an icon at
# the size of the canvas, so it would sit larger than every app beside it. This
# one is the same tile on Apple's grid (824 of 1024), and 1024 is the largest
# size the iconset asks for.
source_png="$here/offhand.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$source_png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$source_png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/offhand.icns"
cp "$app/Contents/Resources/offhand.icns" "$agent/Contents/Resources/offhand.icns"

if [ -f "$p12" ] && [ -f "$password_file" ]; then
  rcodesign="$(fetch_rcodesign)"
  # The outer pass also re-signs nested bundles. Signing the helper first is
  # what gives it its identity from its own Info.plist, and `signed_as` below
  # is what guarantees it. The notification centre refuses a signing
  # identifier that differs from the bundle id.
  for bundle in "$agent" "$app"; do
    if ! output="$("$rcodesign" sign --p12-file "$p12" --p12-password-file "$password_file" "$bundle" 2>&1)"; then
      echo "$output" >&2
      exit 1
    fi
  done
else
  codesign --force --deep --sign - "$app"
  printf '\033[1;33m%s\033[0m\n' "No signing certificate at $p12 - signed ad-hoc, so macOS permissions must be re-granted after every rebuild."
fi

codesign --verify --deep --strict "$app"
# sed reads to the end: `grep -q` stops at the match, codesign dies of SIGPIPE,
# and pipefail turns a correct signature into a failure.
signed_as() {
  local identifier
  identifier="$(codesign -dv "$1" 2>&1 | sed -n 's/^Identifier=//p')"
  [ "$identifier" = "$2" ] || {
    echo "$1 was signed as '$identifier', not $2" >&2
    exit 1
  }
}
signed_as "$agent" com.genoux.offhand.agent
signed_as "$app" com.genoux.offhand
