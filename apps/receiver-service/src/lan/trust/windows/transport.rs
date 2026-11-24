use std::{
    ffi::c_void,
    fmt, io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    ptr,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, DistinguishedName,
    Error, ServerConfig, ServerConnection, SignatureScheme, StreamOwned,
    client::{
        Resumption,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
    server::{
        NoServerSessionStorage, ParsedCertificate,
        danger::{ClientCertVerified, ClientCertVerifier},
    },
    sign::SingleCertAndKey,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{ACK, MAGIC, Trust};
use crate::{
    Result, invalid,
    lan::{AUTH_TIMEOUT, BoundedTcp, Cancellation, CheckedStream, check_deadline},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_LEAF_SIZE: usize = 4096;

pub(super) type ServerStream<'a> = StreamOwned<ServerConnection, BoundedTcp<'a>>;
type ClientStream<'a> = StreamOwned<ClientConnection, BoundedTcp<'a>>;

pub(super) fn server_config(trust: &Trust) -> Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::builder_with_provider(trust.0.provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(VerifyClient {
            trust: trust.clone(),
            algorithms: trust.0.provider.signature_verification_algorithms,
        }))
        .with_cert_resolver(Arc::new(SingleCertAndKey::from(trust.0.key.clone())));
    config.max_early_data_size = 0;
    config.send_tls13_tickets = 0;
    config.session_storage = Arc::new(NoServerSessionStorage {});
    Ok(Arc::new(config))
}

pub(super) fn accept<'a>(
    socket: TcpStream,
    cancel: &'a AtomicBool,
    config: &Arc<ServerConfig>,
) -> Result<ServerStream<'a>> {
    let cancelled = Cancellation {
        worker: cancel,
        connection: None,
    };
    let socket = bound_socket(socket, cancelled, Instant::now() + AUTH_TIMEOUT)?;
    let mut stream = StreamOwned::new(ServerConnection::new(config.clone())?, socket);
    while stream.conn.is_handshaking() {
        stream.check()?;
        stream.conn.complete_io(&mut stream.sock)?;
    }
    stream.check()?;
    Ok(stream)
}

pub(super) fn connect<'a>(
    trust: &Trust,
    address: SocketAddrV4,
    pin: [u8; 32],
    cancelled: Cancellation<'a>,
) -> Result<ClientStream<'a>> {
    let deadline = Instant::now() + AUTH_TIMEOUT;
    check_deadline(cancelled, deadline)?;
    let socket = TcpStream::connect_timeout(&address.into(), CONNECT_TIMEOUT)?;
    let socket = bound_socket(socket, cancelled, deadline)?;
    let mut config = ClientConfig::builder_with_provider(trust.0.provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(VerifyServer {
            pin,
            algorithms: trust.0.provider.signature_verification_algorithms,
        }))
        .with_client_cert_resolver(Arc::new(SingleCertAndKey::from(trust.0.key.clone())));
    config.enable_sni = false;
    config.enable_early_data = false;
    config.resumption = Resumption::disabled();
    let mut stream = StreamOwned::new(
        ClientConnection::new(
            Arc::new(config),
            ServerName::IpAddress((*address.ip()).into()),
        )?,
        socket,
    );
    while stream.conn.is_handshaking() {
        stream.check()?;
        stream.conn.complete_io(&mut stream.sock)?;
    }
    stream.check()?;
    Ok(stream)
}

fn bound_socket(
    socket: TcpStream,
    cancelled: Cancellation<'_>,
    deadline: Instant,
) -> io::Result<BoundedTcp<'_>> {
    socket.set_nonblocking(true)?;
    socket.set_nodelay(true)?;
    Ok(BoundedTcp {
        socket,
        cancelled,
        deadline,
        idle_deadline: deadline,
    })
}

pub(super) fn peer_address(socket: &BoundedTcp<'_>) -> io::Result<Ipv4Addr> {
    match socket.socket.peer_addr()? {
        SocketAddr::V4(address) if address.ip().is_private() => Ok(*address.ip()),
        _ => Err(invalid("invalid trusted connection address")),
    }
}

pub(super) fn write_prelude(stream: &mut impl CheckedStream, op: u8) -> Result<()> {
    let mut prelude = [op; 6];
    prelude[..5].copy_from_slice(MAGIC);
    write(stream, &prelude)
}

pub(super) fn write(stream: &mut impl CheckedStream, bytes: &[u8]) -> Result<()> {
    stream.check()?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

pub(super) fn read_byte(stream: &mut impl CheckedStream) -> Result<u8> {
    stream.check()?;
    let mut byte = [0];
    stream.read_exact(&mut byte)?;
    stream.check()?;
    Ok(byte[0])
}

pub(super) fn read_ack(stream: &mut impl CheckedStream) -> Result<()> {
    if read_byte(stream)? != ACK {
        return Err(invalid("trusted request not approved").into());
    }
    Ok(())
}

pub(super) fn hash(leaf: &CertificateDer<'_>) -> [u8; 32] {
    Sha256::digest(leaf.as_ref()).into()
}

#[derive(Debug)]
struct VerifyServer {
    pin: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for VerifyServer {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, Error> {
        if !bool::from(self.pin.ct_eq(&hash(leaf))) {
            return Err(unauthorized());
        }
        verify_leaf(leaf, chain)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(unauthorized())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

struct VerifyClient {
    trust: Trust,
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for VerifyClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SEAM5 client certificate verifier")
    }
}

impl ClientCertVerifier for VerifyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        leaf: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, Error> {
        let trust = &self.trust;
        let id = hash(leaf);
        {
            let state = trust.state();
            if id == trust.0.id || (state.find(id).is_none() && !state.armed()) {
                return Err(unauthorized());
            }
        }
        verify_leaf(leaf, chain)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(unauthorized())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn unauthorized() -> Error {
    Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}

fn verify_leaf(
    leaf: &CertificateDer<'_>,
    chain: &[CertificateDer<'_>],
) -> std::result::Result<(), Error> {
    if !chain.is_empty() || !(1..=MAX_LEAF_SIZE).contains(&leaf.len()) {
        return Err(unauthorized());
    }
    let _ = ParsedCertificate::try_from(leaf)?;
    let context = unsafe { CertCreateCertificateContext(1, leaf.as_ptr(), leaf.len() as u32) };
    if context.is_null() {
        return Err(Error::InvalidCertificate(CertificateError::BadEncoding));
    }
    let status = unsafe { CertVerifyTimeValidity(ptr::null(), (*context).info) };
    unsafe { CertFreeCertificateContext(context) };
    match status {
        0 => Ok(()),
        -1 => Err(Error::InvalidCertificate(CertificateError::NotValidYet)),
        _ => Err(Error::InvalidCertificate(CertificateError::Expired)),
    }
}

#[repr(C)]
struct CertificateContext {
    encoding: u32,
    bytes: *const u8,
    size: u32,
    info: *const c_void,
    store: *mut c_void,
}

#[link(name = "crypt32")]
unsafe extern "system" {
    fn CertCreateCertificateContext(
        encoding: u32,
        cert: *const u8,
        length: u32,
    ) -> *const CertificateContext;
    fn CertVerifyTimeValidity(time: *const c_void, info: *const c_void) -> i32;
    fn CertFreeCertificateContext(context: *const CertificateContext) -> i32;
}
