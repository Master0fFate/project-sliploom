# SEAM localhost file proof

Build from the workspace root with `cargo build --locked`. The toolchain pins Rust 1.99.0 without forcing a host. Windows x64 MSVC is tested; other hosts are unverified. Run the receiver as a bounded background child, then run the sender using the address printed in the receiver's `LISTEN` line:

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

The receiver stages data beside `OUTPUT`; after length and digest verification it syncs the stage and publishes without replacing an existing path. Normal pre-publication errors discard the stage. After publication it sends `41 43 4b 01` (`ACK` and version byte `1`), the verified 32-byte digest, and TLS `close_notify`. The sender reports success only after verifying this complete response. If sending confirmation fails after publication, the receiver exits nonzero with `File published; confirmation could not be sent` and leaves the file in place. If the sender cannot finish sending the digest and TLS end-of-stream or verify the complete response, it exits nonzero with `Publication unconfirmed; check the destination before retrying`. This does not tell you whether the file was published. Process termination can leave a stage; crash durability is not guaranteed.

This is a local development proof, not a LAN listener, pairing workflow, or Android sender.

## Local CLI smoke check (Windows x64 MSVC)

From the checkout root in PowerShell 7, paste the block below with Python 3 and OpenSSL 3 on `PATH`. It builds the real CLIs and generates short-lived server and client credentials under separate test CAs in a temporary directory. Both attempts send the same benign text fixture and start a fresh receiver on `127.0.0.1:0`.

The readiness deadline is 10 seconds; the sender gets 25 seconds to exit, and the receiver gets up to 5 more seconds after the sender exits. On failure the script exits nonzero, stops any live CLI child, and removes the temporary directory, including credentials. The occupied-output attempt checks a **pre-publication** failure, not loss of confirmation after publication.

```powershell
@'
import hashlib
from pathlib import Path
import re
import subprocess
import tempfile
import time

subprocess.run(["cargo", "build", "--locked", "--target-dir", "target"], check=True)
bins = Path("target/debug").resolve()

with tempfile.TemporaryDirectory(prefix="seam-smoke-") as temp:
    root = Path(temp)

    def openssl(*args):
        subprocess.run(["openssl", *args], cwd=root, check=True, timeout=15)

    for role in ("server", "client"):
        ca = f"{role}-ca"
        openssl("req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
                "-noenc", "-days", "1", "-subj", f"/CN=SEAM Smoke {role} CA",
                "-addext", "basicConstraints=critical,CA:TRUE",
                "-addext", "keyUsage=critical,keyCertSign",
                "-keyout", f"{ca}.key", "-out", f"{ca}.pem")
        openssl("req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
                "-noenc", "-subj", f"/CN=SEAM Smoke {role}",
                "-keyout", f"{role}.key", "-out", f"{role}.csr")
        extensions = f"basicConstraints=critical,CA:FALSE\nextendedKeyUsage={role}Auth\n"
        if role == "server":
            extensions += "subjectAltName=DNS:localhost\n"
        (root / "leaf.ext").write_text(extensions, encoding="ascii")
        openssl("x509", "-req", "-in", f"{role}.csr", "-CA", f"{ca}.pem",
                "-CAkey", f"{ca}.key", "-set_serial", "1", "-days", "1",
                "-extfile", "leaf.ext", "-out", f"{role}.pem")

    fixture = root / "fixture.txt"
    fixture.write_bytes(b"SEAM localhost smoke fixture\n")
    expected_hash = hashlib.sha256(fixture.read_bytes()).hexdigest()
    sentinel = b"Existing output must stay unchanged.\n"

    for case in ("normal", "occupied"):
        dest = root / case
        dest.mkdir()
        output = dest / "published.txt"
        if case == "occupied":
            output.write_bytes(sentinel)
        log_path = root / f"{case}.log"
        with log_path.open("wb") as log:
            receiver = subprocess.Popen([
                bins / "receiver-service.exe", "127.0.0.1:0", output,
                "server.pem", "server.key", "client-ca.pem",
            ], cwd=root, stdout=log, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    log_text = log_path.read_text(encoding="utf-8", errors="replace")
                    ready = re.match(r"LISTEN (127\.0\.0\.1:[1-9][0-9]*)\n", log_text)
                    if ready:
                        break
                    if receiver.poll() is not None:
                        raise RuntimeError(f"receiver exited before readiness: {log_text}")
                    time.sleep(0.05)
                else:
                    raise TimeoutError("receiver did not announce readiness")

                sender = subprocess.run([
                    bins / "dev-sender.exe", ready[1], "localhost", fixture,
                    "client.pem", "client.key", "server-ca.pem",
                ], cwd=root, capture_output=True, text=True, errors="replace", timeout=25)
                receiver_exit = receiver.wait(timeout=5)
            finally:
                if receiver.poll() is None:
                    receiver.kill()
                    receiver.wait(timeout=5)

        receiver_log = log_path.read_text(encoding="utf-8", errors="replace")
        detail = (f"{case}: receiver={receiver_exit}, sender={sender.returncode}\n"
                  f"{receiver_log}{sender.stdout}{sender.stderr}")
        if case == "normal":
            assert (receiver_exit, sender.returncode) == (0, 0), detail
            assert output.stat().st_size == fixture.stat().st_size, "length mismatch"
            actual_hash = hashlib.sha256(output.read_bytes()).hexdigest()
            assert actual_hash == expected_hash, "SHA-256 mismatch"
            print(f"normal: both exit 0, {output.stat().st_size} bytes, SHA-256 {actual_hash}")
        else:
            assert receiver_exit != 0 and sender.returncode != 0, detail
            assert not sender.stdout, detail
            assert "Publication unconfirmed; check the destination before retrying" in sender.stderr, detail
            assert output.read_bytes() == sentinel, "existing output changed"
            assert list(dest.iterdir()) == [output], "staging file left behind"
            print(f"occupied: receiver={receiver_exit}, sender={sender.returncode}, "
                  "sentinel unchanged, no staging file")
'@ | python -I -
if ($LASTEXITCODE -ne 0) { throw 'SEAM CLI smoke failed' }
```
