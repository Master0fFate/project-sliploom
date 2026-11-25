# Desktop packages

Public downloads, when published, are on [GitHub Releases](https://github.com/Master0fFate/project-sliploom/releases). For test builds, run [Package desktop candidates](../.github/workflows/package.yml) manually and download each package and its `.sha256` file from **Artifacts** on the completed run. That workflow does not create tags or publish releases.

Desktop candidates and public release assets use the same stable filenames, with no version in the name. Each SHA-256 sidecar appends `.sha256` to the full package filename. Check the release tag or workflow run to identify the build before installing.

| Target | Filename | Install / remove |
| --- | --- | --- |
| Windows x64 | `sliploom-windows-x64.exe` | Run the installer. It installs only for the current user under `%LOCALAPPDATA%\Programs\Sliploom`. Remove it through Installed apps. Uninstall does not remove transfers or trust data. |
| macOS Apple Silicon | `sliploom-macos-arm64.dmg` | Open the DMG and drag `Sliploom.app` into Applications. Remove the app to uninstall. |
| Ubuntu 24.04 x64 | `sliploom-linux-amd64.deb` | Run `sudo apt install ./sliploom-linux-amd64.deb`. Remove with `sudo apt remove sliploom`. Use a graphical desktop session with working graphics drivers and a desktop portal backend for file pickers. |

The Windows installer is unsigned. The macOS DMG has no Developer ID signature or notarization. Either may trigger OS security prompts. The `.deb` is not distributed through an authenticated APT repository. A SHA-256 match verifies downloaded bytes against the sidecar, not publisher identity. macOS and Ubuntu packaging run the packaged executable's `--version` command to check shared-library loading. This does not exercise the desktop UI or physical transfers. The separate desktop CI smoke check sends a Wi-Fi batch on the same host; it does not test USB hardware.

The v1.1.0 packages include Trusted Devices on all three desktop targets, but trusted receive requires turning on **Allow trusted receive**. Android-to-desktop USB receive needs Android SDK Platform Tools and authorized USB debugging. Desktop-to-Android MTP send needs the phone unlocked and in **File transfer** mode. Unix MTP is experimental.

CI installs NSIS using [Chocolatey `nsis` 3.12.0](https://community.chocolatey.org/packages/nsis/3.12.0). The installer itself does not download anything. macOS builds pinned libusb and libmtp from source using Xcode Command Line Tools and `pkg-config`. The DMG bundles the libraries, upstream source archives with their licenses, and the build script; recipients do not need Homebrew. Ubuntu builds with `libmtp-dev`. Ubuntu packaging requires `dpkg-shlibdeps` to identify `libmtp9t64`, checks that it is installed and that the packaged executable resolves its linked libraries, and adds `libmtp-runtime` for the distro's MTP helper. APT simulates installing the `.deb` without recommendations. Roboto is embedded in the executable; each package carries `Roboto-OFL.txt`.

To reproduce on a matching native machine with the workflow dependencies installed, build from the repository root and pass the numeric desktop crate version and output path to the matching script (examples use `1.1.0`). On Windows, keep the default features:

```powershell
cargo build --locked --release --manifest-path apps/desktop/Cargo.toml
./packaging/windows.ps1 1.1.0 ./sliploom-windows-x64.exe
```

On macOS, start with a fresh build prefix and keep it until packaging finishes:

```bash
export SLIPLOOM_MTP_PREFIX="$(cd "${TMPDIR:-/tmp}" && pwd -P)/sliploom-mtp-1.1.0"
bash packaging/macos-build-mtp.sh "$SLIPLOOM_MTP_PREFIX"
PKG_CONFIG_PATH= PKG_CONFIG_LIBDIR="$SLIPLOOM_MTP_PREFIX/lib/pkgconfig" \
  MACOSX_DEPLOYMENT_TARGET=11.0 RUSTFLAGS='-C link-arg=-Wl,-headerpad_max_install_names' \
  cargo build --locked --release --manifest-path apps/desktop/Cargo.toml --features unix-trusted,unix-mtp
bash packaging/macos.sh 1.1.0 ./sliploom-macos-arm64.dmg
```

On Ubuntu, enable both Unix features against the distribution libraries:

```bash
cargo build --locked --release --manifest-path apps/desktop/Cargo.toml --features unix-trusted,unix-mtp
bash packaging/linux.sh 1.1.0 ./sliploom-linux-amd64.deb
```

Run the Windows command from PowerShell 7 in a disposable user profile with no existing Sliploom install. It runs the new installer and uninstaller silently to verify both.

## Public release assets

The download page follows the latest published GitHub Release. To make all four downloads available, attach these files under the same names each time before publishing the release as latest:

- Android: `sliploom-android.apk` (signed and installable; not the unsigned CI APK)
- Windows x64: `sliploom-windows-x64.exe`
- Ubuntu 24.04 x64: `sliploom-linux-amd64.deb`
- macOS Apple silicon: `sliploom-macos-arm64.dmg`

Drafts and prereleases do not replace the latest release.
