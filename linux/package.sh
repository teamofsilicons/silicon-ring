#!/usr/bin/env bash
# Builds ring-desktop and packages it for the host architecture:
#   dist/ring-desktop_<version>_<arch>.deb      (dpkg-deb)
#   dist/Ring-<version>-<arch>.AppImage          (linuxdeploy + its GTK plugin, or appimagetool)
#
# Usage: linux/package.sh [--no-build] [--deb-only]
#
# Environment:
#   CARGO_TARGET_DIR   cargo target directory (default: <repo>/target/linux)
#   OUT_DIR            where packages are written (default: linux/dist)
#   LINUXDEPLOY        path to linuxdeploy (bundles GTK, libadwaita and their data into the AppImage;
#                      also needs appimagetool and patchelf)
#   LINUXDEPLOY_PLUGIN_GTK  path to linuxdeploy-plugin-gtk.sh (looked up next to linuxdeploy too)
#   APPIMAGETOOL       path to appimagetool (alone: an AppImage that uses the system's GTK4)
#   REQUIRE_APPIMAGE=1 fail instead of skipping when no AppImage tool is available
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
APP_ID="com.teamofsilicons.Ring"
BIN="ring-desktop"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$HERE/Cargo.toml" | head -n1)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/linux}"
OUT_DIR="${OUT_DIR:-$HERE/dist}"
BUILD=1
APPIMAGE=1

for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --deb-only) APPIMAGE=0 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

case "$(uname -m)" in
  x86_64) DEB_ARCH=amd64; APPIMAGE_ARCH=x86_64 ;;
  aarch64|arm64) DEB_ARCH=arm64; APPIMAGE_ARCH=aarch64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

log() { printf '\033[1m==> %s\033[0m\n' "$*"; }

if [ "$BUILD" = 1 ]; then
  log "Building $BIN $VERSION (release)"
  (cd "$HERE" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --locked)
fi
BINARY="$TARGET_DIR/release/$BIN"
[ -x "$BINARY" ] || { echo "missing $BINARY (build first or drop --no-build)" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT_DIR"

# ---- the installed tree (shared by both formats) ---------------------------------------------
stage() {
  local root="$1" prefix="$2"
  local share="$root$prefix/share"
  install -Dm755 "$BINARY" "$root$prefix/bin/$BIN"
  install -Dm644 "$HERE/data/$APP_ID.desktop" "$share/applications/$APP_ID.desktop"
  install -Dm644 "$HERE/data/$APP_ID.metainfo.xml" "$share/metainfo/$APP_ID.metainfo.xml"
  sed "s|@bindir@|$prefix/bin|" "$HERE/data/$APP_ID.service" > "$WORK/$APP_ID.service"
  install -Dm644 "$WORK/$APP_ID.service" "$share/dbus-1/services/$APP_ID.service"

  # App icon: the full-bleed mark reads best at small sizes, the rounded tile above that.
  local size src
  for size in 16 24 32 48; do
    install -Dm644 "$ROOT/assets/icon/png/icon-$size.png" "$share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
  done
  for size in 64 128 256 512; do
    src="$ROOT/assets/icon/png/icon-macos-$size.png"
    install -Dm644 "$src" "$share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
  done
  install -Dm644 "$ROOT/assets/icon/icon-macos.svg" "$share/icons/hicolor/scalable/apps/$APP_ID.svg"
  install -Dm644 "$HERE/data/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg" \
    "$share/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"

  # Brand fonts (loaded app-locally through fontconfig) and call sounds.
  local data="$share/$APP_ID"
  find "$ROOT/assets/fonts" -name '*.ttf' -exec install -Dm644 -t "$data/fonts" {} +
  install -Dm644 "$ROOT/assets/fonts/geist-pixel/LICENSE.txt" "$data/fonts/GeistPixel-LICENSE.txt"
  install -Dm644 -t "$data/sounds" "$ROOT"/assets/sounds/*.wav

  install -Dm644 /dev/stdin "$share/doc/$BIN/copyright" <<EOF
Ring for Linux $VERSION
Copyright Team of Silicons. Licensed under the MIT license.
Source: https://github.com/teamofsilicons/silicon-ring

Bundled fonts: BDO Grotesk (VJ Type), Hedvig Letters Serif (Kristian Möller) and
Geist Pixel (Vercel with basement.studio), all under the SIL Open Font License 1.1.
EOF
}

# ---- .deb ---------------------------------------------------------------------------------------
deb_depends() {
  # Prefer the real shared-library dependencies of this build.
  if command -v dpkg-shlibdeps >/dev/null 2>&1; then
    local probe="$WORK/shlibdeps"
    mkdir -p "$probe/debian"
    printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$BIN" "$BIN" > "$probe/debian/control"
    local out
    if out="$(cd "$probe" && dpkg-shlibdeps -O "$BINARY" 2>/dev/null)"; then
      echo "${out#shlibs:Depends=}"
      return
    fi
  fi
  echo "libc6 (>= 2.35), libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), libglib2.0-0t64 (>= 2.70) | libglib2.0-0 (>= 2.70), libfontconfig1"
}

build_deb() {
  log "Packaging .deb"
  local root="$WORK/deb"
  stage "$root" /usr
  local installed_kb
  installed_kb="$(du -sk "$root/usr" | cut -f1)"
  mkdir -p "$root/DEBIAN"
  cat > "$root/DEBIAN/control" <<EOF
Package: $BIN
Version: $VERSION
Architecture: $DEB_ARCH
Section: net
Priority: optional
Maintainer: Team of Silicons <https://github.com/teamofsilicons/silicon-ring/issues>
Installed-Size: $installed_kb
Depends: $(deb_depends), libpulse0
Recommends: libgtk-4-media-gstreamer, gstreamer1.0-plugins-good, gstreamer1.0-pulseaudio, xdg-desktop-portal, gnome-keyring | kwalletmanager | keepassxc
Homepage: https://ring.teamofsilicons.com
Description: Voice calls between carbons and silicons
 Ring lets people (carbons) and AI agents (silicons) talk over voice calls,
 calling Silicon Accounts ids instead of phone numbers. Calls ring on every
 device, with live transcripts, voicemail and handoff between devices.
 .
 Ring sends no telemetry.
EOF
  local deb="$OUT_DIR/${BIN}_${VERSION}_${DEB_ARCH}.deb"
  dpkg-deb --root-owner-group -Zxz --build "$root" "$deb" >/dev/null
  log "Wrote $deb"
}

# ---- AppImage -----------------------------------------------------------------------------------
find_tool() {
  local var="$1"; shift
  local value="${!var:-}"
  if [ -n "$value" ] && [ -x "$value" ]; then echo "$value"; return; fi
  local name
  for name in "$@"; do
    if command -v "$name" >/dev/null 2>&1; then command -v "$name"; return; fi
  done
}

# Repairs two things linuxdeploy's patchelf pass gets wrong for GTK 4. Rewriting the search path
# relocates the .gresource.* sections that GTK 4.18, libadwaita and AppStream embed, while their
# code keeps reading the old addresses (GTK then aborts with "Cannot find resource"); those
# libraries go back unmodified, and the executable gets a DT_RPATH, which unlike RUNPATH also
# serves the dependencies of libraries that have no search path of their own.
fix_library_paths() {
  local appdir="$1" lib name original ldconfig
  ldconfig="$(command -v ldconfig || echo /sbin/ldconfig)"
  for lib in "$appdir"/usr/lib/*.so*; do
    [ -f "$lib" ] && [ ! -L "$lib" ] || continue
    readelf -S -W "$lib" | grep -q ' \.gresource\.' || continue
    name="$(basename "$lib")"
    original="$("$ldconfig" -p | awk -v n="$name" '$1 == n { print $NF; exit }')"
    [ -n "$original" ] || { echo "cannot find the system copy of $name" >&2; exit 1; }
    install -m644 "$(readlink -f "$original")" "$lib"
  done
  patchelf --force-rpath --set-rpath '$ORIGIN/../lib' "$appdir/usr/bin/$BIN"

  # Modules in subdirectories (GTK's media and print backends, gdk-pixbuf loaders) must find
  # their dependencies in usr/lib. The SVG loader is also linked into usr/lib, where gdk-pixbuf
  # opens it by bare name and `$ORIGIN` is usr/lib itself, so both paths are listed.
  local module relative
  while IFS= read -r -d '' module; do
    relative="$(realpath --relative-to="$(dirname "$module")" "$appdir/usr/lib")"
    patchelf --set-rpath "\$ORIGIN/$relative:\$ORIGIN" "$module"
  done < <(find "$appdir/usr/lib" -mindepth 2 -name '*.so' -type f -print0)
}

# The GTK plugin's hook forces GTK_THEME, which fights libadwaita's own stylesheet.
trim_gtk_hook() {
  local hook="$1/apprun-hooks/linuxdeploy-plugin-gtk.sh"
  [ -f "$hook" ] && sed -i '/^export GTK_THEME=/d' "$hook"
  return 0
}

pack_appimage() {
  local appdir="$1" image="$2"
  APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$APPIMAGE_ARCH" "$APPIMAGETOOL_BIN" --no-appstream "$appdir" "$image" >/dev/null
}

build_appimage() {
  local appdir="$WORK/AppDir"
  stage "$appdir" /usr
  # AppImages are launched by path, not through D-Bus activation.
  sed -i '/^DBusActivatable=/d' "$appdir/usr/share/applications/$APP_ID.desktop"
  rm -rf "$appdir/usr/share/dbus-1"
  local image="$OUT_DIR/Ring-$VERSION-$APPIMAGE_ARCH.AppImage"
  rm -f "$image"

  local linuxdeploy gtk_plugin
  linuxdeploy="$(find_tool LINUXDEPLOY linuxdeploy "linuxdeploy-$APPIMAGE_ARCH.AppImage")"
  gtk_plugin="${LINUXDEPLOY_PLUGIN_GTK:-}"
  if [ -z "$gtk_plugin" ] && [ -n "$linuxdeploy" ] && [ -x "$(dirname "$linuxdeploy")/linuxdeploy-plugin-gtk.sh" ]; then
    gtk_plugin="$(dirname "$linuxdeploy")/linuxdeploy-plugin-gtk.sh"
  fi
  APPIMAGETOOL_BIN="$(find_tool APPIMAGETOOL appimagetool "appimagetool-$APPIMAGE_ARCH.AppImage")"

  if [ -n "$linuxdeploy" ] && [ -n "$gtk_plugin" ] && [ -n "$APPIMAGETOOL_BIN" ] && command -v patchelf >/dev/null; then
    log "Packaging AppImage (linuxdeploy bundles GTK4 and libadwaita, appimagetool packs it)"
    local plugins="$WORK/plugins"
    mkdir -p "$plugins"
    ln -s "$gtk_plugin" "$plugins/linuxdeploy-plugin-gtk.sh"
    (
      cd "$WORK"
      export PATH="$plugins:$PATH" DEPLOY_GTK_VERSION=4 ARCH="$APPIMAGE_ARCH"
      export APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=1
      "$linuxdeploy" --appdir "$appdir" \
        --executable "$appdir/usr/bin/$BIN" \
        --desktop-file "$appdir/usr/share/applications/$APP_ID.desktop" \
        --icon-file "$appdir/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" \
        --plugin gtk >/dev/null
    )
    fix_library_paths "$appdir"
    trim_gtk_hook "$appdir"
    pack_appimage "$appdir" "$image"
  elif [ -n "$APPIMAGETOOL_BIN" ]; then
    log "Packaging AppImage with appimagetool (uses the system's GTK4 and libadwaita)"
    install -Dm755 /dev/stdin "$appdir/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
exec "$HERE/usr/bin/ring-desktop" "$@"
EOF
    cp "$appdir/usr/share/applications/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
    cp "$appdir/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$appdir/$APP_ID.png"
    ln -sf "$APP_ID.png" "$appdir/.DirIcon"
    pack_appimage "$appdir" "$image"
  else
    if [ "${REQUIRE_APPIMAGE:-0}" = 1 ]; then
      echo "no AppImage tools found (linuxdeploy, its GTK plugin, appimagetool and patchelf; or appimagetool alone)" >&2
      exit 1
    fi
    log "Skipping the AppImage: appimagetool is not available"
    return
  fi
  [ -f "$image" ] || { echo "the AppImage was not produced" >&2; exit 1; }
  chmod +x "$image"
  log "Wrote $image"
}

build_deb
if [ "$APPIMAGE" = 1 ]; then
  build_appimage
fi
