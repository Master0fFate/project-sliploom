# Desktop test candidates

Run **Package desktop candidates** manually from the repository’s Actions tab. Each job uploads one package and its `.sha256` file as workflow artifacts. The workflow does not make tags or GitHub Releases. Sliploom is not released; use these candidates only on test machines. The checks verify packaging, not app launch or transfers.

| Runner | Candidate filename | Install / remove |
| --- | --- | --- |
| Windows 2025 x64 | `sliploom-<version>-windows-x64-unsigned-test.exe` | Run the installer. It installs only for the current user under `%LOCALAPPDATA%\Programs\Sliploom`. Remove it through Installed apps. Uninstall does not remove transfers or trust data. |
| macOS 15 Apple Silicon | `sliploom-<version>-macos-arm64-unsigned-test.dmg` | Open the DMG and drag `Sliploom.app` into Applications. Remove the app to uninstall. |
| Ubuntu 24.04 x64 | `sliploom-<version>-ubuntu-24.04-amd64-unsigned-test.deb` | Run `sudo apt install ./sliploom-<version>-ubuntu-24.04-amd64-unsigned-test.deb`. Remove with `sudo apt remove sliploom`. Use a graphical desktop session with working graphics drivers and a desktop portal backend for file pickers. |

The version is the numeric desktop crate version. Windows packages are not Authenticode signed. The macOS candidate has no Developer ID signature or notarization; the native Apple Silicon linker can apply an ad-hoc signature that does not establish publisher identity. OS security prompts may appear. The `.deb` is not an authenticated APT repository package. A SHA-256 match verifies the package against the accompanying checksum, not the publisher.

CI installs NSIS using [Chocolatey `nsis` 3.12.0](https://community.chocolatey.org/packages/nsis/3.12.0). The installer itself does not download anything. Ubuntu builds with the same library headers as `build.yml`. `dpkg-shlibdeps` resolves linked libraries on Ubuntu 24.04, the package also declares desktop libraries loaded at runtime, and APT simulates installing the result to check dependency resolution. Roboto is embedded in the executable; each package carries `Roboto-OFL.txt`.

To reproduce on a matching native machine with the workflow dependencies installed, run `cargo build --locked --release --manifest-path apps/desktop/Cargo.toml` from the repository root. Run the corresponding packaging command with your crate version and output path (examples use `0.1.0`):

```powershell
./packaging/windows.ps1 0.1.0 ./sliploom-0.1.0-windows-x64-unsigned-test.exe
```

```bash
bash packaging/macos.sh 0.1.0 ./sliploom-0.1.0-macos-arm64-unsigned-test.dmg
bash packaging/linux.sh 0.1.0 ./sliploom-0.1.0-ubuntu-24.04-amd64-unsigned-test.deb
```

Run the Windows command from PowerShell 7 in a disposable user profile with no existing Sliploom install. It runs the new installer and uninstaller silently to verify both.
