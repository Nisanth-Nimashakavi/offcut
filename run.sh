#!/usr/bin/env bash
# Launch Offcut with the environment this machine actually needs.
#
# Three things must be set here and none of them are optional:
#
#   CARGO_HOME       — the default ~/.cargo/registry is read-only here.
#   GST_PLUGIN_PATH  — this machine has no system-wide gst-plugins-good or
#                      gst-libav, which means no qtdemux, no mp4mux, no
#                      avdec_h264: it cannot open an MP4 at all. The
#                      user-local prefix in .gst-local supplies them
#                      (run tools/setup-gst-plugins.sh if it is missing).
#   WGPU_BACKEND=gl  — there is no /dev/dri here, so Vulkan finds zero
#                      adapters; the GL backend reaches Mesa's llvmpipe
#                      software rasterizer instead. On a machine with a
#                      real GPU, delete this line and wgpu picks Vulkan.
#   GALLIUM_DRIVER   — names the software rasterizer outright, so Mesa
#                      stops *searching* for hardware it will not find.
#
# Usage:
#   ./run.sh                      # empty editor; open a file with Ctrl+O
#   ./run.sh media/sample.mp4     # open a file straight away
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export CARGO_HOME="$ROOT/.cargo-home"
export GST_PLUGIN_PATH="$ROOT/.gst-local/root/usr/lib/gstreamer-1.0"
export WGPU_BACKEND="${WGPU_BACKEND:-gl}"

# # Skipping the hardware probes that cannot succeed here
#
# With no /dev/dri, Mesa still walks its whole driver list before giving
# up: it dlopens the dri2 loader, fails, tries Zink, calls
# vkEnumeratePhysicalDevices, fails, and falls back to llvmpipe. That
# search is what the window waits on — measured at **8** failed-probe
# log lines before the first frame, and none afterwards once the driver
# is named outright.
#
# Naming llvmpipe does not change what renders; it is what wgpu ends up
# using either way. It only removes the search for something else.
#
# Opt-out rather than unconditional: a machine with a real GPU must not
# be forced onto software, so this only applies when there is no render
# node to find. Set OFFCUT_FORCE_GPU=1 to skip the check.
if [ -z "${OFFCUT_FORCE_GPU:-}" ] && [ ! -e /dev/dri/renderD128 ]; then
  export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
  export GALLIUM_DRIVER="${GALLIUM_DRIVER:-llvmpipe}"
  export MESA_LOADER_DRIVER_OVERRIDE="${MESA_LOADER_DRIVER_OVERRIDE:-llvmpipe}"
fi

# Drag-and-drop needs the X11 backend.
#
# winit 0.30 implements file drops (`DroppedFile`/`HoveredFile`) in its
# X11 backend ONLY -- the Wayland backend contains no occurrence of
# either, so on a native Wayland session no drop event can ever be
# delivered and dragging a file onto the window does nothing at all.
#
# winit prefers Wayland whenever WAYLAND_DISPLAY is set, so the only way
# to reach the X11 path is to hide it. XWayland is what actually serves
# the connection, and it supports drops.
#
# This is opt-out: set OFFCUT_FORCE_WAYLAND=1 to keep the native Wayland
# backend (sharper on HiDPI, no XWayland hop) and lose drag-and-drop.
# `DISPLAY` being set proves nothing: it is routinely exported while no X
# server is listening, and switching to a dead backend means the app does
# not start at all. Probing the socket directly is the only check that
# reflects reality, and it needs no extra tooling installed.
x_is_live() {
  case "${DISPLAY:-}" in
    "") return 1 ;;
    :*) n="${DISPLAY#:}"; n="${n%%.*}"; [ -S "/tmp/.X11-unix/X${n}" ] ;;
    *)  return 1 ;;  # a remote display: not worth guessing about
  esac
}

if [ -z "${OFFCUT_FORCE_WAYLAND:-}" ] && x_is_live; then
  unset WAYLAND_DISPLAY WAYLAND_SOCKET
fi

if [ ! -d "$GST_PLUGIN_PATH" ]; then
  echo "warning: $GST_PLUGIN_PATH is missing." >&2
  echo "         Run ./offcut/tools/setup-gst-plugins.sh first, or Offcut" >&2
  echo "         will start but refuse to open any MP4." >&2
fi

# Resolve file arguments to absolute paths BEFORE the cd below.
# Without this, `./run.sh media/sample.mp4` silently opens an *empty*
# editor: the cd into offcut/ makes the relative path stop resolving, and
# the app drops a path that no longer exists rather than failing loudly.
# Caught by running this script and noticing the titlebar read "Offcut"
# instead of "sample.mp4 — Offcut".
ARGS=()
for a in "$@"; do
  if [ -e "$a" ]; then ARGS+=("$(realpath "$a")"); else ARGS+=("$a"); fi
done

# Release, not debug. A debug build of this app is not merely slower —
# the render path is dependency code (wgpu tessellation, text shaping,
# SVG rasterization) that depends entirely on inlining, and at
# `opt-level = 0` scrubbing visibly lags the pointer. Set OFFCUT_DEBUG=1
# if you actually want the debug binary for a backtrace.
cd "$ROOT/offcut"
if [ -n "${OFFCUT_DEBUG:-}" ]; then
  PROFILE_DIR=debug; BUILD_FLAGS=()
else
  PROFILE_DIR=release; BUILD_FLAGS=(--release)
fi
cargo build "${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"}" --bin offcut 2>&1 \
  | grep -vE '^\s+Compiling|^\s+Finished' || true

# The GStreamer plugin scanner prints warnings for optional plugins whose
# system libraries are absent (libaa, libcaca, libwavpack). They are
# genuinely irrelevant to video editing, and leaving them on screen trains
# you to ignore real errors — so they are filtered, and nothing else is.
# `${ARGS[@]+"${ARGS[@]}"}` rather than `"${ARGS[@]}"`: under `set -u`,
# expanding an empty array is an unbound-variable error on older bash, so
# `./run.sh` with no argument would abort instead of opening an empty
# editor. This form expands to nothing when the array is empty.
exec "./target/$PROFILE_DIR/offcut" ${ARGS[@]+"${ARGS[@]}"} 2>&1 \
  | grep -viE 'libEGL|MESA-LOADER|ZINK|GStreamer-WARNING.*Failed to load plugin' || true
