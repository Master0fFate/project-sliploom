# SEAM prototype baseline audit

Baseline: `65254b4b6db713413d9d22036cc4fe27d6f3156b`. Remediation and verification apply to the working-tree changes on that baseline. Independent auditors: GPT-6.2 Astra at medium reasoning effort. The report was handed to the remediation planner, Android and Rust workers, and separate QA before recheck.

Verdict: not ready for release. This audit covers the implemented Android Share probe, Rust loopback transfer tools, Windows zgx preview, and their build and dependency configuration. It is not the [final application audit](../MASTER_PLAN.md#final-audit-and-remediation).

| ID · priority · owner | Finding and fix | Recheck and remaining acceptance |
| --- | --- | --- |
| AND-01 · P1 · Android | Cancel could report completion and release the activity's read handle while provider I/O was still running. `MainActivity.kt` now holds one process-wide reader slot until work and cleanup return, signals descriptor open, closes the stream on cancellation, and distinguishes **Cancelling** from **Reading cancelled**. Review caught and corrected an unregistered stream wrapper that also needed closing. | Verified on Pixel. Cancelling a slow read closed the provider pipe early, showed **Reading cancelled** without a digest, and let the next share succeed. A second share while the first open was pending showed **Reader busy** and did not open another stream. Providers that never return remain untested. |
| AND-02 · P2 · Android | `MainActivity.kt` previously accepted only `EXTRA_STREAM`. It now also accepts one `ClipData` URI when the extra is absent, rejects conflicting or multiple items, and preserves the content URI and read-grant checks. | Verified on Pixel. The matching `EXTRA_STREAM` + `ClipData` and `ClipData`-only routes both returned the reference PDF hash. An explicit share with conflicting URIs and one with two clip items were both rejected without opening the provider or displaying a digest. |
| AND-03 · P2 · Android | Process-death restore could look like an ordinary launch. `MainActivity.kt` now saves only a session marker and shows **Share the file again** on restoration without a retained result. It does not save or reopen the URI. Rotation keeps the ViewModel's completed result. | Verified process-death recovery on Pixel. QA killed the backgrounded SEAM process during a slow read, then returned to the same task under a new process. It showed **Share the file again** and did not reopen the provider. A normal launch still showed the idle hint. Physical rotation was not tested. |
| CORE-01 · P2 · Rust | `apps/receiver-service/src/main.rs` now distinguishes a file already published from a failure to send confirmation. `src/bin/dev-sender.rs` tells the operator to check the destination when confirmation is uncertain. The wire format and verified-save-before-ACK order are unchanged. | Verified on Windows with a controlled post-publication `BrokenPipe` in a throwaway receiver build. The saved file stayed byte-identical, both CLIs exited `1` with the expected messages, and the sender did not report success. No claim about real Wi-Fi loss. |
| CORE-02 · P2 · Build | `rust-toolchain.toml` now pins Rust `1.99.0` without selecting a Windows host for every checkout. | Verified the pin and locked build on Windows x64 MSVC. Native macOS and Linux builds remain unverified. |
| CORE-03 · P2 · Receiver QA | `apps/receiver-service/README.md` now provides a bounded loopback smoke using disposable credentials and files, for both a normal transfer and an occupied destination. | QA ran the documented check. Normal: both exits `0`; 29 bytes, matching SHA-256 `747694dbd228e2e9c5f2d1c6f4ed018450d15c2d5b51d4a2e352c1dc2270d6aa`. Occupied: both exits `1`; original bytes unchanged, no leftover stage. |

## Evidence and limits

- Windows baseline QA rebuilt the locked zgx 0.1.10 preview, opened a native SEAM window and closed it normally. The preview was unchanged by the audit fixes. This does not establish macOS or Linux window support, accessibility or packaging.
- Rust QA reran the README smoke, `cargo fmt --all --check`, and `cargo clippy --locked --all-targets -- -D warnings`. The locked build passed with a host linker warning about `-fuse-ld=lld`. TLS trust in the prototype is scoped to the supplied test CAs on loopback, not product pairing. Crash recovery and durability are not verified.
- Android QA ran `gradlew.bat --no-daemon assembleDebug lintDebug`. Lint reported 0 errors, 11 warnings and 1 hint. The v2-signed debug APK declares image and PDF shares and requests no Internet or storage permissions. QA installed that build on the Pixel for the checks below. Lint version recommendations, backup-rule advice and the missing app icon are not evidence of a data leak or a failed share.
- Cargo locks give registry artifact versions and checksums; Android pins the wrapper distribution checksum, plugin versions and direct dependencies, not the entire resolved dependency graph. An offline candidate scan ran, but Kotlin is profile-only there. Current dependency advisories, license compliance and platform release artifacts were not verified.

## Physical device checks

On a Pixel 9 Pro XL (API 37), a separate disposable Android app shared generated fixtures through the system Sharesheet. SEAM displayed **File read. Nothing was sent.** The byte counts and SHA-256 digests matched the source fixtures:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| PNG | 234 | `9162f9035a6ea2ca640f5c1f922e15d67f5c9eb8d65684574e339f23e68c99cb` |
| PDF | 589 | `0e226049ca9aa290bbe7bb949cff15811ba0f7527a25d6d6c78a6b0d960b29db` |

The PDF also matched for a clip-only share, a slow provider, and a retry after cancelling. The fixture app and test captures were removed; SEAM stayed installed. This is an Android intake check, not a network transfer or broad provider compatibility result.

## Next gate

Implement and verify a manually addressed, authenticated phone-to-PC transfer with receiver-confirmed publication. Keep the development CLI loopback-only. The release gate still requires transfers on Windows, macOS and Linux, explicit pairing and recovery, accessibility and packaging checks, and an exact-candidate final audit.
