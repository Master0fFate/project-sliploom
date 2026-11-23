use std::{
    env,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::ExitCode,
    sync::Arc,
};

use receiver_service::{
    ACK, Result, certificates, expect_end, invalid, loopback_address, private_key, read_header,
    set_timeouts, transfer, trust_store,
};
use rustls::{
    ServerConfig, ServerConnection, Stream, server::WebPkiClientVerifier, version::TLS13,
};
use tempfile::NamedTempFile;

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [listen, output, cert, key, client_ca] = args.as_slice() else {
        return Err(
            "usage: receiver-service LISTEN OUTPUT SERVER_CERT SERVER_KEY CLIENT_CA".into(),
        );
    };
    let listen = loopback_address(listen)?;
    let output = Path::new(output);
    if output.file_name().is_none() {
        return Err("an output filename is required".into());
    }
    let directory = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let verifier = WebPkiClientVerifier::builder(Arc::new(trust_store(client_ca)?)).build()?;
    let config = ServerConfig::builder_with_protocol_versions(&[&TLS13])
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates(cert)?, private_key(key)?)?;
    let listener = TcpListener::bind(listen)?;
    println!("LISTEN {}", listener.local_addr()?);
    let (mut socket, peer) = listener.accept()?;
    drop(listener);
    if !peer.ip().is_loopback() {
        return Err("a loopback peer is required".into());
    }
    set_timeouts(&socket)?;
    let mut connection = ServerConnection::new(Arc::new(config))?;
    let mut tls = Stream::new(&mut connection, &mut socket);
    let length = read_header(&mut tls)?;
    let mut stage = NamedTempFile::new_in(directory)?;
    let digest = transfer(&mut tls, stage.as_file_mut(), length)?;
    let mut claimed = [0; 32];
    tls.read_exact(&mut claimed)?;
    if digest != claimed {
        return Err(invalid("SHA-256 mismatch").into());
    }
    expect_end(&mut tls)?;
    stage.as_file().sync_all()?;
    stage.persist_noclobber(output).map_err(|err| err.error)?;
    tls.write_all(&ACK)?;
    tls.write_all(&digest)?;
    tls.conn.send_close_notify();
    tls.flush()?;
    println!("Published {} bytes to {}", length, output.display());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("receiver-service: {err}");
            ExitCode::FAILURE
        }
    }
}
