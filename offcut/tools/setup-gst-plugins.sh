#!/usr/bin/env bash
# Offcut: install the GStreamer plugins this project needs into a
# USER-LOCAL prefix, with no root access required.
#
# Why this exists (checked 2026-08-28, corrects PLAN.md §1's claim that
# `avdec_h264` is "confirmed present"): this machine has
# gst-plugins-{base,bad,ugly} but NOT gst-plugins-good and NOT gst-libav.
# Without those two there is no `qtdemux`, no `mp4mux`, and no
# `avdec_h264`/`avenc_aac` -- i.e. the editor cannot open or write an MP4
# at all. `sudo` is unavailable here ("no new privileges" is set), so the
# fix is to fetch the packages and extract them into a prefix that
# GST_PLUGIN_PATH points at. GStreamer supports exactly this natively.
#
# Versions are pinned to this system's installed GStreamer (1.28.5-2) and
# pulled from the Arch Linux Archive, because the live repo has already
# moved to -4 and mixing plugin ABI versions across a GStreamer minor
# build is how you get silent element-registration failures.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="$(cd "$HERE/../.." && pwd)/.gst-local"
PKGDIR="$PREFIX/pkg"
ROOT="$PREFIX/root"
ALA="https://archive.archlinux.org/packages"

# Match whatever GStreamer core is actually installed here.
GST_VER="$(pkg-config --modversion gstreamer-1.0 2>/dev/null || echo "")"
PKGREL="2"
if [ -z "$GST_VER" ]; then
  echo "error: gstreamer-1.0 not found via pkg-config; install GStreamer first." >&2
  exit 1
fi
echo "System GStreamer: $GST_VER (targeting pkgrel -$PKGREL)"

mkdir -p "$PKGDIR" "$ROOT"

fetch() {
  local name="$1" first="${1:0:1}"
  local file="${name}-${GST_VER}-${PKGREL}-x86_64.pkg.tar.zst"
  if [ -f "$PKGDIR/$file" ]; then
    echo "  cached: $file"; return 0
  fi
  echo "  fetching: $file"
  curl -sfL -o "$PKGDIR/$file" "$ALA/$first/$name/$file" || {
    echo "error: could not download $file from the Arch Linux Archive." >&2
    echo "       Check that $GST_VER-$PKGREL is a real published version." >&2
    return 1
  }
}

for p in gst-plugins-good gst-libav; do fetch "$p"; done

for f in "$PKGDIR"/*.pkg.tar.zst; do
  echo "  extracting: $(basename "$f")"
  tar -I zstd -xf "$f" -C "$ROOT"
done

PLUGDIR="$ROOT/usr/lib/gstreamer-1.0"
echo
echo "Done. Plugins extracted to: $PLUGDIR"
echo
echo "Export this before running offcut or its tests:"
echo "  export GST_PLUGIN_PATH=\"$PLUGDIR\""
echo
echo "Verifying the elements the editor actually needs:"
export GST_PLUGIN_PATH="$PLUGDIR"
missing=0
for e in qtdemux matroskademux mp4mux matroskamux avdec_h264 avdec_aac avenc_aac uridecodebin x264enc videoconvert videorate pitch volume concat; do
  if gst-inspect-1.0 "$e" >/dev/null 2>&1; then
    printf '  OK   %s\n' "$e"
  else
    printf '  MISS %s\n' "$e"; missing=1
  fi
done
exit $missing
