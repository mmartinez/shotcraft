# shellcheck shell=bash
# Shared setup for the packaging scripts. Source it: `. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"`.
#
# Exports:
#   ROOT              workspace root
#   VERSION           [workspace.package] version from Cargo.toml
#   DIST              output directory for release artifacts (default: $ROOT/dist/release)
#   CARGO_TARGET_DIR  cargo's target directory (default: $ROOT/target)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
VERSION="$(awk '
  /^\[/ { in_pkg = ($0 == "[workspace.package]"); next }
  in_pkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
' "$ROOT/Cargo.toml")"
if [ -z "$VERSION" ]; then
  echo "error: could not read [workspace.package] version from $ROOT/Cargo.toml" >&2
  exit 1
fi
export VERSION

DIST="${DIST:-$ROOT/dist/release}"
mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"
export DIST
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# The license and notice files every package carries, plus the bundled Inter font's license.
copy_docs() {
  local dest="$1" f
  for f in README.md LICENSE-MIT LICENSE-APACHE NOTICE THIRD-PARTY-LICENSES.md ATTRIBUTION.md; do
    cp "$ROOT/$f" "$dest/"
  done
  cp "$ROOT/crates/doc/assets/fonts/OFL.txt" "$dest/OFL-Inter.txt"
}

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# Download $1 to $2 unless $2 already has SHA-256 $3; fail on a mismatch.
download_verified() {
  local url="$1" dest="$2" expected="$3" tmp
  if [ -f "$dest" ] && [ "$(sha256 "$dest")" = "$expected" ]; then return 0; fi
  tmp="$(mktemp "$dest.XXXXXX")"
  curl -fsSL -o "$tmp" "$url" || { rm -f "$tmp"; return 1; }
  if [ "$(sha256 "$tmp")" != "$expected" ]; then
    echo "error: SHA-256 mismatch for $url" >&2
    rm -f "$tmp"
    return 1
  fi
  mv -f "$tmp" "$dest"
}
