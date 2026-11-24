# SEAM receive-once milestone

**Not released.** The receive-once results below cover the earlier SEAM2 single-file milestone. See the [current batch walkthrough](../../README.md#send-and-receive-batches-over-wi-fi).

## Device-to-device baseline

On Windows, the production Rust sender and receiver completed a three-file SEAM4 batch on the same host over a private IPv4 interface. The synthetic text file, 65,539-byte binary file and empty file matched byte for byte, and the sender confirmed all three receipts. The receiver kept the original file when the text filename already existed. In a separate declined session, it saved nothing.

This used ephemeral invitations in memory. It did not exercise either app’s UI, two devices, firewall behavior, or trusted pairing.

## SEAM4 Windows backend smoke

On Windows, two instances of the production Rust Wi-Fi transfer code used a fresh invitation and a private IPv4 interface with synthetic files. One approval accepted three arbitrary files, including an empty file. All three matched their source bytes and SHA-256 digests; an existing name was preserved. An 11-second approval delay did not trip the normal idle timeout. Decline wrote nothing, and cancellation preserved one confirmed file without claiming any later saves. This was a same-host protocol check, not a physical Android-to-Windows transfer.

The production Android sender also ran against the Rust receiver in a host JVM with Android clock and Base64 shims. One approval accepted a text file (16 bytes), a binary file (65,539 bytes) and an empty file. The sender verified three receipts and the receiver saved exactly those files with matching bytes. This exercises the Android sender protocol, not the Android picker, receiver, or device networking.

## Pixel to Windows over Wi-Fi

A physical Pixel 9 Pro XL shared a generated PDF through the Android Sharesheet. The user selected Wi-Fi, scanned a fresh QR from Windows SEAM, and explicitly tapped Send. Android reported “Saved on computer.” The Windows debug build based on `103410d` with the desktop button update reported a confirmed save and returned to Receive once. The empty test folder ended with just `seam-cable-test.pdf` (596 bytes, SHA-256 `39b48a3717afedb8b619231ef9705df28937a5f9b4063b7f3ed8469693eb083f`), matching the source. The PC was listening on its Wi-Fi interface, not the developer cable receiver.

The first scan worked but left a visibly empty manual code field, which obscured the next step. After the Android update, the user repeated the live QR scan and confirmed “Ready to send” before tapping Send. Both apps confirmed the second save as `seam-cable-test (2).pdf`; both files were still 596 bytes and matched the source SHA-256. The manual-code fallback and transient connection state were not exercised. No private invitation was recorded.

## Windows same-host transfer proof

At `e670c7f`, before the QR UX and later fixes, the unmodified Rust `ReceiveSession` and Android `LanTransfer.kt` communicated on the same Windows host. The sender ran in a JVM harness with only Android clock and Base64 wrappers. Each fixture transferred over real TLS 1.3 with both known and unknown stream lengths. In all four runs, the received bytes matched exactly and the sender returned `Saved`.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| PNG | 192323 | `0f289050a270195a0560774162eb7842cd473909844d2b20dad5eaa117df7021` |
| PDF | 607 | `833dd9f18a46680ada3a1ee964b4d84f15e058f594974efac58323b12f238480` |

## Separate baseline checks

- **Pixel 9 Pro XL:** [Physical-device QA](BASELINE_AUDIT.md#physical-device-checks) shared actual files through the Android Sharesheet and verified intake. Nothing was sent; this was not a Pixel-to-PC LAN transfer.
- **Ubuntu under WSL2 (`9b5b29f`):** The native Linux ELF receiver passed a loopback smoke test and the desktop preview opened a visible WSLg window. This was not a test on bare-metal Linux or of the current desktop milestone.

## Not yet verified

The records above do not establish a current physical batch transfer in both directions, Android-to-Android transfers, or transfers between two desktops. Native macOS and Linux runtime testing remains open. [Build and packaging checks](../RELEASE_PIPELINE.md) do not cover these routes.
