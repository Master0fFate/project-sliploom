use std::{
    fs,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::{ServerConfig, ServerConnection, StreamOwned, pki_types::PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tempfile::NamedTempFile;
use time::OffsetDateTime;

use crate::{MAX_BYTES, Result, check_length, invalid};

const INVITATION_LIFETIME: Duration = Duration::from_secs(120);
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_ATTEMPTS: usize = 8;
const MAX_CHUNK: usize = 65_536;
const VERSION: [u8; 5] = *b"SEAM\x02";
const ACCEPT: [u8; 5] = [0x02, 0, 0, 0, 0];

pub struct ReceiveSession {
    listener: TcpListener,
    folder: PathBuf,
    invitation: String,
    token: [u8; 32],
    tls: Arc<ServerConfig>,
    expires_at: Instant,
}

pub struct Receipt {
    pub path: PathBuf,
    pub bytes: u64,
    pub confirmed: bool,
}

impl ReceiveSession {
    pub fn bind(address: Ipv4Addr, folder: &Path) -> Result<Self> {
        if !address.is_private() {
            return Err(invalid("an RFC1918 IPv4 address is required").into());
        }
        let folder = destination(folder)?;

        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut params = CertificateParams::new(vec![address.to_string()])?;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::minutes(2);
        params.not_after = now + time::Duration::minutes(58);
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "SEAM");
        let cert = params.self_signed(&key)?;
        let fingerprint = Sha256::digest(cert.der().as_ref());
        let mut tls =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])?
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.into()],
                    PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
                )?;
        tls.max_early_data_size = 0;
        tls.send_tls13_tickets = 0;
        tls.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});

        let token = random::<32>()?;
        let listener = TcpListener::bind((address, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let mut packed = [0; 70];
        packed[..4].copy_from_slice(&address.octets());
        packed[4..6].copy_from_slice(&port.to_be_bytes());
        packed[6..38].copy_from_slice(&fingerprint);
        packed[38..].copy_from_slice(&token);
        let invitation = format!("SEAM2:{}", URL_SAFE_NO_PAD.encode(packed));

        Ok(Self {
            listener,
            folder,
            invitation,
            token,
            tls: Arc::new(tls),
            expires_at: Instant::now() + INVITATION_LIFETIME,
        })
    }

    pub fn invitation(&self) -> &str {
        &self.invitation
    }

    pub fn receive(self, cancelled: &AtomicBool, progress: impl FnMut(u64)) -> Result<Receipt> {
        self.receive_with_start(cancelled, || {}, progress)
    }

    pub fn receive_with_start(
        self,
        cancelled: &AtomicBool,
        on_authenticated: impl FnOnce(),
        progress: impl FnMut(u64),
    ) -> Result<Receipt> {
        let mut stream = self.accept_authenticated(cancelled)?;
        let folder = self.folder;
        drop(self.listener);
        on_authenticated();
        let now = Instant::now();
        stream.sock.deadline = now + TRANSFER_TIMEOUT;
        stream.sock.idle_deadline = now + IDLE_TIMEOUT;
        stream.write_all(&ACCEPT)?;
        stream.flush()?;
        receive_file(&mut stream, &folder, progress)
    }

    fn accept_authenticated<'a>(&self, cancelled: &'a AtomicBool) -> Result<TlsStream<'a>> {
        for _ in 0..MAX_ATTEMPTS {
            let accepted = loop {
                check_deadline(cancelled, self.expires_at)?;
                match self.listener.accept() {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(POLL_INTERVAL)
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => break result,
                }
            };
            let (socket, peer) = match accepted {
                Ok(pair) => pair,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            if !matches!(peer, SocketAddr::V4(address) if address.ip().is_private()) {
                continue;
            }
            let now = Instant::now();
            socket.set_nonblocking(true)?;
            let socket = BoundedTcp {
                socket,
                cancelled,
                deadline: self.expires_at.min(now + AUTH_TIMEOUT),
                idle_deadline: now + IDLE_TIMEOUT,
            };
            let mut stream =
                StreamOwned::new(ServerConnection::new(Arc::clone(&self.tls))?, socket);
            if authenticate(&mut stream, &self.token).is_ok() {
                stream.sock.check()?;
                return Ok(stream);
            }
        }
        check_deadline(cancelled, self.expires_at)?;
        Err(invalid("invitation attempt limit reached").into())
    }
}

fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| io::Error::other("system randomness unavailable"))?;
    Ok(bytes)
}

fn destination(folder: &Path) -> io::Result<PathBuf> {
    let folder: PathBuf = folder.components().collect();
    if !matches!(folder.components().next_back(), Some(Component::Normal(_))) {
        return Err(invalid("a private receive directory is required"));
    }
    check_directory(&folder)?;
    let folder = fs::canonicalize(folder)?;
    check_directory(&folder)?;
    Ok(folder)
}

fn check_directory(folder: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(folder)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("receive directory must not be a link"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("receive directory must not be a reparse point"));
        }
    }
    Ok(())
}

type TlsStream<'a> = StreamOwned<ServerConnection, BoundedTcp<'a>>;

struct BoundedTcp<'a> {
    socket: TcpStream,
    cancelled: &'a AtomicBool,
    deadline: Instant,
    idle_deadline: Instant,
}

fn check_deadline(cancelled: &AtomicBool, deadline: Instant) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::other("receive cancelled"));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "receive timed out"));
    }
    Ok(())
}

impl BoundedTcp<'_> {
    fn check(&self) -> io::Result<()> {
        check_deadline(self.cancelled, self.deadline.min(self.idle_deadline))
    }

    fn poll_io(
        &mut self,
        mut op: impl FnMut(&mut TcpStream) -> io::Result<usize>,
    ) -> io::Result<usize> {
        loop {
            self.check()?;
            match op(&mut self.socket) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(POLL_INTERVAL)
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(count) => {
                    if count != 0 {
                        self.idle_deadline = Instant::now() + IDLE_TIMEOUT;
                    }
                    return Ok(count);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Read for BoundedTcp<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.poll_io(|socket| socket.read(buf))
    }
}

impl Write for BoundedTcp<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.poll_io(|socket| socket.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.check()
    }
}

fn authenticate(stream: &mut TlsStream<'_>, token: &[u8; 32]) -> io::Result<()> {
    let mut version = [0; VERSION.len()];
    stream.read_exact(&mut version)?;
    if version != VERSION {
        return Err(invalid("unsupported SEAM envelope"));
    }
    let presented = read_frame::<32>(stream, 0x01)?;
    if !bool::from(presented.ct_eq(token)) {
        return Err(invalid("invalid invitation token"));
    }
    Ok(())
}

fn frame_header(stream: &mut TlsStream<'_>) -> io::Result<(u8, u32)> {
    stream.sock.check()?;
    let mut header = [0; 5];
    stream.read_exact(&mut header)?;
    let [kind, a, b, c, d] = header;
    Ok((kind, u32::from_be_bytes([a, b, c, d])))
}

fn read_frame<const N: usize>(stream: &mut TlsStream<'_>, kind: u8) -> io::Result<[u8; N]> {
    if frame_header(stream)? != (kind, N as u32) {
        return Err(invalid("unexpected SEAM frame"));
    }
    let mut payload = [0; N];
    stream.read_exact(&mut payload)?;
    stream.sock.check()?;
    Ok(payload)
}

fn receive_file(
    stream: &mut TlsStream<'_>,
    folder: &Path,
    mut progress: impl FnMut(u64),
) -> Result<Receipt> {
    let [kind, size @ ..] = read_frame::<9>(stream, 0x03)?;
    if !matches!(kind, 0x01 | 0x02) {
        return Err(invalid("unsupported file kind").into());
    }
    let size = u64::from_be_bytes(size);
    let known = size != u64::MAX;
    let limit = if known { size } else { MAX_BYTES };
    check_length(limit)?;

    check_directory(folder)?;
    let mut staging = NamedTempFile::new_in(folder)?;
    let mut chunk = [0; MAX_CHUNK];
    let mut prefix = [0; 12];
    let mut bytes = 0_u64;
    let mut digest = Sha256::new();
    let end = loop {
        match frame_header(stream)? {
            (0x04, count) if count != 0 && count <= MAX_CHUNK as u32 => {
                let total = bytes + u64::from(count);
                if total > limit {
                    return Err(invalid("transfer exceeds allowed length").into());
                }
                let data = &mut chunk[..count as usize];
                stream.read_exact(data)?;
                stream.sock.check()?;
                staging.write_all(data)?;
                digest.update(&*data);
                if bytes < prefix.len() as u64 {
                    let offset = bytes as usize;
                    let copy = data.len().min(prefix.len() - offset);
                    prefix[offset..offset + copy].copy_from_slice(&data[..copy]);
                }
                bytes = total;
                progress(bytes);
            }
            (0x05, 40) => {
                let mut end = [0; 40];
                stream.read_exact(&mut end)?;
                if (known && size != bytes)
                    || end[..8] != bytes.to_be_bytes()
                    || end[8..] != digest.finalize()[..]
                {
                    return Err(invalid("transfer integrity check failed").into());
                }
                break end;
            }
            _ => return Err(invalid("unexpected transfer frame").into()),
        }
    };

    stream.sock.check()?;
    staging.as_file().sync_all()?;
    let prefix = &prefix[..bytes.min(prefix.len() as u64) as usize];
    let (label, extension) = if kind == 0x01 {
        ("image", image_extension(prefix))
    } else {
        ("document", "pdf")
    };
    let suffix = u128::from_be_bytes(random()?);
    let path = folder.join(format!("SEAM {label}-{suffix:032x}.{extension}"));
    check_directory(folder)?;
    stream.sock.check()?;
    staging
        .persist_noclobber(&path)
        .map_err(|error| error.error)?;

    let mut committed = [0; 45];
    committed[..5].copy_from_slice(&[0x06, 0, 0, 0, 40]);
    committed[5..].copy_from_slice(&end);
    let confirmed = stream
        .write_all(&committed)
        .and_then(|()| stream.flush())
        .is_ok();
    Ok(Receipt {
        path,
        bytes,
        confirmed,
    })
}

fn image_extension(prefix: &[u8]) -> &'static str {
    match prefix {
        [0x89, b'P', b'N', b'G', 13, 10, 26, 10, ..] => "png",
        [0xff, 0xd8, 0xff, ..] => "jpg",
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P'] => "webp",
        _ => "bin",
    }
}
