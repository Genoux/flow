#!/usr/bin/env bash
# Assemble and sign Flow.app: bundle.sh <flow> <flow-console> <version> <out-dir>
#
# Signing input is a self-signed .p12 (FLOW_SIGNING_P12 and
# FLOW_SIGNING_PASSWORD_FILE, defaulting to ~/.config/flow-signing/). The
# point of a stable certificate over ad-hoc is the designated requirement:
# ad-hoc signatures are keyed on the code hash, so macOS drops the
# Microphone/Input Monitoring/Accessibility grants on every rebuild, while
# `identifier ... and certificate root = H"..."` is the same for every build.
set -euo pipefail

[ "$#" -eq 4 ] || { echo "usage: bundle.sh <flow> <flow-console> <version> <out-dir>" >&2; exit 1; }
daemon="$1" console="$2" version="$3" out="$4"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$out/Flow.app"
bundle_id=com.genoux.flow

# apple-codesign 0.29.0, pinned. Apple's `codesign` refuses untrusted
# self-signed identities; rcodesign signs straight from the .p12 with no keychain.
rcodesign_url="https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F0.29.0/apple-codesign-0.29.0-macos-universal.tar.gz"
rcodesign_sha256=d98372d5524226ccf9dc0eda03d4e4f5826182dabb2fc3f2bd303ed9113a748d

p12="${FLOW_SIGNING_P12:-$HOME/.config/flow-signing/flow-signing.p12}"
password_file="${FLOW_SIGNING_PASSWORD_FILE:-$HOME/.config/flow-signing/p12.pass}"

fetch_rcodesign() {
  local cache="${XDG_CACHE_HOME:-$HOME/Library/Caches}/flow-build"
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
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# CFBundleVersion must be dot-separated integers, so a prerelease tag is cut.
sed -e "s|@VERSION@|$version|g" -e "s|@BUILD@|${version%%-*}|g" \
  "$here/Info.plist" > "$app/Contents/Info.plist"
install -m755 "$daemon" "$app/Contents/MacOS/flow"
install -m755 "$console" "$app/Contents/MacOS/flow-console"

iconset="$(mktemp -d)/flow.iconset"
trap 'rm -rf "$(dirname "$iconset")"' EXIT
mkdir "$iconset"
source_png="$here/../flow-console.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$source_png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$source_png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/flow.icns"

if [ -f "$p12" ] && [ -f "$password_file" ]; then
  rcodesign="$(fetch_rcodesign)"
  # Signing the bundle re-signs every executable in it, so the daemon's
  # identifier is scoped to its path here; signed separately beforehand, it
  # came out as plain "flow". The console takes CFBundleIdentifier.
  if ! output="$("$rcodesign" sign --p12-file "$p12" --p12-password-file "$password_file" \
    --binary-identifier "Contents/MacOS/flow:$bundle_id.daemon" "$app" 2>&1)"; then
    echo "$output" >&2
    exit 1
  fi
else
  codesign --force --deep --sign - "$app"
  printf '\033[1;33m%s\033[0m\n' "No signing certificate at $p12 - signed ad-hoc, so macOS permissions must be re-granted after every rebuild."
fi

codesign --verify --deep --strict "$app"
