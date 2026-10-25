# Silicon Apps runs this with /bin/sh after installing or updating the ring CLI on macOS (Apple
# silicon), from the package directory, with APPS_INSTALL_DIR set. Updates also come from the
# background updater.
#
# The CLI is for silicons and is already in place. This adds the Ring app for the people (carbons)
# who use this Mac, for this user only and without an administrator password: the notarized
# Ring.app in this package goes to ~/Applications. A new copy replaces the old one only after its
# code signature checks out (codesign --verify), so a full disk or a damaged download keeps what
# works; a Ring that is open keeps running and uses the new copy next time it starts. An update
# that brings the same app changes nothing.
#
# It gives way to Silicon Ring in /Applications (installed from the disk image), removing its own
# copy once that copy is not running, and skips Macs older than macOS 14, which the app needs. To turn it off for good: run any install or update once with
# RING_NO_DESKTOP=1, or create ~/Library/Application Support/silicon-ring/no-desktop (delete it to
# turn it back on). To remove the app as well, also delete ~/Applications/Ring.app.
#
# It never fails the CLI install: whatever happens, it exits 0 and says what it did.

set -u

say() { printf 'ring desktop app: %s\n' "$*"; }

# Whether a process runs from this user's copy (Launch Services starts it by its resolved path).
copy_in_use() {
  pgrep -f "$app/Contents/MacOS/" >/dev/null 2>&1 || pgrep -f "$apps/Ring.app/Contents/MacOS/" >/dev/null 2>&1
}

# Silicon Ring, not another app called Ring, is installed for everyone in /Applications.
system_ring() {
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Applications/Ring.app/Contents/Info.plist 2>/dev/null)" = com.teamofsilicons.ring ]
}

install_mac() {
  [ -n "${HOME:-}" ] || { say 'skipped: HOME is not set'; return 0; }
  support=$HOME/Library/Application\ Support/silicon-ring
  app=$HOME/Applications/Ring.app
  # ~/Applications as Launch Services sees it (no symlinks): Ring runs from this path.
  apps=$(CDPATH= cd -P -- "$HOME/Applications" 2>/dev/null && pwd -P) || apps=$HOME/Applications
  stamp=$support/desktop-sha256
  # The opt-out is a file, so the background updater sees it too whatever its environment.
  optout=$support/no-desktop
  if [ -n "${RING_NO_DESKTOP:-}" ]; then
    { mkdir -p "$support" && touch "$optout"; } 2>/dev/null
  fi
  # A ~/Applications that is a link to /Applications is everyone's folder: never install into or
  # delete from it on one user's behalf.
  if [ "$HOME/Applications" -ef /Applications ]; then
    say 'skipped: ~/Applications is a link to /Applications, which is for everyone on this Mac'
    return 0
  fi
  if system_ring; then
    # Ring for everyone on this Mac: this user's own copy would only compete with it.
    if [ -d "$app" ] && ! copy_in_use; then
      rm -rf "$app" "$stamp"
      say "removed this user's own copy in favour of /Applications/Ring.app"
    fi
    say 'skipped: Ring is installed in /Applications'
    return 0
  fi
  if [ -n "${RING_NO_DESKTOP:-}" ] || [ -e "$optout" ]; then
    say "skipped: turned off for this user (delete $optout to turn it back on)"
    return 0
  fi
  # Ring for Mac needs macOS 14 (Darwin 23); the CLI alone runs from macOS 12. uname, not sw_vers,
  # which SYSTEM_VERSION_COMPAT can make report 10.16.
  darwin=$(uname -r 2>/dev/null)
  if [ "${darwin%%.*}" -lt 23 ] 2>/dev/null; then
    say "skipped: Ring for Mac needs macOS 14 or later; this Mac has macOS $(sw_vers -productVersion 2>/dev/null)"
    return 0
  fi
  zip=${APPS_INSTALL_DIR:-.}/desktop/Ring-macOS.zip
  [ -f "$zip" ] || { say "skipped: $zip is missing from the package"; return 1; }

  # An update that only changes the CLI brings the same app: keep the copy that is there. The
  # record lives outside the bundle, which must stay exactly as signed.
  digest=$(shasum -a 256 "$zip" 2>/dev/null | cut -d ' ' -f 1)
  if [ -n "$digest" ] && [ -x "$app/Contents/MacOS/Ring" ] && [ "$(cat "$stamp" 2>/dev/null)" = "$digest" ]; then
    say "up to date in $app"
    return 0
  fi

  mkdir -p "$HOME/Applications" "$support" || return 1
  # What earlier runs left when the install script's time limit stopped them.
  find "$apps/" -maxdepth 1 -type d -name '.ring-install.*' -mmin +5 -exec rm -rf {} + 2>/dev/null
  work=$(mktemp -d "$HOME/Applications/.ring-install.XXXXXX") || return 1
  if ! ditto -x -k "$zip" "$work" 2>/dev/null || [ ! -x "$work/Ring.app/Contents/MacOS/Ring" ]; then
    rm -rf "$work"
    say 'not installed: the app could not be unpacked'
    return 1
  fi
  # A complete, untouched copy has a valid signature; a short write on a full disk does not.
  if ! codesign --verify --deep --strict "$work/Ring.app" >/dev/null 2>&1; then
    rm -rf "$work"
    say 'not installed: the unpacked app did not pass its signature check (is the disk full?)'
    return 1
  fi

  # Swap in the new copy; a Ring that is open keeps the files it already opened.
  old=$work/Ring.app.old
  if [ -e "$app" ]; then mv "$app" "$old" || { rm -rf "$work"; return 1; }; fi
  if ! mv "$work/Ring.app" "$app"; then
    [ -e "$old" ] && mv "$old" "$app"
    rm -rf "$work"
    return 1
  fi
  rm -rf "$work"
  [ -z "$digest" ] || printf '%s\n' "$digest" > "$stamp"
  # Tell Launch Services, so Spotlight and Launchpad find it right away.
  lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  [ -x "$lsregister" ] && "$lsregister" -f "$app" >/dev/null 2>&1

  say "installed for $(id -un 2>/dev/null || echo this user) in $app; open Ring from Applications or Spotlight"
  return 0
}

install_mac || say 'not installed this time; the ring CLI is installed and works without it'
exit 0
