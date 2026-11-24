use std::{
    error::Error,
    ffi::OsStr,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    time::Duration,
};

use rustls::{
    RootCertStore,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use sha2::{Digest, Sha256};

pub mod lan;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);
pub const ACK: [u8; 4] = *b"ACK\x01";
const VERSION: [u8; 5] = *b"SEAM\x01";
const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn loopback_address(value: &OsStr) -> Result<SocketAddr> {
    let address: SocketAddr = value.to_str().ok_or("invalid socket address")?.parse()?;
    if !address.ip().is_loopback() {
        return Err("a numeric loopback address is required".into());
    }
    Ok(address)
}

pub fn set_timeouts(socket: &TcpStream) -> io::Result<()> {
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    socket.set_write_timeout(Some(SOCKET_TIMEOUT))
}

pub fn certificates(path: &OsStr) -> Result<Vec<CertificateDer<'static>>> {
    let certs = CertificateDer::pem_file_iter(Path::new(path))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err("empty PEM certificate file".into());
    }
    Ok(certs)
}

pub fn private_key(path: &OsStr) -> Result<PrivateKeyDer<'static>> {
    Ok(PrivateKeyDer::from_pem_file(Path::new(path))?)
}

pub fn trust_store(path: &OsStr) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in certificates(path)? {
        roots.add(cert)?;
    }
    Ok(roots)
}

pub fn check_length(length: u64) -> io::Result<()> {
    if length > MAX_BYTES {
        return Err(invalid("file exceeds 64 MiB"));
    }
    Ok(())
}

pub fn read_header(mut input: impl Read) -> io::Result<u64> {
    let mut version = [0; VERSION.len()];
    input.read_exact(&mut version)?;
    if version != VERSION {
        return Err(invalid("unsupported Sliploom envelope"));
    }
    let mut length = [0; 8];
    input.read_exact(&mut length)?;
    let length = u64::from_be_bytes(length);
    check_length(length)?;
    Ok(length)
}

pub fn write_header(mut output: impl Write, length: u64) -> io::Result<()> {
    output.write_all(&VERSION)?;
    output.write_all(&length.to_be_bytes())
}

pub fn transfer(
    mut input: impl Read,
    mut output: impl Write,
    mut length: u64,
) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    while length != 0 {
        let chunk = length.min(buffer.len() as u64) as usize;
        let bytes = &mut buffer[..chunk];
        input.read_exact(bytes)?;
        output.write_all(bytes)?;
        digest.update(bytes);
        length -= chunk as u64;
    }
    Ok(digest.finalize().into())
}

pub fn expect_end(mut input: impl Read) -> io::Result<()> {
    let mut extra = [0];
    if input.read(&mut extra)? != 0 {
        return Err(invalid("unexpected trailing bytes"));
    }
    Ok(())
}
