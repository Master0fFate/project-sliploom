# Sliploom

Transfer files directly between devices on the same local network, without an account or cloud service.

## Send and receive batches over Wi-Fi

- Open Sliploom on both devices. Use the same local Wi-Fi network and make sure the devices can reach each other.
- Start with **Receive once** on the receiving device. Its short-lived QR code and text code are two ways to share the same invitation. Anyone with it can send one batch without a second approval, so share it only with your sender.
- A batch can include up to **32 regular files**, **64 MiB per file**, and **256 MiB total**. A file of unknown size counts as 64 MiB. Existing files are not overwritten. If a transfer is interrupted, already saved files remain; check the receiver before retrying an unconfirmed file.

[Android](#android) · [macOS Apple silicon](#macos-apple-silicon) · [Ubuntu 24.04 x64](#ubuntu-2404-x64) · [Windows x64](#windows-x64) · [Downloads](#downloads) · [USB on macOS and Ubuntu](#usb-on-macos-and-ubuntu) · [USB on Windows](#usb-on-windows)

### Android

- **Install:** Open the signed [sliploom-android.apk](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-android.apk). If prompted, allow APK installs from the app you opened it with. Wi-Fi sending and receiving require **Android 10 or later**.
- **Receive:** Swipe to **Receive files**, tap **Receive once**, and keep the sheet open until the batch finishes. To share the text code, tap **Show Wi-Fi code**. Find transfers in **Received files** and use **Save copy** to export a file.
- **Send:** In **Send file(s)**, tap **Pick files**, or use **Share → Sliploom** from another app. Tap **Scan QR** to scan the receiver’s invitation, or **Enter code** to enter its **Wi-Fi code**. Check the destination, then tap **Send**.

Android requires the same signing key for an in-place upgrade of `org.projectseam.android`. A debug or differently signed build cannot be upgraded in place with the release APK. Removing the existing app may delete its data.

### macOS Apple silicon

- **Install:** Open [sliploom-macos-arm64.dmg](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-macos-arm64.dmg), drag `Sliploom.app` into Applications, then open it. The DMG has no Developer ID signature or notarization, so macOS may warn or block it.
- **Receive:** Sliploom opens in Wi-Fi receive mode. Select **Choose save folder** before your first **Receive once**. Use **Manual code** to reveal the invitation as text. Sliploom remembers your folder across launches.
- **Send:** Select **Send → Choose files**. Paste the receiver’s **One-time code** or select **Open QR image** to read a saved QR image (not a live camera scan). Check the destination, then select **Send**.

### Ubuntu 24.04 x64

- **Install:** Download [sliploom-linux-amd64.deb](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-linux-amd64.deb), then run `sudo apt install ./sliploom-linux-amd64.deb` from the download directory. Use a graphical desktop session with working graphics drivers and a desktop portal backend for file pickers.
- **Receive:** Sliploom opens in Wi-Fi receive mode. Select **Choose save folder** before your first **Receive once**. Use **Manual code** to reveal the invitation as text. Sliploom remembers your folder across launches.
- **Send:** Select **Send → Choose files**. Paste the receiver’s **One-time code** or select **Open QR image** to read a saved QR image (not a live camera scan). Check the destination, then select **Send**.

### Windows x64

- **Install:** Run [sliploom-windows-x64.exe](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-windows-x64.exe). It installs for the current user under `%LOCALAPPDATA%\Programs\Sliploom`. The installer has no Authenticode signature, so Windows may show a security warning.
- **Receive:** Sliploom starts in cable receive mode. Select **Use Wi-Fi instead**, then **Receive once**. Use **Manual code** to reveal the invitation as text. Files go to `%LOCALAPPDATA%\Sliploom` by default. **Choose save folder** remembers your selection across launches.
- **Send:** Select **Send → Choose files**. Paste the receiver’s **One-time code** or select **Open QR image** to read a saved QR image (not a live camera scan). Check the destination, then select **Send**.

### What’s been tested

Desktop CI runs a same-host Wi-Fi batch on Windows, macOS, and Ubuntu. It uses the same sender and receiver as the app and checks file contents and receipts. It does not test the desktop UI or transfers between physical devices. The [LAN test notes](docs/TESTING/LAN_PROOF.md) record the physical Android-to-Windows transfer and earlier protocol checks.

## Downloads

Get the package and its matching checksum from the [latest GitHub release](https://github.com/Master0fFate/project-sliploom/releases/latest).

| Platform | Package | SHA-256 |
| --- | --- | --- |
| Android | [APK](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-android.apk) | [Checksum](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-android.apk.sha256) |
| macOS Apple silicon | [DMG](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-macos-arm64.dmg) | [Checksum](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-macos-arm64.dmg.sha256) |
| Ubuntu 24.04 x64 | [DEB](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-linux-amd64.deb) | [Checksum](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-linux-amd64.deb.sha256) |
| Windows x64 | [EXE](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-windows-x64.exe) | [Checksum](https://github.com/Master0fFate/project-sliploom/releases/latest/download/sliploom-windows-x64.exe.sha256) |

See [how to verify downloads](docs/RELEASE_PIPELINE.md#verify-downloads). A matching checksum verifies the downloaded bytes, not the publisher’s identity.

## USB on macOS and Ubuntu

These cable modes are available in local source builds, not in the macOS or Ubuntu v1.0.0 installers.

- **Android → desktop (ADB):** Install Android SDK Platform-Tools and run `adb start-server`. Connect one phone and authorize USB debugging. In Sliploom, select **Use cable instead** and choose an existing save folder. On Android, select one image or PDF and tap **Use debug USB (image or PDF)**, then accept the transfer on the desktop. This direction does not use Wi-Fi.
- **Desktop → Android (MTP):** Build with `--features unix-mtp` and libmtp 1.1.21 or later available through `pkg-config` (`libmtp-dev` on Ubuntu, `libmtp` on Homebrew). Unlock the phone, set its USB mode to **File transfer**, and select **Send → Use cable instead**. This experimental mode checks the new file’s name and size on the phone, not its contents. If a transfer is unconfirmed, check **Downloads** before retrying.

## USB on Windows

The v1.0.0 desktop installers offer USB transfers and Trusted Devices only on Windows. Neither is needed for Wi-Fi **Receive once**.

- **Android → Windows (ADB):** Install Android SDK Platform Tools on Windows, connect with a USB data cable, and make sure USB debugging is authorized for that PC. Keep Sliploom open in cable receive mode. On Android, select one image or PDF and choose **Use debug USB (image or PDF)**. Windows asks you to **Accept** or **Decline** before any file content is sent. There is no QR code or automatic Wi-Fi fallback.
- **Windows → Android (MTP):** Unlock the phone, set its USB mode to **File transfer**, and select **Send → Use cable instead** on Windows. Check the detected phone, then choose and send files. The Wi-Fi batch limits also apply here. Files go to Android’s **Downloads** folder; a name conflict stops the copy rather than overwriting a file. This direction does not require Sliploom on Android, USB debugging, Wi-Fi, or a QR code.

<details>
<summary>Optional Trusted Devices pairing (Android and Windows)</summary>

Open **Trusted devices** on both devices. On Windows, select **Show pairing QR**; on Android, select **Scan QR**. Compare the verification codes and confirm only if they match. Turn on **Allow trusted receive** on Windows to accept transfers from paired devices.

A ready trusted peer can send without a new QR or per-batch approval. Keep Sliploom running, keep Android in the foreground, and make sure the devices can reach each other on the same Wi-Fi. Pair again if an address changes. Forgetting a device revokes trust without deleting received files. Pairing does not change USB permissions, and existing trusted-device state remains usable.

</details>

Windows uses the same save folder for cable, Wi-Fi, and trusted receive. If it becomes unusable, choose another folder. Changing the folder does not disable trusted receive; a trusted batch already in progress finishes in the folder it started with.

## Trusted Devices on macOS and Linux

Trusted Devices on macOS and Linux require an experimental source build with `--features unix-trusted`; this feature is not in the v1.0.0 installers. Choose a save folder before enabling trusted receive. Pair with another current build that recognizes macOS and Linux peers. Trust data is stored in files intended to be private to your OS account; these files are not encrypted by Keychain or Secret Service.

## Build locally

**Desktop:** Install Rust through `rustup` (the repository pins `1.99.0`). On Windows x64, use the MSVC toolchain, Visual Studio Build Tools with C++ support, and the Windows SDK. On Apple silicon, use Xcode Command Line Tools and native ARM64 Rust. On Ubuntu 24.04 x64, install a C toolchain, `pkg-config`, `libfontconfig1-dev`, `libxkbcommon-dev`, `libdbus-1-dev`, and `libwayland-dev`. From the repository root:

```sh
cargo build --locked --manifest-path apps/desktop/Cargo.toml
cargo run --locked --manifest-path apps/desktop/Cargo.toml
```

On macOS and Ubuntu, to build both experimental features after installing libmtp:

```sh
cargo build --locked --manifest-path apps/desktop/Cargo.toml --features unix-trusted,unix-mtp
```

**Android:** Install JDK 21, Android SDK Platform 36, and Build Tools 35.0.0. Set `ANDROID_HOME` to the SDK location. From `apps/android`, run `./gradlew --no-daemon :app:assembleDebug :app:lintDebug` (use `.\gradlew.bat` instead on Windows). The local debug APK is at `apps/android/app/build/outputs/apk/debug/app-debug.apk`. Android CI builds and lints the debug variant without testing on a device.

See [desktop packaging](packaging/README.md) and the [release pipeline](docs/RELEASE_PIPELINE.md) for packaging, candidate artifacts, and release verification.
