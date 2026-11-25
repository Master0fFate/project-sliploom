# Desktop packages

Public downloads, when published, are on [GitHub Releases](https://github.com/Master0fFate/project-sliploom/releases). For test builds, run [Package desktop candidates](../.github/workflows/package.yml) manually and download each package and its `.sha256` file from **Artifacts** on the completed run. That workflow does not create tags or publish releases.

Desktop candidates and public release assets use the same stable filenames, with no version in the name. Each SHA-256 sidecar appends `.sha256` to the full package filename. Check the release tag or workflow run to identify the build before installing.

| Target | Filename | Install / remove |
| --- | --- | --- |
| Windows x64 | `sliploom-windows-x64.exe` | Run the installer. It installs only for the current user under `%LOCALAPPDATA%\Programs\Sliploom`. Remove it through Installed apps. Uninstall does not remove transfers or trust data. |
| macOS Apple Silicon | `sliploom-macos-arm64.dmg` | Open the DMG and drag `Sliploom.app` into Applications. Remove the app to uninstall. |
| Ubuntu 24.04 x64 | `sliploom-linux-amd64.deb` | Run `sudo apt install ./sliploom-linux-amd64.deb`. Remove with `sudo apt remove sliploom`. Use a graphical desktop session with working graphics drivers and a desktop portal backend for file pickers. |

The Windows installer is unsigned. The macOS DMG has no Developer ID signature or notarization. Either may trigger OS security prompts. The `.deb` is not distributed through an authenticated APT repository. A SHA-256 match verifies downloaded bytes against the sidecar, not publisher identity. Packaging checks do not verify app launch or transfers.

CI installs NSIS using [Chocolatey `nsis` 3.12.0](https://community.chocolatey.org/packages/nsis/3.12.0). The installer itself does not download anything. Ubuntu builds with the same library headers as `build.yml`. `dpkg-shlibdeps` resolves linked libraries on Ubuntu 24.04, the package also declares desktop libraries loaded at runtime, and APT simulates installing the result to check dependency resolution. Roboto is embedded in the executable; each package carries `Roboto-OFL.txt`.

To reproduce on a matching native machine with the workflow dependencies installed, run `cargo build --locked --release --manifest-path apps/desktop/Cargo.toml` from the repository root. Run the corresponding packaging command with your numeric desktop crate version and output path (examples use `1.0.0`):

```powershell
./packaging/windows.ps1 1.0.0 ./sliploom-windows-x64.exe
```

```bash
bash packaging/macos.sh 1.0.0 ./sliploom-macos-arm64.dmg
bash packaging/linux.sh 1.0.0 ./sliploom-linux-amd64.deb
```

Run the Windows command from PowerShell 7 in a disposable user profile with no existing Sliploom install. It runs the new installer and uninstaller silently to verify both.

## Public release assets

The download page follows the latest published GitHub Release. To make all four downloads available, attach these files under the same names each time before publishing the release as latest:

- Android: `sliploom-android.apk` (signed and installable; not the unsigned CI APK)
- Windows x64: `sliploom-windows-x64.exe`
- Ubuntu 24.04 x64: `sliploom-linux-amd64.deb`
- macOS Apple silicon: `sliploom-macos-arm64.dmg`

Drafts and prereleases do not replace the latest release.
