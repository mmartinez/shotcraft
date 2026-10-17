#!/usr/bin/env bash
# Build ShotCraft and package it for Linux (<arch> is x86_64 or aarch64, the build machine's):
#
#   $DIST/shotcraft-<version>-linux-<arch>.AppImage  any distro with glibc 2.35 or newer
#   $DIST/shotcraft-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/shotcraft-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/shotcraft-<version>-linux-<arch>.tar.gz    the same files as a plain bin/ and share/ tree
#
# Usage: packaging/linux/package.sh [--skip-build]
#
# Needs cargo and nfpm (https://nfpm.goreleaser.com) for deb and rpm. appimagetool and the AppImage
# runtime are downloaded at pinned, checksummed versions into $CARGO_TARGET_DIR. The script fails if
# the binary needs a newer glibc than 2.35, the minimum nfpm.yaml and the README promise.
set -euo pipefail
# shellcheck source=packaging/env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=io.github.mmartinez.shotcraft

SKIP_BUILD=0
case "${1:-}" in
  "") ;;
  --skip-build) SKIP_BUILD=1 ;;
  *) echo "usage: $0 [--skip-build]" >&2; exit 2 ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) PKG_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; PKG_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
BASENAME="shotcraft-$VERSION-linux-$ARCH"
echo "==> ShotCraft $VERSION for Linux $ARCH"

# Link with the LLD that ships with rustc, as x86_64 already does by default. GNU ld marks the GLIBC_2.39
# version of std's weak pidfd functions as required, so the binary would not start on older glibc.
if [ "$SKIP_BUILD" = 0 ]; then
  (
    cd "$ROOT"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    sysroot="$(rustc --print sysroot)"
    export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=lld -C link-arg=-B$sysroot/lib/rustlib/$host/bin/gcc-ld"
    cargo build --release --locked -p shotcraft
  )
fi
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- the installed file tree, shared by every format ---------------------------------------------
install -Dm755 "$CARGO_TARGET_DIR/release/shotcraft" "$STAGE/usr/bin/shotcraft"
strip "$STAGE/usr/bin/shotcraft"
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.metainfo.xml" "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "$ROOT/assets/app-icon/shotcraft.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/shotcraft"
copy_docs "$STAGE/usr/share/doc/shotcraft"
"$STAGE/usr/bin/shotcraft" --version

# The newest glibc symbol version the binary requires. Weak references don't count: Rust's standard
# library links newer functions weakly and checks for them at run time.
GLIBC_MIN=2.35
glibc="$(readelf -V "$STAGE/usr/bin/shotcraft" | sed -nE 's/.*Name: GLIBC_([0-9.]+) +Flags: none.*/\1/p' | sort -V | tail -1)"
if [ "$(printf '%s\n%s\n' "$glibc" "$GLIBC_MIN" | sort -V | tail -1)" != "$GLIBC_MIN" ]; then
  echo "error: the binary needs glibc $glibc, newer than the $GLIBC_MIN the packages promise:" >&2
  objdump -T "$STAGE/usr/bin/shotcraft" | grep -F "(GLIBC_$glibc)" >&2
  exit 1
fi
echo "needs glibc $glibc or newer"

# ---- .tar.gz --------------------------------------------------------------------------------------
mkdir -p "$WORK/tar"
cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
echo "wrote $DIST/$BASENAME.tar.gz"

# ---- .deb and .rpm --------------------------------------------------------------------------------
command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
# nfpm expands environment variables in fields like `version`, but not in `contents[].src`.
sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
for fmt in deb rpm; do
  VERSION="$VERSION" PKG_ARCH="$PKG_ARCH" nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"
done

# ---- AppImage -------------------------------------------------------------------------------------
APPDIR="$WORK/ShotCraft.AppDir"
cp -R "$STAGE" "$APPDIR"
rm -rf "$APPDIR/usr/share/doc"
ln -s usr/bin/shotcraft "$APPDIR/AppRun"
cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
cp "$ROOT/assets/app-icon/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

# Digests from the GitHub release assets of appimagetool 1.9.1 and type2-runtime 20251108. Pin the
# runtime too: appimagetool otherwise downloads whatever runtime is newest.
TOOL_VERSION=1.9.1
RUNTIME_VERSION=20251108
case "$ARCH" in
  x86_64)
    TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    ;;
  aarch64)
    TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
    ;;
esac
TOOL="$CARGO_TARGET_DIR/appimagetool-$TOOL_VERSION-$ARCH.AppImage"
RUNTIME="$CARGO_TARGET_DIR/type2-runtime-$RUNTIME_VERSION-$ARCH"
download_verified "https://github.com/AppImage/appimagetool/releases/download/$TOOL_VERSION/appimagetool-$ARCH.AppImage" "$TOOL" "$TOOL_SHA256"
download_verified "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_VERSION/runtime-$ARCH" "$RUNTIME" "$RUNTIME_SHA256"
chmod 755 "$TOOL"
# Extract-and-run works without FUSE (containers, CI); the AppImage itself embeds the static runtime,
# so users need no libfuse2 either.
ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream --runtime-file "$RUNTIME" "$APPDIR" "$DIST/$BASENAME.AppImage"
APPIMAGE_EXTRACT_AND_RUN=1 "$DIST/$BASENAME.AppImage" --version

echo "==> done"
ls -lh "$DIST"/"$BASENAME".*
