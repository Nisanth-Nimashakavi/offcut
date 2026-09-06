# Publishing to the AUR

The AUR hosts *build recipes*, not binaries: `PKGBUILD` and `.SRCINFO` are the
whole package. Everything else is fetched from the GitHub release tarball at
build time.

## Verified before publishing

    makepkg -f --noconfirm          # builds, tests, packages
    namcap offcut-*.pkg.tar.zst     # 0 errors
    namcap PKGBUILD                 # 0 findings

The `namcap` warnings that remain are expected: it reports the GStreamer plugin
packages as "may not be needed" because it inspects ELF linkage, and plugins
are looked up at runtime through `gst_element_factory_make`. Dropping them
would leave a package that installs and then cannot open an MP4.

## Publish

You need an AUR account with an SSH key registered at
<https://aur.archlinux.org/account/>.

    git clone ssh://aur@aur.archlinux.org/offcut.git aur-offcut
    cd aur-offcut
    cp ../packaging/aur/PKGBUILD ../packaging/aur/.SRCINFO .
    git add PKGBUILD .SRCINFO
    git commit -m "Initial import: offcut 0.1.0"
    git push

Cloning an unregistered name gives an empty repository; pushing to it creates
the package.

## Updating for a new release

1. Tag and publish the GitHub release first — the tarball must exist.
2. Bump `pkgver`, reset `pkgrel=1`.
3. Recompute the checksum:

       updpkgsums

4. Regenerate the metadata, which is what the AUR actually reads:

       makepkg --printsrcinfo > .SRCINFO

5. Rebuild and re-lint before pushing.

`.SRCINFO` is not generated server-side. A push whose `.SRCINFO` disagrees with
its `PKGBUILD` shows the wrong version on the package page.
