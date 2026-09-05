#!/usr/bin/env bash
# Install a built Offcut into a prefix.
#
# Shared by every packaging path — the AUR PKGBUILD, the Flatpak
# manifest, and the AppImage builder all call this rather than each
# repeating an install layout that would then drift between them.
#
# Usage:  packaging/install.sh <destdir> [prefix]
#   destdir  staging root the files land under (a package's $pkgdir)
#   prefix   defaults to /usr
set -euo pipefail

DESTDIR="${1:?usage: install.sh <destdir> [prefix]}"
PREFIX="${2:-/usr}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BIN="$ROOT/offcut/target/release/offcut"
[ -x "$BIN" ] || { echo "install.sh: $BIN is missing — build first" >&2; exit 1; }

install -Dm755 "$BIN"                              "$DESTDIR$PREFIX/bin/offcut"
install -Dm644 "$ROOT/packaging/offcut.desktop"     "$DESTDIR$PREFIX/share/applications/io.offcut.Offcut.desktop"
install -Dm644 "$ROOT/packaging/offcut.metainfo.xml" "$DESTDIR$PREFIX/share/metainfo/io.offcut.Offcut.metainfo.xml"
install -Dm644 "$ROOT/packaging/offcut.svg"         "$DESTDIR$PREFIX/share/icons/hicolor/scalable/apps/offcut.svg"

# The example themes ship as documentation, not as config: writing into
# a user's ~/.config from a package installer is a thing packages must
# not do. They are copied by hand, and the README says so.
for theme in "$ROOT"/themes/*.toml; do
  [ -e "$theme" ] || continue
  install -Dm644 "$theme" "$DESTDIR$PREFIX/share/offcut/themes/$(basename "$theme")"
done
