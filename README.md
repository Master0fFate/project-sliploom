# SEAM

SEAM is a local-first Android-to-desktop file transfer project. This repository contains three development prototypes, not a working phone-to-PC transfer yet. See [the master plan](docs/MASTER_PLAN.md) and [first-session results](docs/MASTER_PLAN.md#first-session-results).

## Windows

Use Rust 1.99.0 for Windows x64 MSVC and Visual Studio Build Tools with the Windows SDK.

```powershell
cargo run --locked --manifest-path apps/desktop/Cargo.toml
cargo build --locked
```

The first command opens the native window preview. The second builds the [localhost transfer tools](apps/receiver-service/README.md). The preview is not connected to the receiver.

## Android

Open `apps/android` in Android Studio, or build from that directory with JDK 21, Android SDK Platform 36 and Build Tools 35.0.0 installed.

```powershell
.\gradlew.bat --no-daemon assembleDebug lintDebug
```

The debug APK is at `apps/android/app/build/outputs/apk/debug/app-debug.apk`. On a device with SEAM installed, share one non-sensitive image or PDF from another app and select **SEAM**. The probe displays the number of bytes it read and their SHA-256. It does not send or save the file.
