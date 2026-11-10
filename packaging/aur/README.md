# AUR packages

`raw-editor-bin` repackages the Linux zip from a GitHub release. Its
PKGBUILD lives here; the AUR itself is a separate git repo per package.

## First time

1. Make an account at https://aur.archlinux.org and add your SSH public
   key under My Account.
2. `git clone ssh://aur@aur.archlinux.org/raw-editor-bin.git` (an empty
   clone creates the package on first push).
3. Copy `PKGBUILD` in, then follow "Each release" below.

## Each release

1. Wait for the release workflow to upload `raw-editor-linux-v<ver>.zip`.
2. Set `pkgver` to the version and `pkgrel=1`.
3. `updpkgsums` (from `pacman-contrib`) replaces the `SKIP`s with real
   checksums.
4. `makepkg -si` to build and install it locally, and check it runs
   (`raw-editor --version`, then launch it from the app menu).
5. `makepkg --printsrcinfo > .SRCINFO`, commit both files, `git push`.

## Notes

- A binary under `/usr` counts as packaged (`core::updates::packaged`):
  no update prompt or check, and no `~/.local/share/applications` entry,
  since the package installs `/usr/share/applications/raw-editor.desktop`.
- A from-source `raw-editor` package would build with
  `RAWEDITOR_PACKAGED="the AUR" cargo build --release --locked`; its
  `ort` dependency downloads ONNX Runtime during the build.
