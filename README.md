<h1 align="center">Sliploom</h1>
<p align="center">Transfer files directly between devices on the same local network. No account or cloud service.</p>
<p align="center">
  <a href="https://github.com/Master0fFate/project-sliploom/actions/workflows/build.yml"><img src="https://github.com/Master0fFate/project-sliploom/actions/workflows/build.yml/badge.svg?branch=main" alt="Build checks"></a>
</p>
<p align="center">
  <a href="#send-and-receive-batches-over-wi-fi">Wi-Fi</a> ·
  <a href="#usb-on-windows">USB</a> ·
  <a href="#build-locally">Build locally</a> ·
  <a href="#ci-artifacts">CI artifacts</a>
</p>

**Not released.** Android and Windows have Wi-Fi development builds. macOS (Apple Silicon) and Ubuntu 24.04 (x64) are build and package targets; native transfers on those platforms haven’t been verified. USB and desktop Trusted Devices remain Windows-only. See the [transfer test results](docs/TESTING/LAN_PROOF.md) for the routes exercised so far.

## Send and receive batches over Wi-Fi

1. Run current builds on devices that can reach each other on the same Wi-Fi network. Wi-Fi sending and receiving require Android 10 or later. On the receiver, open **Receive** and choose **Receive once**. On Windows, select **Use Wi-Fi instead** first; on Android, keep the receive sheet open.
2. Select files with **Pick files** or **Share → Sliploom** on Android, or **Send → Choose files** on desktop. Scan the receiver’s QR code on Android, enter its code, or open a saved QR image in the desktop app. Check the destination, then select **Send**.
3. **Receive once** allows one batch from whoever has the short-lived invitation. Keep the QR and code private: there is no second approval. Files appear in **Received files** on Android or in the selected save folder on Windows.

A batch can contain any regular file type: up to **32 files**, **64 MiB per file**, and **256 MiB total**. A file of unknown size counts as 64 MiB. If a transfer is interrupted, already saved files remain. Check the receiver before retrying an unconfirmed file.

<details>
<summary>Trusted Devices on Android and Windows</summary>

Open **Trusted devices** on both devices. On Windows, choose **Show pairing QR** and scan it from Android. Compare the verification codes and confirm only if they match. Enable **Allow trusted receive** on Windows if you want it to accept files from paired devices.

A ready trusted peer can send without a new QR or per-batch approval. Sliploom must be running, Android must stay in the foreground, and the devices must be reachable on the same local network. Pair again if an address changes. **Forget** revokes trust without deleting saved files. Pairing does not change USB permissions.

</details>

See [LAN test evidence](docs/TESTING/LAN_PROOF.md) for the earlier physical Android-to-Windows PDF transfer and separate batch protocol checks. The protocol checks were not physical-device batch tests.

## USB on Windows

- **Android → Windows (ADB debug mode):** Install Android SDK Platform Tools on Windows and connect with a USB data cable. USB debugging must already be authorized for that PC. With Sliploom open on Windows, select one image or PDF on Android and choose **Use debug USB**. Windows asks you to **Accept** or **Decline** before any file content is sent. No QR code or automatic Wi-Fi fallback. The default save folder is `%LOCALAPPDATA%\SEAM`.
- **Windows → Android (MTP):** Unlock the phone, switch its USB mode to **File transfer**, then choose **Send → Use cable instead** in Sliploom on Windows. Check the detected phone, then select and send files (the same 32-file, 64 MiB per-file, and 256 MiB total limits apply). Files go to Android’s **Downloads** folder; a name conflict stops the copy rather than overwriting a file. This direction does not require Sliploom on Android, USB debugging, Wi-Fi, or a QR code.

## Build locally

### Desktop

Install Rust through `rustup`; the repository pins Rust `1.99.0`. On Windows x64, use the MSVC toolchain, Visual Studio Build Tools with C++ support, and the Windows SDK. From the repository root:

```sh
cargo build --locked --manifest-path apps/desktop/Cargo.toml
cargo run --locked --manifest-path apps/desktop/Cargo.toml
```

<details>
<summary>Build prerequisites for macOS and Ubuntu</summary>

- **macOS (Apple Silicon):** Xcode Command Line Tools and the native ARM64 Rust toolchain.
- **Ubuntu 24.04 (x64):** A C toolchain and the CI dependencies: `pkg-config`, `libfontconfig1-dev`, `libxkbcommon-dev`, `libdbus-1-dev`, and `libwayland-dev`.

These build instructions do not imply runtime support on either target.

</details>

### Android

Use JDK 21, Android SDK Platform 36, and Build Tools 35.0.0. Point `ANDROID_HOME` at the installed SDK. Run the Gradle wrapper from `apps/android`:

```powershell
.\gradlew.bat --no-daemon :app:assembleDebug :app:lintDebug
```

On macOS or Linux, use `./gradlew --no-daemon :app:assembleDebug :app:lintDebug` instead. Install the local debug APK from `apps/android/app/build/outputs/apk/debug/app-debug.apk` on your Android device. Android CI builds the same debug variant without signing and runs lint; it does not test on a device.

## CI artifacts

Run [Package desktop candidates](https://github.com/Master0fFate/project-sliploom/actions/workflows/package.yml) manually and open **Artifacts** on the completed run. Each archive contains an unsigned test package and its SHA-256 checksum:

- **Windows x64:** NSIS installer (`.exe`).
- **macOS Apple Silicon:** disk image (`.dmg`) containing `Sliploom.app`, not notarized.
- **Ubuntu 24.04 x64:** Debian package (`.deb`).

These are GitHub Actions artifacts, not a public release or evidence of runtime support. See [build and packaging](docs/RELEASE_PIPELINE.md) for workflow details and checksum instructions.
