# Linux desktop packaging assets

Shared desktop-integration assets for the `fauxx-desktop` GUI, consumed by
[`.github/workflows/linux-packaging.yml`](../../.github/workflows/linux-packaging.yml)
to build the AppImage (issue #43).

## App ID

`net.digitalgrease.FauxxDesktop` (reverse-DNS of digitalgrease.net). Every file
here keys off it: the `.desktop` file, the AppStream metainfo, the icon
filenames, and the AppImage `.zsync` update-information string.

## Contents

- `net.digitalgrease.FauxxDesktop.desktop` — the desktop entry (`Exec=fauxx-desktop`).
  Validated with `desktop-file-validate`.
- `net.digitalgrease.FauxxDesktop.metainfo.xml` — AppStream metadata. Validated
  with `appstreamcli validate`. Drives the software-center/AppImage update
  metadata; add a `<release>` entry per version.
- `icons/hicolor/<size>x<size>/apps/net.digitalgrease.FauxxDesktop.png` — the app
  icon at 16, 32, 48, 64, 128, 256, and 512 px.

## Icon provenance

The icons are downscaled from the shared Fauxx logo
(`app/src/main/res/drawable/ic_launcher_512.png`) in the
[Fauxx Android repo](https://github.com/digital-grease/fauxx), so the phone and
desktop apps present the same mark. Regenerate from the 512 px master with:

```sh
SRC=../fauxx/app/src/main/res/drawable/ic_launcher_512.png
ID=net.digitalgrease.FauxxDesktop
for s in 16 32 48 64 128 256 512; do
  mkdir -p "icons/hicolor/${s}x${s}/apps"
  magick "$SRC" -resize "${s}x${s}" "icons/hicolor/${s}x${s}/apps/${ID}.png"
done
```

## Validate locally

```sh
desktop-file-validate net.digitalgrease.FauxxDesktop.desktop
appstreamcli validate --no-net net.digitalgrease.FauxxDesktop.metainfo.xml
```
