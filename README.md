# Offcut

A single-source video trimmer. Open a file, choose the piece you want to keep,
export it. **Your original file is never modified.**

It is deliberately not an NLE: one clip, one range, plus crop, straighten,
speed, volume and tone. Exports MP4, MOV or MKV carrying H.264 or HEVC.

---

## Install

### Arch (AUR)

```bash
yay -S offcut
```

### Flatpak

```bash
flatpak install --user offcut.flatpak
flatpak run io.offcut.Offcut
```

### AppImage

```bash
chmod +x Offcut-0.1.0-x86_64.AppImage
./Offcut-0.1.0-x86_64.AppImage
```

The AppImage bundles GStreamer, so it is the right choice if you do not want
to install anything system-wide.

---

## Build from source

You need a Rust toolchain and GStreamer's development headers.

```bash
git clone https://github.com/Nisanth-Nimashakavi/offcut
cd offcut
./run.sh                     # builds and launches
./run.sh some-video.mp4      # or open a file straight away
```

`run.sh` builds in release mode and sets the environment the app needs. A debug
build is not merely slower here — the render path is dependency code that
depends entirely on inlining, and at `opt-level = 0` scrubbing visibly lags the
pointer.

To build without running:

```bash
cd offcut && cargo build --release --bin offcut
```

The binary lands at `offcut/target/release/offcut`.

### What it needs at runtime

GStreamer, split across several packages. Offcut probes for these at startup and
names the missing one rather than failing with a black frame:

| Package (Arch names) | Supplies |
|---|---|
| `gstreamer`, `gst-plugins-base` | `uridecodebin`, `videoconvert`, `appsink` |
| `gst-plugins-good` | `qtdemux` — without it, no MP4 opens |
| `gst-plugins-ugly` | `x264enc` — the shipping encoder |
| `gst-libav` | `avdec_h264`, AAC encoding |

On Debian/Ubuntu these are `libgstreamer1.0-dev`,
`gstreamer1.0-plugins-{base,good,ugly}` and `gstreamer1.0-libav`.

Without root, `offcut/tools/setup-gst-plugins.sh` extracts them into a
user-local prefix and prints the `GST_PLUGIN_PATH` to export.

### Software rendering

With no `/dev/dri`, Mesa searches its whole driver list before falling back to
llvmpipe — measured at 8 failed driver probes before the first frame. `run.sh`
names the software rasterizer outright when there is no render node, which
removes the search. A machine with a real GPU is untouched;
`OFFCUT_FORCE_GPU=1` skips the check.

---

## Testing

```bash
cd offcut && cargo test
```

455 tests. The engine and export suites need real GStreamer elements, so
`GST_PLUGIN_PATH` must point at them if they are not installed system-wide.

---

## Theming

Offcut reads `~/.config/offcut/colors.toml`, overriding any of its 37 colour
roles per mode — or deriving all of them from a few wallpaper colours:

```toml
[wallpaper]
background = "#111016"   # what your wallpaper mostly is
accent     = "#D3685B"   # what stands out of it
```

### Where themes live

| | |
|---|---|
| `/usr/share/offcut/themes/` | what the package installs: read-only, system-wide |
| `~/.config/offcut/themes/` | yours, editable, what the menu lists |

Four themes ship with the app — `dark`, `light`, `rose-pine-moon` and
`violet` — and are copied into your config directory the **first time you run
it**, so they appear in the menu without any setup.

A package cannot write your config at install time: it runs as root, with no
user and no `HOME`. So the app does it on first launch, as you. On Flatpak the
source is `/app/share/offcut/themes/`; an AppImage carries its own copy inside
the image.

It copies once and records that it did, so editing a theme keeps your version
and deleting one keeps it deleted.

### Making your own

Copy any of them and edit:

```bash
cp ~/.config/offcut/themes/dark.toml ~/.config/offcut/themes/mine.toml
```

`dark.toml` is the app's default palette written out in full, so it is the
useful starting point: every role is present with the value the app would have
used anyway. Change one line or all 37.

Or start from a wallpaper instead of listing roles, and let the app generate
the rest:

```toml
[wallpaper]
background = "#0d1117"   # what the image mostly is
accent     = "#58a6ff"   # what stands out of it
```

New files appear in the menu on the next launch. Nothing is selected
automatically.

A contrast reading runs on whatever you load and reports controls that would be
invisible — it warns, it does not override your choice.

The full role list is documented in `offcut/crates/offcut-ui/src/theme.rs`, where each colour carries the measurement that chose it.

---

## Keyboard

Press `?` in the app for the full list. The ones worth knowing:

| | |
|---|---|
| `I` / `O` | set the start / end at the playhead |
| `Space` | play or pause |
| `←` `→` | one frame back / forward |
| `Ctrl O` / `Ctrl E` | open / export |
| `Ctrl +` `−` `0` | interface bigger / smaller / reset |

---

## Licence

MIT OR Apache-2.0.

