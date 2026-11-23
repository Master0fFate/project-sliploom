# SEAM

SEAM is a local-first Android-to-desktop file transfer project. **Not released.** At `43ea485`, the debug apps let you opt in to receiving one file on Windows using a QR invitation or a manual code. See [what was verified and what remains open](docs/TESTING/LAN_PROOF.md).

## Build the debug apps on Windows

Use Rust 1.99.0 (Windows x64 MSVC) and Visual Studio Build Tools with the Windows SDK. For Android, use JDK 21, Android SDK Platform 36, and Build Tools 35.0.0. From the repository root in PowerShell:

```powershell
cargo build --locked --manifest-path apps/desktop/Cargo.toml
cargo run --locked --manifest-path apps/desktop/Cargo.toml
```

In a second PowerShell window, from the repository root:

```powershell
Set-Location apps/android
.\gradlew.bat --no-daemon assembleDebug lintDebug
```

Install the debug APK at `apps/android/app/build/outputs/apk/debug/app-debug.apk` on your own Android device.

## Receive one file

1. Put both devices on the same local Wi-Fi, where they can reach each other. On Windows, confirm the **Private IPv4 address** is the computer’s Wi-Fi address, click **Choose save folder**, and select a private folder you own and can write to. Click **Receive once**. These steps and tools do not change firewall rules. If local access is blocked, stop rather than bypassing network policy.
2. On Android, use **Pick a file** or share one of your own non-sensitive PNGs or PDFs to **SEAM**. Tap **Scan invitation** and point the camera at the desktop QR. If scanning is unavailable or you decline camera access, enter the **Manual invitation** in Android’s **Invitation** field. **Do not post the QR or code in chat or logs.** Verify the displayed destination is your computer, then tap **Send**. Selecting a file or scanning does not send it.
3. **Saved on computer** means Android received a matching authenticated save receipt, not just that it sent the bytes. If the save is unconfirmed, check the chosen folder first; do not auto-retry. To try the other file type, start a new **Receive once** session.

Android gets access to the selected file through the picker or Sharesheet, not broad storage access. Camera permission is only for scanning; Android’s network permission is used for the direct local connection. The one-time invitation pins the receiver for that session: no system-wide trust changes, account, or cloud service.
