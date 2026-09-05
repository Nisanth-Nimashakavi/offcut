# Packaging

Four ways to get Offcut to a tester. All of them install the same layout,
via `install.sh` — one script rather than three drifting copies.

| | Command | Tester needs |
|---|---|---|
| **Source** | `./run.sh` | Rust + GStreamer dev packages |
| **AUR** | `makepkg -si` in `aur/` | Arch; deps resolve themselves |
| **Flatpak** | `flatpak-builder --user --install --force-clean build flatpak/io.offcut.Offcut.yml` | `flatpak-builder` |
| **AppImage** | `appimage/build.sh` | Nothing — GStreamer is bundled |

## Which to hand out

**AppImage**, for testing. It is the only one that carries GStreamer, and
GStreamer is where this app's environment problems live: a missing
`gst-plugins-good` means no MP4 opens at all. 41MB, one file, `chmod +x` and
run.

The AUR package is the right long-term answer on Arch, and Flatpak the right
one for everyone else — but both ask the tester to install something first.

## Verified

- `install.sh` stages the correct layout (binary, desktop entry, metainfo,
  icon, both example themes).
- The AppDir runs **with the host's `GST_PLUGIN_PATH` unset**: 0 GStreamer
  warnings, 0 missing required elements. That is the actual test of a bundle —
  it must not silently fall back to the build machine's libraries.

## Not verified

Neither the AUR nor the Flatpak build has been *run*: `makepkg` and
`flatpak-builder` are not installed here. Their syntax is checked and the
Flatpak's checksums were fetched from the release server rather than guessed,
but the first person to run either should expect to fix something.

`appimagetool` is likewise absent, so `build.sh` was verified up to the packing
step — every stage before it was executed and its output inspected.
