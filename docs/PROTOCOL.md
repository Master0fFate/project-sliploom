# SEAM receive-once protocol — Phase 2

This specifies original filename support as an extension to the deployed development protocol at `53c3b4f`. It is not implementation evidence or release confirmation.

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

The receiver uses the supplied name only if it is a safe basename, not a path, and its extension is compatible with the received file type. If the name is absent or unsuitable, use a generic document or image name without a `SEAM` prefix. On a collision, add a number before the extension: for example, save `report.pdf` and then `report (2).pdf`. Do not alter any existing file.

Updated receivers accept older senders that omit NAME. Older receivers may reject transfers that include NAME. Update the Android and desktop apps together.

## Commit and failure

END is the commit request; the receiver does not wait for read EOF. It verifies the accumulated byte count against END and against BEGIN when the declared count is known, enforces the 64 MiB cap even when the count is unknown, and verifies the SHA-256 digest. Only after syncing the received file and publishing it without overwriting an existing file may the receiver send COMMITTED.

The sender reports success only on a matching COMMITTED. If confirmation fails after END, the outcome is uncertain; do not resend automatically. The receiver preserves a known save even when it cannot confirm it to the sender.

## Planned session limits

The invitation expires after 120 seconds. The authentication intent is at most eight attempts per invitation and a five-second authentication window per attempt. The transfer budget is approximately five minutes. The session is foreground and user-cancellable; cancellation closes the session without undoing an already published save. Reverify these defaults and limits in the integrated flow.

## Acceptance goal

Use a real Android sender and a real Windows receiver on a private LAN to verify that a foreground receive-once session accepts an image and a PDF in separate sessions, saves the exact bytes, and reports the corresponding outcomes. Cross-platform release validation remains outstanding.
