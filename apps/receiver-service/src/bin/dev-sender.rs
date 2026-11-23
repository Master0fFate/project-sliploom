use std::{
    env,
    fs::File,
    io::{self, Read, Write},
    net::TcpStream,
    process::ExitCode,
    sync::Arc,
};

use receiver_service::{
    ACK, Result, SOCKET_TIMEOUT, certificates, check_length, expect_end, invalid, loopback_address,
    private_key, set_timeouts, transfer, trust_store, write_header,
};
use rustls::{ClientConfig, ClientConnection, Stream, pki_types::ServerName, version::TLS13};

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [target, name, input, cert, key, server_ca] = args.as_slice() else {
        return Err(
            "usage: dev-sender TARGET TLS_NAME INPUT CLIENT_CERT CLIENT_KEY SERVER_CA".into(),
        );
    };
    let target = loopback_address(target)?;
    let name = ServerName::try_from(name.to_str().ok_or("invalid TLS name")?.to_owned())?;
    let config = ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(trust_store(server_ca)?)
        .with_client_auth_cert(certificates(cert)?, private_key(key)?)?;
    let mut input = File::open(input)?;
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        return Err("input must be a regular file".into());
    }
    let length = metadata.len();
    check_length(length)?;
    let mut socket = TcpStream::connect_timeout(&target, SOCKET_TIMEOUT)?;
    set_timeouts(&socket)?;
    let mut connection = ClientConnection::new(Arc::new(config), name)?;
    let mut tls = Stream::new(&mut connection, &mut socket);
    write_header(&mut tls, length)?;
    let digest = transfer(&mut input, &mut tls, length)?;
    expect_end(&mut input)?;
    confirm_publication(&mut tls, &digest)
        .map_err(|_| "Publication unconfirmed; check the destination before retrying")?;
    println!("Receiver verified and published {length} bytes");
    Ok(())
}

fn confirm_publication(
    tls: &mut Stream<'_, ClientConnection, TcpStream>,
    digest: &[u8; 32],
) -> io::Result<()> {
    tls.write_all(digest)?;
    tls.conn.send_close_notify();
    tls.flush()?;
    let mut ack = [0; ACK.len()];
    let mut received = [0; 32];
    tls.read_exact(&mut ack)?;
    tls.read_exact(&mut received)?;
    if ack != ACK || received != *digest {
        return Err(invalid("invalid publish acknowledgment"));
    }
    expect_end(tls)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dev-sender: {err}");
            ExitCode::FAILURE
        }
    }
}
