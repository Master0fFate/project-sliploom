# SEAM receive-once protocol — Phase 2

This records the adopted design against baseline `9b5b29f`. It is not implementation evidence or release confirmation. Operational defaults and limits below are planned until reverified in the integrated flow.

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
| Client → server | `0x04` DATA | 1–65536 | Raw data bytes; zero or more DATA frames |
| Client → server | `0x05` END | 40 | Unsigned 64-bit big-endian actual byte count, then 32-byte SHA-256 digest of all DATA payload bytes in order |
| Server → client | `0x06` COMMITTED | 40 | Exact END payload echoed back |

Successful AUTH consumes the invite and admits only that transfer. Reject invalid frame types, lengths, or ordering. No sender filename is transmitted; the receiver chooses the local filename.

## Commit and failure

END is the commit request; the receiver does not wait for read EOF. It verifies the accumulated byte count against END and against BEGIN when the declared count is known, enforces the 64 MiB cap even when the count is unknown, and verifies the SHA-256 digest. Only after syncing the received file and publishing it without overwriting an existing file may the receiver send COMMITTED.

The sender reports success only on a matching COMMITTED. If confirmation fails after END, the outcome is uncertain; do not resend automatically. The receiver preserves a known save even when it cannot confirm it to the sender.

## Planned session limits

The invitation expires after 120 seconds. The authentication intent is at most eight attempts per invitation and a five-second authentication window per attempt. The transfer budget is approximately five minutes. The session is foreground and user-cancellable; cancellation closes the session without undoing an already published save. Reverify these defaults and limits in the integrated flow.

## Acceptance goal

Use a real Android sender and a real Windows receiver on a private LAN to verify that a foreground receive-once session accepts an image and a PDF in separate sessions, saves the exact bytes, and reports the corresponding outcomes. Cross-platform release validation remains outstanding.
