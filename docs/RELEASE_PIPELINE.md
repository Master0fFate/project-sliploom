# Build and packaging

[Build checks](../.github/workflows/build.yml) run on pushes to `main`, pull requests, or manual dispatch. Windows x64, macOS Apple Silicon, and Ubuntu 24.04 x64 check formatting, run Clippy, and build the Rust workspace and desktop crate with their lockfiles. The Android job uses JDK 21 and SDK 36 to build an unsigned debug APK and run lint. It does not connect to an emulator or phone.

Run [Package desktop candidates](../.github/workflows/package.yml) from the Actions tab to build release-mode desktop binaries. Download the artifacts from the completed run. Each contains a Windows installer, macOS disk image, or Ubuntu package and its SHA-256 sidecar. See [desktop packages](../packaging/README.md) for requirements and install and removal commands.

From the directory containing the extracted files, verify a checksum with `sha256sum -c <filename>.sha256` on Linux or `shasum -a 256 -c <filename>.sha256` on macOS. On Windows, use `Get-FileHash -Algorithm SHA256 <filename>` and compare it with the sidecar. A match verifies the downloaded bytes, not publisher identity.

These unsigned packages are for testing. A build or package check does not certify installation on every machine or a successful transfer between real devices. The [LAN test notes](TESTING/LAN_PROOF.md) describe the runtime checks performed so far.
