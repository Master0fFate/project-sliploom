# SEAM

SEAM is a local-first Android-to-desktop file transfer project. **Not released.** USB cable is the primary development flow; local Wi-Fi is optional.

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

## Receive one file over USB (development)

This option currently uses USB debugging already authorized for the Windows PC. Ordinary plug-in USB with debugging off is not yet supported.

1. With Android Platform Tools installed on the PC, connect Android using an already-authorized USB data connection and open SEAM on Windows.
2. On Android, select one image or PDF in another app and choose **Share → SEAM**.
3. In SEAM on Windows, choose **Accept** or **Decline**.

Cable mode requires no QR code, invitation, or save-folder setup. Accepted files go to `%LOCALAPPDATA%\SEAM`. SEAM sends no file content before **Accept**, never overwrites existing files, and does not automatically fall back to Wi-Fi.

## Optional: receive one file over Wi-Fi

1. Put both devices on the same local Wi-Fi, where they can reach each other. On Windows, choose **Use Wi-Fi instead**, confirm the **Private IPv4 address**, and select a private save folder you own. Click **Receive once**. SEAM does not change firewall rules.
2. On Android, share one image or PDF to **SEAM**. When cable is unavailable, choose **Use Wi-Fi**, then tap **Scan invitation** and point the camera at the desktop QR. If scanning is unavailable or you decline camera access, enter the **Manual invitation** in Android’s **Invitation** field. **Do not post the QR or code in chat or logs.** Verify the displayed destination is your computer, then tap **Send**. Selecting a file or scanning does not send it.
3. **Saved on computer** means Android received a matching authenticated save receipt, not just that it sent the bytes. If the save is unconfirmed, check the chosen folder first; do not auto-retry. To try the other file type, start a new **Receive once** session.

Android gets access to the selected file through the picker or Sharesheet, not broad storage access. Camera permission is only for scanning; Android’s network permission is used for the direct local connection. The one-time invitation pins the receiver for that session: no system-wide trust changes, account, or cloud service.
