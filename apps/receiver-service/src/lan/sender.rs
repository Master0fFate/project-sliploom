use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
    time::{Instant, SystemTime},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, Error, RootCertStore,
    SignatureScheme, StreamOwned,
    client::{
        Resumption,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        verify_server_cert_signed_by_trust_anchor, verify_server_name,
    },
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
    server::ParsedCertificate,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{
    AUTH_TIMEOUT, BoundedTcp, Cancellation, CheckedStream, IDLE_TIMEOUT, MAX_CHUNK, MAX_NAME_BYTES,
    POLL_INTERVAL, TRANSFER_TIMEOUT, WifiVersion,
    batch::{self, BatchOutcome, BatchStatus, MAX_BATCH_BYTES, MAX_FILES},
    check_deadline, filename, frame_header, read_frame, write_frame,
};
use crate::{MAX_BYTES, invalid};

pub struct SendInvite {
    endpoint: SocketAddrV4,
    pin: [u8; 32],
    token: [u8; 32],
    version: WifiVersion,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Saved(u64),
    Unconfirmed,
    Cancelled,
    Failed,
    TooLarge,
    Unsupported,
}

impl SendInvite {
    pub fn parse(value: &str) -> Option<Self> {
        let (version, encoded) = [WifiVersion::Batch, WifiVersion::Legacy]
            .into_iter()
            .find_map(|version| Some((version, value.strip_prefix(version.prefix())?)))?;
        let mut packed = [0; 70];
        if encoded.len() != 94
            || URL_SAFE_NO_PAD.decode_slice(encoded, &mut packed).ok()? != 70
            || URL_SAFE_NO_PAD.encode(packed) != encoded
        {
            return None;
        }
        let address = Ipv4Addr::new(packed[0], packed[1], packed[2], packed[3]);
        let port = u16::from_be_bytes([packed[4], packed[5]]);
        if !address.is_private() || port == 0 {
            return None;
        }
        Some(Self {
            endpoint: SocketAddrV4::new(address, port),
            pin: packed[6..38].try_into().ok()?,
            token: packed[38..].try_into().ok()?,
            version,
        })
    }

    pub fn is_batch(&self) -> bool {
        self.version == WifiVersion::Batch
    }

    pub fn address(&self) -> Ipv4Addr {
        *self.endpoint.ip()
    }

    pub fn send(
        &self,
        path: &Path,
        cancel: &AtomicBool,
        on_connected: impl FnOnce(),
        mut on_progress: impl FnMut(u64),
    ) -> SendOutcome {
        if self.is_batch() {
            return SendOutcome::Unsupported;
        }
        let cancelled = Cancellation {
            worker: cancel,
            connection: None,
        };
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let mut end_started = false;
        let result = (|| -> io::Result<SendOutcome> {
            check_deadline(cancelled, deadline)?;
            let Some(mut file) = open_source(path)? else {
                return Ok(SendOutcome::Unsupported);
            };
            let metadata = file.metadata()?;
            if !ordinary_file(&metadata) {
                return Ok(SendOutcome::Unsupported);
            }
            let length = metadata.len();
            if length > MAX_BYTES {
                return Ok(SendOutcome::TooLarge);
            }
            check_deadline(cancelled, deadline)?;
            let mut prefix = [0; 12];
            let prefix = &mut prefix[..length.min(12) as usize];
            file.read_exact(prefix)?;
            let name = path.file_name().and_then(|name| name.to_str());
            let Some(kind) = FileKind::detect(name, prefix) else {
                return Ok(SendOutcome::Unsupported);
            };

            let mut stream = self.connect(cancelled, deadline)?;
            on_connected();
            stream.check()?;
            let mut begin = [0; 9];
            begin[0] = kind as u8;
            begin[1..].copy_from_slice(&length.to_be_bytes());
            write_frame(&mut stream, 0x03, &begin)?;
            if let Some(name) = compatible_name(name, kind, prefix) {
                write_frame(&mut stream, 0x08, name.as_bytes())?;
            }

            let mut input = (&*prefix).chain(&mut file);
            let mut chunk = [0; MAX_CHUNK];
            let mut bytes = 0;
            let mut digest = Sha256::new();
            while bytes < length {
                stream.check()?;
                let count = (length - bytes).min(MAX_CHUNK as u64) as usize;
                let data = &mut chunk[..count];
                input.read_exact(data)?;
                stream.check()?;
                write_frame(&mut stream, 0x04, data)?;
                digest.update(data);
                bytes += count as u64;
                on_progress(bytes);
            }
            stream.check()?;
            if input.read(&mut chunk[..1])? != 0 {
                return if length == MAX_BYTES || file.metadata()?.len() > MAX_BYTES {
                    Ok(SendOutcome::TooLarge)
                } else {
                    Err(invalid("source length changed"))
                };
            }

            let mut end = [0; 40];
            end[..8].copy_from_slice(&bytes.to_be_bytes());
            end[8..].copy_from_slice(&digest.finalize());
            stream.check()?;
            end_started = true;
            write_frame(&mut stream, 0x05, &end)?;
            if read_frame::<40>(&mut stream, 0x06)? != end {
                return Err(invalid("transfer confirmation did not match"));
            }
            Ok(SendOutcome::Saved(bytes))
        })();

        match result {
            Ok(saved @ SendOutcome::Saved(_)) => saved,
            _ if end_started => SendOutcome::Unconfirmed,
            _ if cancelled.is_cancelled() => SendOutcome::Cancelled,
            Ok(outcome) => outcome,
            Err(_) => SendOutcome::Failed,
        }
    }

    pub fn send_batch(
        &self,
        paths: &[PathBuf],
        cancel: &AtomicBool,
        on_connected: impl FnOnce(),
        mut on_progress: impl FnMut(u64),
    ) -> BatchOutcome {
        let mut outcome = BatchOutcome {
            total: paths.len(),
            confirmed: 0,
            unconfirmed: false,
            status: BatchStatus::Failed,
        };
        let cancelled = Cancellation {
            worker: cancel,
            connection: None,
        };
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let result = (|| -> io::Result<BatchStatus> {
            check_deadline(cancelled, deadline)?;
            if paths.is_empty() {
                return Ok(BatchStatus::Unsupported);
            }
            if !self.is_batch() {
                let [path] = paths else {
                    return Ok(BatchStatus::Unsupported);
                };
                return Ok(match self.send(path, cancel, on_connected, on_progress) {
                    SendOutcome::Saved(_) => {
                        outcome.confirmed = 1;
                        BatchStatus::Complete
                    }
                    SendOutcome::Unconfirmed => {
                        outcome.unconfirmed = true;
                        BatchStatus::Failed
                    }
                    SendOutcome::Cancelled => BatchStatus::Cancelled,
                    SendOutcome::Failed => BatchStatus::Failed,
                    SendOutcome::TooLarge => BatchStatus::TooLarge,
                    SendOutcome::Unsupported => BatchStatus::Unsupported,
                });
            }
            if paths.len() > MAX_FILES {
                return Ok(BatchStatus::TooLarge);
            }

            let mut sources = Vec::with_capacity(paths.len());
            let mut payload = Vec::with_capacity(paths.len() * 8);
            let mut reservation = 0;
            for path in paths {
                check_deadline(cancelled, deadline)?;
                let Some(source) = BatchSource::open(path)? else {
                    return Ok(BatchStatus::Unsupported);
                };
                if source.length > MAX_BYTES || reservation + source.length > MAX_BATCH_BYTES {
                    return Ok(BatchStatus::TooLarge);
                }
                reservation += source.length;
                payload.extend_from_slice(&source.length.to_be_bytes());
                sources.push(source);
            }

            let mut stream = self.connect(cancelled, deadline)?;
            on_connected();
            write_frame(&mut stream, batch::BATCH_OFFER, &payload)?;
            if !wait_for_approval(&mut stream)? {
                return Ok(BatchStatus::Declined);
            }

            let mut bytes = 0;
            for (index, mut source) in sources.into_iter().enumerate() {
                let end = source.send_data(&mut stream, index as u16, |n| {
                    on_progress(bytes + n);
                })?;
                stream.check()?;
                outcome.unconfirmed = true;
                write_frame(&mut stream, batch::END, &end)?;
                if batch::read_committed(&mut stream)? != end {
                    return Err(invalid("transfer confirmation did not match"));
                }
                outcome.unconfirmed = false;
                outcome.confirmed += 1;
                bytes += source.length;
            }
            Ok(BatchStatus::Complete)
        })();
        outcome.status = result.unwrap_or_else(|_| {
            if cancelled.is_cancelled() {
                BatchStatus::Cancelled
            } else {
                BatchStatus::Failed
            }
        });
        outcome
    }

    fn connect<'a>(
        &self,
        cancelled: Cancellation<'a>,
        deadline: Instant,
    ) -> io::Result<ClientStream<'a>> {
        let auth_deadline = deadline.min(Instant::now() + AUTH_TIMEOUT);
        check_deadline(cancelled, auth_deadline)?;
        let endpoint = self.endpoint.into();
        let socket = connect_tcp(endpoint, cancelled, auth_deadline)?;
        socket.set_nonblocking(true)?;
        socket.set_nodelay(true)?;

        let provider = rustls::crypto::ring::default_provider();
        let verifier = PinnedLeaf {
            pin: self.pin,
            algorithms: provider.signature_verification_algorithms,
        };
        let mut config = ClientConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(io::Error::other)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        config.enable_early_data = false;
        config.resumption = Resumption::disabled();
        let host = ServerName::IpAddress(self.address().into());
        let mut stream = StreamOwned::new(
            ClientConnection::new(Arc::new(config), host).map_err(io::Error::other)?,
            BoundedTcp {
                socket,
                cancelled,
                deadline: auth_deadline,
                idle_deadline: auth_deadline,
            },
        );
        while stream.conn.is_handshaking() {
            stream.check()?;
            stream.conn.complete_io(&mut stream.sock)?;
        }
        stream.check()?;
        stream.write_all(&self.version.prelude())?;
        write_frame(&mut stream, 0x01, &self.token)?;
        read_frame::<0>(&mut stream, 0x02)?;
        stream.sock.deadline = deadline;
        stream.sock.idle_deadline = Instant::now() + IDLE_TIMEOUT;
        Ok(stream)
    }
}

fn connect_tcp(
    endpoint: std::net::SocketAddr,
    cancelled: Cancellation<'_>,
    deadline: Instant,
) -> io::Result<TcpStream> {
    let (send, recv) = mpsc::channel();
    thread::Builder::new().spawn(move || {
        let timeout = deadline.saturating_duration_since(Instant::now());
        if !timeout.is_zero() {
            let _ = send.send(TcpStream::connect_timeout(&endpoint, timeout));
        }
    })?;
    loop {
        check_deadline(cancelled, deadline)?;
        match recv.recv_timeout(POLL_INTERVAL) {
            Ok(result) => {
                check_deadline(cancelled, deadline)?;
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(error) => return Err(io::Error::other(error)),
        }
    }
}

type ClientStream<'a> = StreamOwned<ClientConnection, BoundedTcp<'a>>;

impl CheckedStream for ClientStream<'_> {
    fn check(&self) -> io::Result<()> {
        self.sock.check()
    }
}

fn wait_for_approval(stream: &mut ClientStream<'_>) -> io::Result<bool> {
    let deadline = stream.sock.deadline;
    let approval_deadline = Instant::now() + batch::APPROVAL_TIMEOUT;
    stream.sock.deadline = deadline.min(approval_deadline);
    stream.sock.idle_deadline = stream.sock.deadline;
    let decision = frame_header(stream)?;
    stream.check()?;
    stream.sock.deadline = deadline;
    stream.sock.idle_deadline = Instant::now() + IDLE_TIMEOUT;
    match decision {
        (batch::APPROVE, 0) => Ok(true),
        (batch::DECLINE, 0) => Ok(false),
        _ => Err(invalid("unexpected batch approval frame")),
    }
}

struct BatchSource {
    file: File,
    length: u64,
    modified: SystemTime,
    name: String,
}

impl BatchSource {
    fn open(path: &Path) -> io::Result<Option<Self>> {
        let Some(file) = open_source(path)? else {
            return Ok(None);
        };
        let metadata = file.metadata()?;
        if !ordinary_file(&metadata) {
            return Ok(None);
        }
        let name =
            path.file_name()
                .and_then(|name| name.to_str())
                .map_or_else(String::new, |name| {
                    let (stem, extension) = filename::batch_parts(name);
                    extension.map_or_else(
                        || stem.to_owned(),
                        |extension| format!("{stem}.{extension}"),
                    )
                });
        Ok(Some(Self {
            file,
            length: metadata.len(),
            modified: metadata.modified()?,
            name,
        }))
    }

    fn check_unchanged(&self) -> io::Result<()> {
        let metadata = self.file.metadata()?;
        if metadata.len() != self.length || metadata.modified()? != self.modified {
            return Err(invalid("source file changed"));
        }
        Ok(())
    }

    fn send_data(
        &mut self,
        stream: &mut impl CheckedStream,
        index: u16,
        mut progress: impl FnMut(u64),
    ) -> io::Result<[u8; batch::END_LEN]> {
        stream.check()?;
        self.check_unchanged()?;
        write_frame(stream, batch::NAME, self.name.as_bytes())?;
        let mut chunk = [0; MAX_CHUNK];
        let mut bytes = 0;
        let mut digest = Sha256::new();
        while bytes < self.length {
            stream.check()?;
            let count = (self.length - bytes).min(MAX_CHUNK as u64) as usize;
            let data = &mut chunk[..count];
            self.file.read_exact(data)?;
            write_frame(stream, batch::DATA, data)?;
            digest.update(data);
            bytes += count as u64;
            progress(bytes);
        }
        stream.check()?;
        if self.file.read(&mut chunk[..1])? != 0 {
            return Err(invalid("source length changed"));
        }
        self.check_unchanged()?;
        Ok(batch::end_payload(index, bytes, digest.finalize().into()))
    }
}

fn ordinary_file(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    if unsafe_source_attributes(metadata) {
        return false;
    }
    metadata.is_file() && !metadata.file_type().is_symlink()
}

#[cfg(not(windows))]
fn open_source(path: &Path) -> io::Result<Option<File>> {
    if !ordinary_file(&fs::symlink_metadata(path)?) {
        return Ok(None);
    }
    File::open(path).map(Some)
}

#[cfg(windows)]
fn unsafe_source_attributes(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & (FILE_ATTRIBUTE_DEVICE | FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(windows)]
fn open_source(path: &Path) -> io::Result<Option<File>> {
    use std::{
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        path::{Component, Prefix},
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetFileType(handle: std::os::windows::io::RawHandle) -> u32;
    }

    if path.components().any(|component| match component {
        Component::Prefix(prefix) => {
            !path.has_root() || !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        }
        Component::ParentDir => true,
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            name.contains([':', '/', '\\', '*', '?', '<', '>', '|', '"', '\0'])
                || name.ends_with(['.', ' '])
                || filename::reserved_device(&name)
        }
        _ => false,
    }) {
        return Ok(None);
    }
    let path = std::path::absolute(path)?;
    let mut components = path.components();
    let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) =
        (components.next(), components.next())
    else {
        return Ok(None);
    };
    let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix.kind() else {
        return Ok(None);
    };
    let root = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
    if !matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5 | 6) {
        return Ok(None);
    }
    let mut directories = path.ancestors().skip(1).collect::<Vec<_>>();
    directories.reverse();
    for directory in directories.iter().copied().filter(|path| path.has_root()) {
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || unsafe_source_attributes(&metadata) {
            return Ok(None);
        }
    }
    if !ordinary_file(&fs::symlink_metadata(&path)?) {
        return Ok(None);
    }

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_TYPE_DISK: u32 = 1;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return Ok(None);
    }
    Ok(Some(file))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Image = 0x01,
    Pdf = 0x02,
}

impl FileKind {
    fn detect(name: Option<&str>, prefix: &[u8]) -> Option<Self> {
        if prefix.starts_with(b"%PDF-") {
            return Some(Self::Pdf);
        }
        if filename::parts(None, false, prefix).1 != "bin" {
            return Some(Self::Image);
        }
        let (_, extension) = name?.rsplit_once('.')?;
        match extension.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "png" | "apng" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico"
            | "avif" | "heic" | "heif" => Some(Self::Image),
            _ => None,
        }
    }
}

fn compatible_name<'a>(name: Option<&'a str>, kind: FileKind, prefix: &[u8]) -> Option<&'a str> {
    let name = name.filter(|name| (1..=MAX_NAME_BYTES).contains(&name.len()))?;
    let (stem, extension) = filename::parts(Some(name), kind == FileKind::Pdf, prefix);
    (name == stem
        || name
            .strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('.'))
            == Some(extension))
    .then_some(name)
}

#[derive(Debug)]
struct PinnedLeaf {
    pin: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedLeaf {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if !intermediates.is_empty() || !bool::from(self.pin.ct_eq(&Sha256::digest(leaf.as_ref())))
        {
            return Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        let mut roots = RootCertStore::empty();
        roots.add(leaf.clone())?;
        let cert = ParsedCertificate::try_from(leaf)?;
        verify_server_cert_signed_by_trust_anchor(
            &cert,
            &roots,
            intermediates,
            now,
            self.algorithms.all,
        )?;
        verify_server_name(&cert, server_name)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
