#!/usr/bin/env bash
# Build a self-contained Offcut AppImage.
#
# This is the "hand someone a file" option: it carries GStreamer and its
# plugins, so a tester needs nothing installed. That is also why it is
# the largest of the three — roughly 24MB of binary plus whatever the
# GStreamer set weighs.
#
# Needs `appimagetool` on PATH. It is a single download:
#   https://github.com/AppImage/AppImageKit/releases
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="$(grep -m1 '^version' "$ROOT/offcut/Cargo.toml" | cut -d'"' -f2)"
APPDIR="$ROOT/dist/Offcut.AppDir"

command -v appimagetool >/dev/null 2>&1 || {
  echo "appimagetool is not on PATH — see the comment at the top of this file" >&2
  exit 1
}

echo "==> building release binary"
( cd "$ROOT/offcut" && cargo build --release --bin offcut )

echo "==> staging AppDir"
rm -rf "$APPDIR"
"$ROOT/packaging/install.sh" "$APPDIR" /usr

# AppImage expects these at the AppDir root as well as under /usr.
ln -sf usr/share/applications/io.offcut.Offcut.desktop "$APPDIR/io.offcut.Offcut.desktop"
ln -sf usr/share/icons/hicolor/scalable/apps/offcut.svg "$APPDIR/offcut.svg"

# # Bundling GStreamer
#
# Copy the libraries the binary links against, plus the plugin set. The
# plugins are dlopened rather than linked, so `ldd` does not see them —
# missing them produces a black frame with no error, which is the exact
# failure the app's capability probe exists to make legible.
echo "==> bundling libraries"
mkdir -p "$APPDIR/usr/lib" "$APPDIR/usr/lib/gstreamer-1.0"

ldd "$ROOT/offcut/target/release/offcut" \
  | awk '/=> \//{print $3}' \
  | grep -vE '/(libc|libm|libdl|libpthread|librt|ld-linux)' \
  | sort -u \
  | while read -r lib; do cp -Ln "$lib" "$APPDIR/usr/lib/" 2>/dev/null || true; done

# The plugin directory: the local prefix if this tree has one, else the
# system's.
# Every plugin directory, not the first one found.
#
# This used to `break` after the first hit, which on a machine with a
# user-local prefix meant copying only that prefix -- 71 plugins that
# looked like plenty, but without `coreelements`, `playback`,
# `typefindfunctions` or `app`. Those are what `uridecodebin` is built
# from, so the app started, drew its window, and then failed every open
# with "could not probe media file". A partial plugin set is worse than
# none: nothing warns, because the elements the startup probe checks for
# were present.
for dir in "$ROOT/.gst-local/root/usr/lib/gstreamer-1.0" \
           /usr/lib/gstreamer-1.0 /usr/lib64/gstreamer-1.0 \
           /usr/lib/x86_64-linux-gnu/gstreamer-1.0; do
  [ -d "$dir" ] || continue
  cp -Ln "$dir"/*.so "$APPDIR/usr/lib/gstreamer-1.0/" 2>/dev/null || true
done

# The plugins `uridecodebin` cannot work without. Absent any one of them
# the failure is a probe error at open time, far from its cause, so it is
# worth failing the build here instead.
for required in coreelements typefindfunctions playback app isomp4 \
                videoconvertscale audioconvert audioresample libav; do
  if [ ! -e "$APPDIR/usr/lib/gstreamer-1.0/libgst${required}.so" ]; then
    echo "build.sh: missing GStreamer plugin '${required}' -- the bundle" >&2
    echo "          would start but fail to open any file." >&2
    exit 1
  fi
done

# # The plugin scanner
#
# GStreamer shells out to `gst-plugin-scanner` to build its registry.
# Without it in the bundle the scan still completes in-process, but it
# prints "External plugin loader failed" on every launch — an alarming
# message about a condition that is not actually fatal, which is exactly
# the kind of noise that trains people to ignore real errors.
for scanner in /usr/lib/gstreamer-1.0/gst-plugin-scanner \
               /usr/libexec/gstreamer-1.0/gst-plugin-scanner \
               /usr/lib/x86_64-linux-gnu/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner; do
  [ -x "$scanner" ] || continue
  install -Dm755 "$scanner" "$APPDIR/usr/lib/gstreamer-1.0/gst-plugin-scanner"
  break
done

# # Plugins whose own dependencies are not bundled
#
# The plugin set includes wrappers around libraries this app has no use
# for — libaa, libcaca, wavpack. They fail to load and warn on every
# launch about ASCII-art video sinks nobody asked for. Dropping them is
# not an optimisation, it is removing a false alarm from a first-run log.
for junk in aasink cacasink wavpack dvdread dv1394 shout2 gme openal \
            vulkan libvisual; do
  rm -f "$APPDIR/usr/lib/gstreamer-1.0/libgst${junk}"*.so 2>/dev/null || true
done

cat > "$APPDIR/AppRun" <<'RUN'
#!/usr/bin/env bash
# Point the bundled GStreamer at itself before anything loads.
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
export GST_PLUGIN_SYSTEM_PATH="$HERE/usr/lib/gstreamer-1.0"
export GST_PLUGIN_PATH="$HERE/usr/lib/gstreamer-1.0"
# A stale registry from the host names plugins at paths that do not
# exist inside the image; keep ours separate.
export GST_REGISTRY="${XDG_CACHE_HOME:-$HOME/.cache}/offcut-appimage-registry.bin"
export GST_PLUGIN_SCANNER="$HERE/usr/lib/gstreamer-1.0/gst-plugin-scanner"
# The example themes travel inside the image, so the first run can copy
# them into the user's config the same way a distro package's do.
export OFFCUT_SHIPPED_THEMES="$HERE/usr/share/offcut/themes"

# # Drag-and-drop needs the X11 backend
#
# winit 0.30 implements file drops in its X11 backend only, and prefers
# Wayland whenever WAYLAND_DISPLAY is set. XWayland serves the connection
# and does support drops, so hiding the Wayland socket is what makes
# dropping a file onto the window work at all. This mirrors run.sh.
#
# Opt out with OFFCUT_FORCE_WAYLAND=1 to keep the native backend, which
# is sharper on HiDPI and cannot receive drops.
x_is_live() {
  case "${DISPLAY:-}" in
    "") return 1 ;;
    :*) n="${DISPLAY#:}"; n="${n%%.*}"; [ -S "/tmp/.X11-unix/X${n}" ] ;;
    *)  return 1 ;;
  esac
}
if [ -z "${OFFCUT_FORCE_WAYLAND:-}" ] && x_is_live; then
  unset WAYLAND_DISPLAY WAYLAND_SOCKET
fi

# Same reasoning as run.sh: with no render node, naming the software
# rasterizer skips a driver search that cannot succeed.
if [ -z "${OFFCUT_FORCE_GPU:-}" ] && [ ! -e /dev/dri/renderD128 ]; then
  export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
  export GALLIUM_DRIVER="${GALLIUM_DRIVER:-llvmpipe}"
fi

exec "$HERE/usr/bin/offcut" "$@"
RUN
chmod +x "$APPDIR/AppRun"

echo "==> packing"
mkdir -p "$ROOT/dist"
ARCH=x86_64 appimagetool "$APPDIR" "$ROOT/dist/Offcut-$VERSION-x86_64.AppImage"
echo "==> dist/Offcut-$VERSION-x86_64.AppImage"
