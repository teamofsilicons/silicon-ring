# Silicon Apps runs this with /bin/sh after installing or updating the ring CLI on Linux
# (x86_64 and aarch64), from the package directory, with APPS_INSTALL_DIR set. Updates also come
# from the background updater, which may not have the desktop session's variables.
#
# The CLI is for silicons and is already in place. This adds the Ring desktop app for the people
# (carbons) who use this machine, for this user only and without root: the AppImage in this package
# is extracted (so FUSE is not needed) to $XDG_DATA_HOME/silicon-ring/desktop, with a menu entry,
# its D-Bus activation file and its icons. A new copy replaces the old one only after it has been
# unpacked completely and has started (`AppRun --version`), so a full disk or a system too old for
# it (Ring for Linux needs glibc 2.39, as in Ubuntu 24.04, Debian 13 or Fedora 40) keeps what works.
#
# It does nothing on a machine with no desktop session, and it gives way to the ring-desktop package
# (.deb), removing its own copy once nothing runs from it. To turn it off for good: run any install
# or update once with RING_NO_DESKTOP=1, or create ~/.local/share/silicon-ring/no-desktop (delete
# that file to turn it back on). To remove the app as well, also delete
# $XDG_DATA_HOME/silicon-ring/desktop and the com.teamofsilicons.Ring files under
# $XDG_DATA_HOME/applications and dbus-1/services.
#
# It never fails the CLI install: whatever happens, it exits 0 and says what it did.

set -u

APP_ID=com.teamofsilicons.Ring

say() { printf 'ring desktop app: %s\n' "$*"; }

has_desktop() {
  [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}${XDG_CURRENT_DESKTOP:-}" ] && return 0
  for sessions in /usr/share/xsessions /usr/share/wayland-sessions; do
    for entry in "$sessions"/*.desktop; do
      [ -f "$entry" ] && return 0
    done
  done
  return 1
}

# A literal for the replacement side of sed's s|…|…|: escape \, & and the delimiter.
sed_literal() { printf '%s' "$1" | sed -e 's/[\\&|]/\\&/g'; }

# Writes $2 to $1 through a temporary file, keeping the old file unless the new one is complete.
replace_file() {
  printf '%s' "$2" > "$1.new" && [ -s "$1.new" ] && mv "$1.new" "$1" && return 0
  rm -f "$1.new"
  return 1
}

# $base as the kernel reports it (no symlinks, no trailing or doubled slashes), so paths written
# into entries and compared with /proc and AppRun's APPDIR agree.
resolve_base() {
  real=$(CDPATH= cd -P -- "$base" 2>/dev/null && pwd -P) && base=$real
  app=$base/desktop
}

# Whether a process runs from this user's copy (a swapped-out one included).
copy_in_use() {
  for exe in /proc/[0-9]*/exe; do
    case $(readlink "$exe" 2>/dev/null) in
      "$base/desktop"/*|"$base/desktop.old"/*) return 0 ;;
    esac
  done
  return 1
}

# The menu entry, D-Bus activation and icons for the copy in $app.
write_entries() {
  run=$(sed_literal "\"$app/AppRun\"")
  icon=$(sed_literal "$app/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg")
  mkdir -p "$data/applications" "$data/dbus-1/services" || return 1
  entry=$(sed -e "s|^Exec=ring-desktop|Exec=$run|" -e "s|^Icon=.*|Icon=$icon|" \
    "$app/usr/share/applications/$APP_ID.desktop") || return 1
  # Menu editors change this file in place: replace it only while it is still what was written last.
  menu=$data/applications/$APP_ID.desktop
  if [ ! -f "$menu" ] || [ ! -f "$base/menu-entry" ] || [ "$(cat "$menu")" = "$(cat "$base/menu-entry")" ]; then
    replace_file "$menu" "$entry
" || return 1
    replace_file "$base/menu-entry" "$entry
"
  fi
  # The menu entry is D-Bus activatable, so the session bus needs to know how to start Ring.
  replace_file "$data/dbus-1/services/$APP_ID.service" "[D-BUS Service]
Name=$APP_ID
Exec=\"$app/AppRun\" --gapplication-service
" || return 1

  # Icons by name too, for notifications and docks that look them up in the theme.
  (cd "$app/usr/share/icons" 2>/dev/null && find hicolor -name "$APP_ID*" -type f) | while IFS= read -r file; do
    mkdir -p "$data/icons/$(dirname "$file")" && cp "$app/usr/share/icons/$file" "$data/icons/$file"
  done
  # The icon theme spec's signal to rescan, without creating a cache that would hide later icons.
  touch "$data/icons/hicolor" 2>/dev/null
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" >/dev/null 2>&1

  # "Open at login" records the binary Ring ran from (also a swapped-out "desktop.old" one): start
  # it through AppRun, like the menu does.
  autostart=$config/autostart/$APP_ID.desktop
  # Also an entry for the ring-desktop package, which is gone now (this runs only without it).
  if { grep -qF "$base/desktop" "$autostart" 2>/dev/null \
       || { grep -qE '^Exec=ring-desktop( |$)' "$autostart" 2>/dev/null && ! command -v ring-desktop >/dev/null 2>&1; }; } \
     && ! grep -qF "Exec=\"$app/AppRun\" --background" "$autostart"; then
    login=$(sed -e "s|^Exec=.*|Exec=$run --background|" "$autostart") && replace_file "$autostart" "$login
"
  fi
  return 0
}

# The ring-desktop package provides Ring for everyone: a copy from an earlier run would shadow its
# menu entry and D-Bus service ($XDG_DATA_HOME comes first), so remove what this script made. The
# files go once nothing runs from them (a running Ring opens its sounds and modules by path).
remove_own_copy() {
  [ -d "$app" ] || [ -d "$app.old" ] || return 0
  for file in "$data/applications/$APP_ID.desktop" "$data/dbus-1/services/$APP_ID.service"; do
    grep -qF "$app/AppRun" "$file" 2>/dev/null && rm -f "$file"
  done
  rm -f "$base/menu-entry"
  autostart=$config/autostart/$APP_ID.desktop
  if grep -qF "$base/desktop" "$autostart" 2>/dev/null; then
    login=$(sed -e 's|^Exec=.*|Exec=ring-desktop --background|' "$autostart") && replace_file "$autostart" "$login
"
  fi
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" >/dev/null 2>&1
  if copy_in_use; then
    say "this user's own copy stays until Ring restarts from the ring-desktop package"
    return 0
  fi
  rm -rf "$app" "$app.old"
  say "removed this user's own copy in favour of the ring-desktop package"
}

install_desktop() {
  [ -n "${HOME:-}" ] || { say 'skipped: HOME is not set'; return 0; }
  # The background updater may lack the session's XDG_DATA_HOME: reuse where the session put Ring.
  where=$HOME/.local/share/silicon-ring/data-home
  if [ -n "${XDG_DATA_HOME:-}" ]; then
    data=$XDG_DATA_HOME
  else
    data=$(cat "$where" 2>/dev/null)
    { [ -n "$data" ] && [ -d "$data/silicon-ring" ]; } || data=$HOME/.local/share
  fi
  # Likewise for XDG_CONFIG_HOME, where Ring's "open at login" entry lives.
  cwhere=$HOME/.local/share/silicon-ring/config-home
  if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    config=$XDG_CONFIG_HOME
    { mkdir -p "${cwhere%/*}" && printf '%s\n' "$config" > "$cwhere"; } 2>/dev/null
  else
    config=$(cat "$cwhere" 2>/dev/null)
    { [ -n "$config" ] && [ -d "$config" ]; } || config=$HOME/.config
  fi
  base=$data/silicon-ring
  resolve_base
  # The opt-out is a file, so the background updater sees it too whatever its environment.
  optout=$HOME/.local/share/silicon-ring/no-desktop
  if [ -n "${RING_NO_DESKTOP:-}" ]; then
    { mkdir -p "${optout%/*}" && touch "$optout"; } 2>/dev/null
  fi
  if [ -n "${RING_NO_DESKTOP:-}" ] || [ -e "$optout" ] || [ -e "$base/no-desktop" ]; then
    [ -x /usr/bin/ring-desktop ] && remove_own_copy
    say "skipped: turned off for this user (delete $optout to turn it back on)"
    return 0
  fi
  if [ -x /usr/bin/ring-desktop ]; then
    remove_own_copy
    say 'skipped: Ring is installed for everyone on this machine (ring-desktop package)'
    return 0
  fi
  if ! has_desktop; then
    say 'skipped: this machine has no desktop session'
    return 0
  fi
  image=${APPS_INSTALL_DIR:-.}/desktop/Ring.AppImage
  [ -f "$image" ] || { say "skipped: $image is missing from the package"; return 1; }

  mkdir -p "$base" || return 1
  resolve_base
  [ -z "${XDG_DATA_HOME:-}" ] || { mkdir -p "${where%/*}" && printf '%s\n' "$data" > "$where"; } 2>/dev/null
  # What earlier runs left when the install script's time limit stopped them.
  find "$base" -maxdepth 1 -type d -name '.install.*' -mmin +5 -exec rm -rf {} + 2>/dev/null

  # An update that only changes the CLI brings the same app: keep the copy that is there.
  digest=$(sha256sum "$image" 2>/dev/null | cut -d ' ' -f 1)
  if [ -n "$digest" ] && [ -x "$app/AppRun" ] && [ "$(cat "$app/.package-sha256" 2>/dev/null)" = "$digest" ]; then
    write_entries || return 1
    say "up to date in $app"
    return 0
  fi

  work=$(mktemp -d "$base/.install.XXXXXX") || return 1
  limit=
  command -v timeout >/dev/null 2>&1 && limit='timeout 90'
  # The package's own files stay untouched: extract from a copy marked executable.
  if ! cp "$image" "$work/Ring.AppImage" || ! chmod 755 "$work/Ring.AppImage" \
     || ! (cd "$work" && $limit ./Ring.AppImage --appimage-extract >/dev/null 2>&1); then
    rm -rf "$work"
    say 'not installed: the AppImage could not be extracted'
    return 1
  fi
  new=$work/squashfs-root
  # The extractor does not report short writes. With the AppImage copy still taking its space, the
  # disk must have room left; otherwise some files may have been cut short.
  if ! dd if=/dev/zero of="$work/.room" bs=65536 count=16 >/dev/null 2>&1 \
     || [ ! -s "$new/AppRun" ] || [ ! -s "$new/usr/share/applications/$APP_ID.desktop" ] \
     || [ ! -s "$new/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg" ]; then
    rm -rf "$work"
    say 'not installed: not enough free disk space to unpack Ring'
    return 1
  fi
  rm -f "$work/.room" "$work/Ring.AppImage"
  if ! $limit "$new/AppRun" --version >/dev/null 2>&1; then
    rm -rf "$work"
    say 'skipped: Ring for Linux cannot run here (it needs glibc 2.39 or newer, as in Ubuntu 24.04, Debian 13 or Fedora 40, and the usual desktop libraries)'
    return 0
  fi
  [ -z "$digest" ] || printf '%s\n' "$digest" > "$new/.package-sha256"

  # Swap in the new copy; a running Ring keeps the files it already opened.
  rm -rf "$app.old"
  if [ -e "$app" ]; then mv "$app" "$app.old" || { rm -rf "$work"; return 1; }; fi
  if ! mv "$new" "$app"; then
    [ -e "$app.old" ] && mv "$app.old" "$app"
    rm -rf "$work"
    return 1
  fi
  rm -rf "$app.old" "$work"

  write_entries || return 1
  say "installed for $(id -un 2>/dev/null || echo this user) in $app; open Ring from the app menu"
  return 0
}

install_desktop || say 'not installed this time; the ring CLI is installed and works without it'
exit 0
