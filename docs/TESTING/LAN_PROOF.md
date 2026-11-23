# SEAM receive-once milestone

**Not released.** The [debug build and manual QR walkthrough](../../README.md#build-the-debug-apps-on-windows) describe `43ea485`. The evidence below is from earlier commits; it does not verify the current QR flow or a phone-to-PC LAN transfer.

## Windows same-host transfer proof

At `e670c7f`, before the QR UX and later fixes, the unmodified Rust `ReceiveSession` and Android `LanTransfer.kt` communicated on the same Windows host. The sender ran in a JVM harness with only Android clock and Base64 wrappers. Each fixture transferred over real TLS 1.3 with both known and unknown stream lengths. In all four runs, the received bytes matched exactly and the sender returned `Saved`.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| PNG | 192323 | `0f289050a270195a0560774162eb7842cd473909844d2b20dad5eaa117df7021` |
| PDF | 607 | `833dd9f18a46680ada3a1ee964b4d84f15e058f594974efac58323b12f238480` |

## Separate baseline checks

- **Pixel 9 Pro XL:** [Physical-device QA](BASELINE_AUDIT.md#physical-device-checks) shared actual files through the Android Sharesheet and verified intake. Nothing was sent; this was not a Pixel-to-PC LAN transfer.
- **Ubuntu under WSL2 (`9b5b29f`):** The native Linux ELF receiver passed a loopback smoke test and the desktop preview opened a visible WSLg window. This was not a test on bare-metal Linux or of the current desktop milestone.

## Release gates remain open

Release still requires the agreed later features, including discovery and explicit pairing, and real Android-to-desktop transfers on Windows, macOS, and Linux. Native macOS and Linux accessibility and packaging checks are still open. No Mac test environment or release signatures are available here. No new build, transfer, or CI results are attached.

[Master plan §15](../MASTER_PLAN.md#15-release-definition-and-metrics) still applies: only after the agreed product scope is implemented and the required quality checks have run does the independent final audit take place, using GPT-6.2 Astra at `medium` for both the audit and final recheck. If unavailable, the gate stays blocked.
