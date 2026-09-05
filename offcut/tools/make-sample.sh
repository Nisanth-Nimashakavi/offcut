#!/usr/bin/env bash
# Offcut: generate real sample media for development and tests.
#
# No video file ships in this repo (nothing redistributable, and a binary
# blob in git is a poor way to test a video editor anyway). This script
# synthesizes real, honestly-encoded H.264 + AAC MP4s with GStreamer, so
# every test that claims to "open a real video file" is opening a genuine
# container with a genuine codec — not a videotestsrc pipeline pretending
# to be one.
#
# Requires the user-local plugin prefix from ./setup-gst-plugins.sh
# (mp4mux/avenc_aac live in gst-plugins-good/gst-libav, which are not
# installed system-wide here — see that script's header for why).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
MEDIA="$ROOT/media"
PLUGDIR="$ROOT/.gst-local/root/usr/lib/gstreamer-1.0"

if [ -d "$PLUGDIR" ]; then
  export GST_PLUGIN_PATH="$PLUGDIR"
fi

for e in mp4mux avenc_aac x264enc; do
  if ! gst-inspect-1.0 "$e" >/dev/null 2>&1; then
    echo "error: missing GStreamer element '$e'." >&2
    echo "       Run ./tools/setup-gst-plugins.sh first." >&2
    exit 1
  fi
done

mkdir -p "$MEDIA"

# $1 name, $2 pattern, $3 seconds, $4 width, $5 height, $6 fps, $7 audio freq
gen() {
  local name="$1" pattern="$2" secs="$3" w="$4" h="$5" fps="$6" freq="$7"
  local out="$MEDIA/$name.mp4"
  local vframes=$(( secs * fps ))
  # 44100Hz / 1024 samples-per-AAC-frame ~= 43.07 frames/sec
  local aframes=$(( secs * 44100 / 1024 + 1 ))

  echo "generating $out (${w}x${h} @ ${fps}fps, ${secs}s, pattern=$pattern)"
  gst-launch-1.0 -q \
    videotestsrc num-buffers="$vframes" pattern="$pattern" \
      ! "video/x-raw,width=$w,height=$h,framerate=$fps/1" \
      ! videoconvert ! x264enc bitrate=1500 speed-preset=veryfast key-int-max="$fps" \
      ! h264parse ! mux. \
    audiotestsrc num-buffers="$aframes" wave=sine freq="$freq" \
      ! "audio/x-raw,rate=44100,channels=2" \
      ! audioconvert ! avenc_aac ! aacparse ! mux. \
    mp4mux name=mux ! filesink location="$out" 2>&1 \
    | grep -viE "Failed to load plugin|^$" || true

  if [ ! -s "$out" ]; then
    echo "error: $out was not produced" >&2
    exit 1
  fi
}

# The primary fixture: a moving ball, so a *changing* frame proves live
# decode rather than one static image being redrawn.
gen "sample"       ball   5 640 360 30 440
# A second, visually distinct source for multi-source timeline tests.
gen "sample-bars"  smpte  3 640 360 30 660

echo
echo "Sample media in $MEDIA:"
ls -la "$MEDIA"/*.mp4
echo
echo "Verifying with gst-discoverer:"
for f in "$MEDIA"/*.mp4; do
  echo "--- $(basename "$f") ---"
  gst-discoverer-1.0 "$f" 2>/dev/null | grep -E "Duration|video #|audio #|Width|Height|Frame rate" || true
done
