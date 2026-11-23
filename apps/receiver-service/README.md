# SEAM localhost file proof

Build from the workspace root with `cargo build --locked` (Rust 1.99.0, Windows x64 MSVC). Run the receiver as a bounded background child, then run the sender using the address printed in the receiver's `LISTEN` line:

```text
target/debug/receiver-service.exe LISTEN OUTPUT SERVER_CERT SERVER_KEY CLIENT_CA
target/debug/dev-sender.exe TARGET TLS_NAME INPUT CLIENT_CERT CLIENT_KEY SERVER_CA
```

- `LISTEN` and `TARGET` are numeric loopback socket addresses, such as `127.0.0.1:43123` or `[::1]:43123`. `LISTEN` can use port `0` to choose an available port. Start a fresh receiver for every attempt; it accepts one connection and exits.
- `OUTPUT` is a new filename in an existing private directory controlled by the receiver (no untrusted write access). `INPUT` is an unchanged regular file. Quote paths containing spaces.
- Use disposable test credentials only. Each `*_CERT` is a PEM leaf certificate (followed by any intermediates) and `*_KEY` is its corresponding PEM private key. Use a server leaf with `serverAuth` and a SAN matching `TLS_NAME` (for example `DNS:localhost`), and a client leaf with `clientAuth`. Each `*_CA` PEM file contains only the deliberately trusted test CA certificates for that peer, preferably separate CA roots for the server and client. No system trust, automatic enrollment, or bypass of TLS name verification. Keep credentials and files in a private temporary directory, then delete it.
- TLS 1.3 mutual authentication is required. Connect timeout and socket read/write timeouts are 10 seconds each, not an overall deadline; the listener can wait for its first connection. Set a deadline on the child process when running a smoke check.

## Envelope v1

Over TLS, the sender writes `53 45 41 4d 01` (`SEAM` and version byte `1`), an unsigned 64-bit big-endian payload length, exactly that many file bytes, and the 32 raw SHA-256 bytes of the payload. It then sends TLS `close_notify` on its write side. The receiver requires that authenticated end-of-stream with no extra data. A session carries one file of 0–64 MiB, inclusive, streamed through a 16 KiB application buffer. The sender supplies no destination name.

The receiver stages data beside `OUTPUT`; after length and digest verification it syncs the stage and publishes without replacing an existing path. Normal pre-publication errors discard the stage. After publication it sends `41 43 4b 01` (`ACK` and version byte `1`), the verified 32-byte digest, and TLS `close_notify`. The sender reports success only after verifying this complete response. A lost ACK can leave a published file without sender confirmation. Process termination can leave a stage; crash durability is not guaranteed.

This is a local development proof, not a LAN listener, pairing workflow, or Android sender.
