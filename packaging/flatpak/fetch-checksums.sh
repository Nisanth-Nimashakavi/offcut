#!/usr/bin/env bash
# Fill in the manifest's placeholder checksums.
#
# The manifest ships with `REPLACE_ME_*` rather than plausible-looking
# wrong hashes: a wrong hash fails the build with a mismatch and no
# explanation, whereas an obvious placeholder points here.
set -euo pipefail
MANIFEST="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/io.offcut.Offcut.yml"

fetch() {
  local url="$1" key="$2"
  echo "fetching $url" >&2
  local sum
  sum="$(curl -fsSL "$url" | sha256sum | cut -d' ' -f1)"
  echo "  $key = $sum" >&2
  sed -i "s|$key|$sum|" "$MANIFEST"
}

fetch "https://gstreamer.freedesktop.org/src/gst-plugins-ugly/gst-plugins-ugly-1.24.9.tar.xz" REPLACE_ME_gst_plugins_ugly
fetch "https://gstreamer.freedesktop.org/src/gst-libav/gst-libav-1.24.9.tar.xz"               REPLACE_ME_gst_libav
echo "manifest updated: $MANIFEST" >&2
