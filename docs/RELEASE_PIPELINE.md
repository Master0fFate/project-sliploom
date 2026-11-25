# Release pipeline

## Public release

In **Actions**, open the [release workflow](../.github/workflows/release.yml) and select **Run workflow** on the default branch, `main`. Enter `1.1.0` for `version` (no `v` prefix). The workflow runs build checks and reusable native desktop packaging on the same commit, builds and verifies the signed APK, and checks all four SHA-256 sidecars against their packages. If these checks pass, it publishes the four packages and four sidecars to [GitHub Releases](https://github.com/Master0fFate/project-sliploom/releases) as `Sliploom v1.1.0` and marks it as latest.

| Asset | Target |
| --- | --- |
| `sliploom-android.apk` | Android, signed release APK |
| `sliploom-windows-x64.exe` | Windows x64, unsigned installer |
| `sliploom-linux-amd64.deb` | Ubuntu 24.04 x64 |
| `sliploom-macos-arm64.dmg` | macOS Apple Silicon, no Developer ID signature or notarization |

Each sidecar appends `.sha256` to the full asset filename (for example, `sliploom-android.apk.sha256`). Windows and macOS may show OS security prompts.

If publication is interrupted, the workflow can leave a draft or a reserved tag. It will not overwrite either on retry. Check the draft, tag, and commit SHA against the failed run before clearing a draft or reserving the version again. Never move a published tag.

## Android signing setup

In repository **Settings → Environments**, create `android-release` and restrict its deployment branches to `main`. Add these environment secrets:

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`
- `ANDROID_KEY_PASSWORD`

Set the environment variable `ANDROID_RELEASE_CERT_SHA256` to the public SHA-256 fingerprint of the release signing certificate, in lowercase without colons. Use a durable, securely backed-up release key for future updates. Never share the keystore or its credentials in chat or commit them to the repository.

The signed Android release has package ID `org.projectseam.android`, `versionName` `1.1.0`, and `versionCode` `9`. Android requires the same signer for an in-place upgrade. A prior debug or differently signed install cannot be upgraded in place with this APK; uninstalling it may delete app data.

## Build checks and desktop candidates

[Build checks](../.github/workflows/build.yml) also run on pushes to `main`, pull requests, or manual dispatch. Windows x64, macOS Apple silicon, and Ubuntu 24.04 x64 check formatting, run Clippy, and build the Rust workspace and desktop crate with their lockfiles. Each desktop runner also sends a small batch to itself using the production Wi-Fi sender and receiver, then checks the saved bytes and confirmed receipts. The Android job uses JDK 21 and SDK 36 to build a debug APK and run lint, without an emulator or phone.

Run [Package desktop candidates](../.github/workflows/package.yml) manually from **Actions** and download the three desktop packages and their sidecars from **Artifacts** on the completed run. They use the same filenames as public release assets, but this workflow does not publish a release. Windows x64 uses its existing defaults; macOS Apple silicon and Ubuntu 24.04 x64 build with `--features unix-trusted,unix-mtp`. On Ubuntu, packaging verifies the linked `libmtp9t64` dependency, includes `libmtp-runtime`, and simulates installation with APT. See [desktop packages](../packaging/README.md) for installation and removal.

## Verify downloads

Save the package and its matching sidecar from the same release or workflow run in one directory. From that directory, verify a checksum with `sha256sum -c <filename>.sha256` on Linux or `shasum -a 256 -c <filename>.sha256` on macOS. On Windows, use `Get-FileHash -Algorithm SHA256 <filename>` and compare it with the sidecar. A match verifies the downloaded bytes, not publisher identity.

Same-host CI and packaging checks do not validate transfers on real devices or over USB. The [LAN test notes](TESTING/LAN_PROOF.md) distinguish the physical Android-to-Windows PDF transfer from separate batch protocol checks; native macOS and Linux transfers have not been verified.
