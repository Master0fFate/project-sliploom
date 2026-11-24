# Sliploom receive-once protocol — Phase 2

This specifies original filename support as an extension to the deployed development protocol at `53c3b4f`. It is not implementation evidence or release confirmation. Sliploom retains the `SEAM`-based protocol identifiers below for compatibility.

## Invitation and trust

The desktop user explicitly opens one expiring receive session on a chosen RFC 1918 IPv4 address and nonzero port. The receiver binds only that endpoint, makes no automatic firewall changes, and generates a short-lived self-signed P-256 leaf certificate with that address in its IP SAN.

An invite is the literal prefix `SEAM2:` followed by canonical unpadded base64url encoding of exactly 70 bytes, in this order:

| Bytes | Field |
| ---: | --- |
| 4 | RFC 1918 IPv4 address in network byte order |
| 2 | Nonzero port, big-endian |
| 32 | SHA-256 digest of the exact DER-encoded leaf certificate |
| 32 | Single-use invite secret from the OS random source |

Reject noncanonical encodings and invalid endpoint fields. Share the invite out of band, without cloud services or access by others. The secret is a bearer credential, not a device identity. There is no pairing, persistent device identity, or global trust-store change; do not log the invite or secret.

The sender connects to the invited endpoint over TLS 1.3. Before sending the bearer, it verifies the peer's exact leaf pin, certificate validity period, and matching IP SAN, and completes the TLS handshake proving possession of the leaf private key. Do not send the bearer as early data. A separate TLS 1.3 mutual-TLS loopback fixture is not the LAN invitation flow or evidence of LAN acceptance.

## Wire exchange

After TLS, the client starts with the four ASCII bytes `SEAM` followed by byte `0x02`. Each subsequent frame has a one-byte unsigned type and a four-byte unsigned big-endian payload length, followed by exactly that many payload bytes. The order, direction, and lengths are fixed:

| Direction | Type | Payload bytes | Payload |
| --- | --- | ---: | --- |
| Client → server | `0x01` AUTH | 32 | Invite bearer secret |
| Server → client | `0x02` ACCEPT | 0 | Empty; sent only after successful authentication |
| Client → server | `0x03` BEGIN | 9 | One-byte kind (`0x01` image or `0x02` PDF), then unsigned 64-bit big-endian declared byte count (all ones means unknown) |
| Client → server | `0x08` NAME | 1–180 | Optional original basename as raw UTF-8 bytes |
| Client → server | `0x04` DATA | 1–65536 | Raw data bytes; zero or more DATA frames |
| Client → server | `0x05` END | 40 | Unsigned 64-bit big-endian actual byte count, then 32-byte SHA-256 digest of all DATA payload bytes in order |
| Server → client | `0x06` COMMITTED | 40 | Exact END payload echoed back |

Successful AUTH consumes the invite and admits only that transfer. Reject invalid frame types, lengths, or ordering.

## Original filenames

On Wi-Fi, ACCEPT precedes BEGIN. On USB (`SEAM` followed by `0x03`), BEGIN is the offer and the desktop replies with ACCEPT (`0x02`) or DECLINE (`0x07`). On either transport, the sender may send at most one NAME frame after both BEGIN and ACCEPT and before the first DATA frame (or before END if there is no DATA). USB never sends the filename before the user chooses **Accept**. NAME is metadata and does not count toward the file byte count or DATA digest. Do not retain filenames in logs.

The receiver uses the supplied name only if it is a safe basename, not a path, and its extension is compatible with the received file type. If the name is absent or unsuitable, use a generic document or image name without an app-name prefix. On a collision, add a number before the extension: for example, save `report.pdf` and then `report (2).pdf`. Do not alter any existing file.

Updated receivers accept older senders that omit NAME. Older receivers may reject transfers that include NAME. Update the Android and desktop apps together.

## Commit and failure

END is the commit request; the receiver does not wait for read EOF. It verifies the accumulated byte count against END and against BEGIN when the declared count is known, enforces the 64 MiB cap even when the count is unknown, and verifies the SHA-256 digest. Only after syncing the received file and publishing it without overwriting an existing file may the receiver send COMMITTED.

The sender reports success only on a matching COMMITTED. If confirmation fails after END, the outcome is uncertain; do not resend automatically. The receiver preserves a known save even when it cannot confirm it to the sender.

## Planned session limits

The invitation expires after 120 seconds. The authentication intent is at most eight attempts per invitation and a five-second authentication window per attempt. The transfer budget is approximately five minutes. The session is foreground and user-cancellable; cancellation closes the session without undoing an already published save. Reverify these defaults and limits in the integrated flow.

## Acceptance goal

Use a real Android sender and a real Windows receiver on a private LAN to verify that a foreground receive-once session accepts an image and a PDF in separate sessions, saves the exact bytes, and reports the corresponding outcomes. Cross-platform release validation remains outstanding.

## Wi-Fi batch protocol (SEAM4)

SEAM4 uses the same 70-byte invitation layout and TLS 1.3 validation as SEAM2, with the `SEAM4:` prefix and `SEAM\x04` prelude. The frame header remains one type byte and a big-endian `u32` payload length. Match the invitation to its protocol version. Never fall back to another version on a failed handshake. The USB debugging relay remains SEAM3.

AUTH consumes the invitation. The resulting empty `0x02` ACCEPT confirms authentication only. The sender freezes the ordered list and sends one offer. In the apps, clicking Receive once authorizes one valid batch from the holder of that fresh code; the receiver sends APPROVE after validating the offer. A receiver can instead prompt for the offer without changing the wire format. Neither flow remembers the device or approves a later transfer.

| Direction | Type | Payload |
| --- | --- | --- |
| Client → server | `0x09` OFFER | 1–32 ordered big-endian `u64` lengths. All ones means unknown. |
| Server → client | `0x0A` APPROVE or `0x07` DECLINE | Empty. No filenames, content or staged destination files before APPROVE. |
| Client → server | `0x08` NAME | 0–180 UTF-8 bytes. Empty means no usable basename. |
| Client → server | `0x04` DATA | 1–65,536 bytes, repeated as needed. |
| Client → server | `0x05` END | 42 bytes: zero-based `u16` item index, `u64` actual size, then the SHA-256 of that item's DATA. The integers are big-endian. |
| Server → client | `0x06` COMMITTED | Exact 42-byte END payload, echoed after the corresponding save. |

Allow up to 64 MiB per item and 256 MiB per offer. Reserve 64 MiB for each unknown length; show the resulting total as an upper bound. Keep cancellation and the overall deadline active during the approval window, which is capped at 60 seconds. An expired approval must not admit data.

After approval, stream NAME, DATA and END for each item in order; wait for its COMMITTED before starting the next item. Check actual EOF on the sender, and verify the index, size and digest before publishing on the receiver. Save regular files as opaque bytes, regardless of extension. Treat malformed UTF-8 as a protocol error, replace unsuitable basenames with `file`, and add numeric suffixes before a safe extension (or at the end of an extensionless name) to avoid overwrites. Do not open received files automatically.

The receiver preserves any file already published, even if sending its confirmation fails. The sender counts only exact matching receipts as confirmed. An interrupted item is unconfirmed if END had started; later items were not sent. No automatic retry or rollback of earlier saves.

## Trusted devices (SEAM5)

Trusted devices use a separate SEAM5 control channel. An ordinary SEAM4 receive QR authorizes one batch and never enrolls a peer.

- **Identity:** Android uses an Android Keystore P-256 key; Windows protects its private key at rest with current-user DPAPI. Trust belongs to the exact enrolled certificate and proof of its private key in mutual TLS 1.3. A label, IP address, USB serial or ADB attachment does not count. A key change requires fresh pairing.
- **Pairing:** Open a separate short-lived, single-use SEAM5 QR. The scanner pins the server certificate before sharing pairing data. Both people compare the same six-digit value, derived from the pairing nonce and both ordered certificate hashes, and explicitly allow incoming batches while Sliploom is open. If pairing is interrupted and only one side remembers the other, forget that entry and pair again.
- **Availability:** Probe only saved IPv4 endpoint hints. The peer appears available after an authenticated readiness reply, not after finding the same label. A stale address can require a fresh pairing QR. A connection can reveal the stable TLS leaf to a host on the LAN; Sliploom does not broadcast device names.
- **Transfer:** Selecting a paired device and tapping Send opens a fresh mutual-TLS connection. An authorized receiver reserves one bounded SEAM4 session and returns its invitation privately over the encrypted control channel. The data socket still authenticates with that single-use bearer, not a client certificate. Windows saves to `%LOCALAPPDATA%\Sliploom` by default; Android saves to Received files. Neither app opens received content automatically. Forget revokes future access and aborts an active grant without deleting completed files.
- **Cable:** The SEAM3 debug relay and native MTP do not establish an enrolled app identity. The Wi-Fi grant cannot be used to bypass their authorization.

Changing the Windows receive default does not move existing received files. Existing trusted-device state remains usable.

The foreground control listener uses private IPv4 TCP port 48162. Its prelude is `SEAM\x05`, followed by one operation byte: `0x01` PAIR, `0x02` PING or `0x03` OPEN. A PAIR request carries the 32-byte QR nonce and one platform byte (Windows `1`, Android `2`). The server sends `0x06` and its platform byte before confirmation. Each side explicitly confirms the six-digit value derived from `SHA-256("SEAM5 pair\x00" || nonce || serverHash || clientHash)`: interpret the first four bytes as an unsigned big-endian integer and reduce modulo 1,000,000. A successful PING returns `0x06`; a successful OPEN returns `0x06` followed by the 100-byte ASCII SEAM4 invitation. A refusal returns `0x15` or closes the connection. No operation may fall back to unauthenticated traffic.

Verify correct pairing, a saved-device reconnect, revocation, a real multi-item send, and refusal of unknown or changed keys before release.
