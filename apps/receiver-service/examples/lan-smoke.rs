use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use receiver_service::{
    Result,
    lan::{
        ReceiveSession,
        batch::{BatchOffer, BatchOutcome, BatchStatus},
        sender::SendInvite,
    },
};

fn private_route_ip() -> Result<Ipv4Addr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    probe
        .connect((Ipv4Addr::new(192, 0, 2, 1), 9))
        .map_err(|error| format!("cannot select an IPv4 route for the LAN smoke check: {error}"))?;
    let local = probe.local_addr()?;
    match local {
        SocketAddr::V4(address) if address.ip().is_private() => Ok(*address.ip()),
        _ => {
            Err(format!("LAN smoke check requires an RFC1918 IPv4 route; selected {local}").into())
        }
    }
}

fn main() -> Result<()> {
    let ip = private_route_ip()?;
    let source = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    let folder = fs::canonicalize(destination.path())?;
    let files: [(&str, &[u8]); 2] = [
        ("first.txt", b"Sliploom LAN smoke\n"),
        ("second.bin", b"\0\xff\x01\x80"),
    ];
    let paths = files.map(|(name, _)| source.path().join(name));
    for (path, (_, contents)) in paths.iter().zip(files) {
        fs::write(path, contents)?;
    }
    let expected_offer = BatchOffer {
        count: files.len(),
        max_bytes: files.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
        has_unknown_sizes: false,
    };
    let expected_outcome = BatchOutcome {
        total: files.len(),
        confirmed: files.len(),
        unconfirmed: false,
        status: BatchStatus::Complete,
    };
    let session = ReceiveSession::bind_batch(ip, &folder)
        .map_err(|error| format!("cannot bind LAN receiver on {ip}: {error}"))?;
    let invite = SendInvite::parse(session.invitation()).ok_or("invalid batch invitation")?;
    assert!(invite.is_batch(), "expected a batch invitation");

    let cancelled = &AtomicBool::new(false);
    let (sent, received) = thread::scope(|scope| {
        let (done, wait) = mpsc::channel::<()>();
        let watchdog = scope.spawn(move || {
            let timed_out =
                wait.recv_timeout(Duration::from_secs(30)) == Err(mpsc::RecvTimeoutError::Timeout);
            cancelled.store(true, Ordering::Release);
            timed_out
        });
        let receiver = scope.spawn(move || {
            let mut receipts = Vec::new();
            session
                .receive_batch(
                    cancelled,
                    |offer| offer == expected_offer,
                    |receipt| receipts.push(receipt),
                    |_| {},
                )
                .map(|()| receipts)
                .map_err(|error| error.to_string())
        });
        let sent = invite.send_batch(&paths, cancelled, || {}, |_| {});
        if sent != expected_outcome {
            cancelled.store(true, Ordering::Release);
        }
        let received = receiver.join().expect("LAN receiver panicked");
        drop(done);
        assert!(
            !watchdog.join().expect("watchdog panicked"),
            "LAN smoke transfer exceeded 30 seconds"
        );
        (sent, received)
    });

    assert_eq!(
        sent,
        expected_outcome,
        "unexpected sender outcome; receive error: {:?}",
        received.as_ref().err()
    );
    let receipts = received?;
    assert_eq!(receipts.len(), files.len(), "wrong receipt count");
    for (receipt, (name, contents)) in receipts.iter().zip(files) {
        assert!(receipt.confirmed, "unconfirmed receipt for {name}");
        assert_eq!(
            receipt.bytes,
            contents.len() as u64,
            "wrong byte count for {name}"
        );
        assert_eq!(receipt.path, folder.join(name), "wrong receive path");
        assert_eq!(
            fs::read(&receipt.path)?,
            contents,
            "wrong contents for {name}"
        );
    }
    println!(
        "Confirmed {} same-host Wi-Fi batch files on {ip}",
        receipts.len()
    );
    Ok(())
}
