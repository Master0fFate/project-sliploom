use std::{
    io::{self, Read},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rustls::{crypto::CryptoProvider, sign::CertifiedKey};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{PairPrompt, Peer};
use crate::{
    Result, invalid,
    lan::{self, Cancellation, CheckedStream, Receipt, ReceiveSession, sender::SendInvite},
};
use storage::Store;

#[path = "storage.rs"]
mod storage;
#[path = "transport.rs"]
mod transport;

const PORT: u16 = 48162;
const PREFIX: &str = "SEAM5:";
const MAGIC: &[u8; 5] = b"SEAM\x05";
const PAIR: u8 = 0x01;
const PING: u8 = 0x02;
const OPEN: u8 = 0x03;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const MAX_PEERS: usize = 16;
const PAIR_TIMEOUT: Duration = Duration::from_secs(60);
const INVITE_LEN: usize = 100;

#[derive(Clone)]
pub struct Trust(Arc<Shared>);

struct Shared {
    store: Store,
    key: Arc<CertifiedKey>,
    id: [u8; 32],
    provider: Arc<CryptoProvider>,
    state: Mutex<State>,
    publish_gate: Mutex<()>,
}

struct State {
    listener: Option<Ipv4Addr>,
    peers: Vec<StoredPeer>,
    pairing: Option<PairSlot>,
    grant: Option<Grant>,
}

#[derive(Clone)]
struct StoredPeer {
    id: [u8; 32],
    address: Ipv4Addr,
    platform: Platform,
    cancelled: Arc<AtomicBool>,
}

impl StoredPeer {
    fn new(id: [u8; 32], address: Ipv4Addr, platform: Platform) -> Self {
        Self {
            id,
            address,
            platform,
            cancelled: Arc::default(),
        }
    }

    fn public(&self) -> Peer {
        Peer {
            id: self.id,
            address: self.address,
            label: self.platform.label().into(),
        }
    }
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum Platform {
    Windows = 1,
    Android = 2,
    MacOs = 3,
    Linux = 4,
}

impl Platform {
    fn local() -> Self {
        #[cfg(windows)]
        return Self::Windows;
        #[cfg(target_os = "macos")]
        return Self::MacOs;
        #[cfg(target_os = "linux")]
        return Self::Linux;
    }

    fn parse(byte: u8) -> io::Result<Self> {
        match byte {
            1 => Ok(Self::Windows),
            2 => Ok(Self::Android),
            3 => Ok(Self::MacOs),
            4 => Ok(Self::Linux),
            _ => Err(invalid("unsupported peer platform")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Android => "Android",
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
        }
    }
}

struct Armed {
    expires: Instant,
    nonce: [u8; 32],
}

enum PairSlot {
    Armed(Armed),
    Active(Arc<AtomicBool>),
}

struct Grant {
    id: [u8; 32],
    cancelled: Arc<AtomicBool>,
}

impl Trust {
    pub fn open(state_dir: &Path) -> Result<Self> {
        let store = Store::open(state_dir)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = store.identity(&provider)?;
        let id = transport::hash(&key.cert[0]);
        let peers = store.peers(&id)?;
        if peers.iter().any(|peer| peer.id == id) {
            return Err(invalid("invalid protected peer identity").into());
        }
        Ok(Self(Arc::new(Shared {
            store,
            key,
            id,
            provider,
            state: Mutex::new(State {
                listener: None,
                peers,
                pairing: None,
                grant: None,
            }),
            publish_gate: Mutex::new(()),
        })))
    }

    #[cfg(unix)]
    pub fn receive_opt_in(&self) -> io::Result<bool> {
        let _state = self.state();
        self.0.store.receive_opt_in()
    }

    #[cfg(unix)]
    pub fn set_receive_opt_in(&self, allowed: bool) -> io::Result<()> {
        let _state = self.state();
        self.0.store.set_receive_opt_in(allowed)
    }

    #[cfg(unix)]
    pub fn receive_destination(&self, folder: &Path) -> Option<PathBuf> {
        self.0.store.receive_destination(folder)
    }

    pub fn arm_pair(&self) -> Result<String> {
        let mut state = self.state();
        let address = state
            .listener
            .ok_or_else(|| invalid("trusted Wi-Fi is not listening"))?;
        if state.peers.len() >= MAX_PEERS || matches!(state.pairing, Some(PairSlot::Active(_))) {
            return Err(invalid("pairing unavailable").into());
        }
        let nonce = lan::random()?;
        let mut packed = [0; 70];
        packed[..4].copy_from_slice(&address.octets());
        packed[4..6].copy_from_slice(&PORT.to_be_bytes());
        packed[6..38].copy_from_slice(&self.0.id);
        packed[38..].copy_from_slice(&nonce);
        state.pairing = Some(PairSlot::Armed(Armed {
            expires: Instant::now() + lan::INVITATION_LIFETIME,
            nonce,
        }));
        Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(packed)))
    }

    pub fn cancel_pair(&self) {
        self.state().cancel_pair();
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.state().peers.iter().map(StoredPeer::public).collect()
    }

    pub fn forget(&self, id: [u8; 32]) -> Result<()> {
        let _publish_gate = self.0.publish_gate.lock().expect("publish lock poisoned");
        let mut state = self.state();
        let index = state
            .peers
            .iter()
            .position(|peer| peer.id == id)
            .ok_or_else(|| invalid("paired device not found"))?;
        let mut peers = state.peers.clone();
        let removed = peers.remove(index);
        self.0.store.save_peers(&self.0.id, &peers)?;
        state.peers = peers;
        removed.cancelled.store(true, Ordering::Release);
        if let Some(grant) = state.grant.as_ref().filter(|grant| grant.id == id) {
            grant.cancelled.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub fn serve<D: AsRef<Path> + Send>(
        &self,
        address: Ipv4Addr,
        destination: impl Fn() -> io::Result<D>,
        cancel: &AtomicBool,
        on_pair: impl Fn(PairPrompt) -> bool,
        on_saved: impl Fn(Receipt),
    ) -> Result<()> {
        if !address.is_private() {
            return Err(invalid("an RFC1918 IPv4 address is required").into());
        }
        let tls = transport::server_config(self)?;
        let listener = {
            let mut state = self.state();
            if state.listener.is_some() || cancel.load(Ordering::Acquire) {
                return Err(invalid("trusted Wi-Fi is already listening or was stopped").into());
            }
            let listener = TcpListener::bind((address, PORT))?;
            listener.set_nonblocking(true)?;
            state.listener = Some(address);
            listener
        };
        let (send, saved) = mpsc::channel();
        let result = thread::scope(|scope| {
            let _listener = ListenerLease(self);
            while !cancel.load(Ordering::Acquire) {
                self.state().expire_pairing();
                for receipt in saved.try_iter() {
                    on_saved(receipt);
                }
                let (socket, remote) = match listener.accept() {
                    Ok(accepted) => accepted,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(lan::POLL_INTERVAL);
                        continue;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted
                                | io::ErrorKind::ConnectionAborted
                                | io::ErrorKind::ConnectionReset
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                let SocketAddr::V4(remote) = remote else {
                    continue;
                };
                if !remote.ip().is_private() {
                    continue;
                }
                let Ok(mut stream) = transport::accept(socket, cancel, &tls) else {
                    continue;
                };
                let Some(id) = stream
                    .conn
                    .peer_certificates()
                    .and_then(|chain| chain.first())
                    .map(transport::hash)
                else {
                    continue;
                };
                let mut request = [0; 6];
                if stream.read_exact(&mut request).is_err() || &request[..5] != MAGIC {
                    continue;
                }
                match request[5] {
                    PAIR => {
                        let _ = self.accept_pair(stream, id, *remote.ip(), &on_pair);
                    }
                    PING => {
                        let ready = self
                            .available_destination(&destination, id, *remote.ip())
                            .is_some();
                        let _ = transport::write(&mut stream, &[if ready { ACK } else { NAK }]);
                    }
                    OPEN => {
                        let Some((session, grant, folder)) =
                            self.prepare_receive(address, &destination, id, *remote.ip(), cancel)
                        else {
                            let _ = transport::write(&mut stream, &[NAK]);
                            continue;
                        };
                        let mut reply = [ACK; 1 + INVITE_LEN];
                        reply[1..].copy_from_slice(session.invitation().as_bytes());
                        if grant.cancelled.load(Ordering::Acquire)
                            || transport::write(&mut stream, &reply).is_err()
                        {
                            continue;
                        }
                        let send = send.clone();
                        let _ = thread::Builder::new().spawn_scoped(scope, move || {
                            let _ = session.receive_batch_trusted(
                                Cancellation {
                                    worker: cancel,
                                    connection: Some(&grant.cancelled),
                                },
                                &self.0.publish_gate,
                                |_| self.state().peers.iter().any(|peer| peer.id == id),
                                |receipt| {
                                    let _ = send.send(receipt);
                                },
                            );
                            drop(folder);
                        });
                    }
                    _ => {
                        let _ = transport::write(&mut stream, &[NAK]);
                    }
                }
            }
            Ok(())
        });
        for receipt in saved.try_iter() {
            on_saved(receipt);
        }
        result
    }

    pub fn pair(
        &self,
        code: &str,
        cancel: &AtomicBool,
        on_pair: impl FnOnce(PairPrompt) -> bool,
    ) -> Result<Peer> {
        let (endpoint, server, nonce) = decode_invite(code)?;
        let pending = {
            let mut state = self.state();
            state.check_new_peer(self.0.id, server)?;
            if state.armed() || matches!(state.pairing, Some(PairSlot::Active(_))) {
                return Err(invalid("pairing already armed or in progress").into());
            }
            self.reserve_pair(&mut state)
        };
        let cancellation = Cancellation {
            worker: cancel,
            connection: Some(&pending.cancelled),
        };
        let mut stream = transport::connect(self, endpoint, server, cancellation)?;
        let remote = transport::peer_address(&stream.sock)?;
        transport::write_prelude(&mut stream, PAIR)?;
        let mut request = [Platform::local() as u8; 33];
        request[..32].copy_from_slice(&nonce);
        transport::write(&mut stream, &request)?;
        transport::read_ack(&mut stream)?;
        let platform = Platform::parse(transport::read_byte(&mut stream)?)?;
        set_pair_deadline(&mut stream.sock);
        let agreed = on_pair(pair_prompt(nonce, server, self.0.id, platform));
        transport::write(&mut stream, &[if agreed { ACK } else { NAK }])?;
        if !agreed {
            return Err(invalid("pairing declined").into());
        }
        transport::read_ack(&mut stream)?;
        let peer = StoredPeer::new(server, remote, platform);
        self.enroll(&pending, stream.sock.cancelled, stream.sock.deadline, peer)
    }

    pub fn ping(&self, peer: &Peer, cancel: &AtomicBool) -> bool {
        self.control(peer, cancel, PING).is_ok()
    }

    pub fn open_receive(&self, peer: &Peer, cancel: &AtomicBool) -> Result<SendInvite> {
        let (address, invite) = self.control(peer, cancel, OPEN)?;
        let invite = SendInvite::parse(str::from_utf8(&invite)?)
            .filter(|invite| invite.is_batch() && invite.address() == address)
            .ok_or_else(|| invalid("invalid trusted receive invitation"))?;
        Ok(invite)
    }

    fn control(
        &self,
        peer: &Peer,
        cancel: &AtomicBool,
        operation: u8,
    ) -> Result<(Ipv4Addr, [u8; INVITE_LEN])> {
        let enrolled = self
            .state()
            .find(peer.id)
            .cloned()
            .ok_or_else(|| invalid("paired device not found"))?;
        let cancellation = Cancellation {
            worker: cancel,
            connection: Some(&enrolled.cancelled),
        };
        let mut stream = transport::connect(
            self,
            SocketAddrV4::new(enrolled.address, PORT),
            enrolled.id,
            cancellation,
        )?;
        let remote = transport::peer_address(&stream.sock)?;
        transport::write_prelude(&mut stream, operation)?;
        transport::read_ack(&mut stream)?;
        let mut invite = [0; INVITE_LEN];
        if operation == OPEN {
            stream.read_exact(&mut invite)?;
        }
        stream.check()?;
        Ok((remote, invite))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().expect("trust state lock poisoned")
    }

    fn accept_pair(
        &self,
        mut stream: transport::ServerStream<'_>,
        id: [u8; 32],
        address: Ipv4Addr,
        on_pair: impl FnOnce(PairPrompt) -> bool,
    ) -> Result<()> {
        let mut body = [0; 33];
        stream.read_exact(&mut body)?;
        stream.check()?;
        let [nonce @ .., platform] = body;
        let pending = {
            let mut state = self.state();
            if let Some(PairSlot::Armed(armed)) = &state.pairing
                && Instant::now() < armed.expires
                && bool::from(nonce.ct_eq(&armed.nonce))
            {
                let pending = self.reserve_pair(&mut state);
                drop(state);
                pending
            } else {
                drop(state);
                return transport::write(&mut stream, &[NAK]);
            }
        };
        let platform = match Platform::parse(platform) {
            Ok(platform) if self.state().check_new_peer(self.0.id, id).is_ok() => platform,
            _ => return transport::write(&mut stream, &[NAK]),
        };
        let mut stream = rustls::StreamOwned::new(
            stream.conn,
            lan::BoundedTcp {
                cancelled: Cancellation {
                    worker: stream.sock.cancelled.worker,
                    connection: Some(&pending.cancelled),
                },
                ..stream.sock
            },
        );
        set_pair_deadline(&mut stream.sock);
        transport::write(&mut stream, &[ACK, Platform::local() as u8])?;
        let result = (|| -> Result<Peer> {
            if !on_pair(pair_prompt(nonce, self.0.id, id, platform)) {
                return Err(invalid("pairing declined").into());
            }
            transport::read_ack(&mut stream)?;
            self.enroll(
                &pending,
                stream.sock.cancelled,
                stream.sock.deadline,
                StoredPeer::new(id, address, platform),
            )
        })();
        transport::write(&mut stream, &[if result.is_ok() { ACK } else { NAK }])
    }

    fn reserve_pair<'a>(&'a self, state: &mut State) -> PairLease<'a> {
        let cancelled = Arc::default();
        state.pairing = Some(PairSlot::Active(Arc::clone(&cancelled)));
        PairLease {
            trust: self,
            cancelled,
        }
    }

    fn enroll(
        &self,
        lease: &PairLease<'_>,
        cancel: Cancellation<'_>,
        deadline: Instant,
        peer: StoredPeer,
    ) -> Result<Peer> {
        let mut state = self.state();
        if !matches!(&state.pairing, Some(PairSlot::Active(current)) if Arc::ptr_eq(current, &lease.cancelled))
        {
            return Err(invalid("pairing cancelled").into());
        }
        state.check_new_peer(self.0.id, peer.id)?;
        lan::check_deadline(cancel, deadline)?;
        let result = peer.public();
        let mut peers = state.peers.clone();
        peers.push(peer);
        self.0.store.save_peers(&self.0.id, &peers)?;
        state.peers = peers;
        Ok(result)
    }

    fn observe_peer(&self, id: [u8; 32], address: Ipv4Addr) -> bool {
        let mut state = self.state();
        let Some(index) = state.peers.iter().position(|peer| peer.id == id) else {
            return false;
        };
        if state.peers[index].address != address {
            let mut peers = state.peers.clone();
            peers[index].address = address;
            if self.0.store.save_peers(&self.0.id, &peers).is_ok() {
                state.peers = peers;
            }
        }
        true
    }

    fn available_destination<D: AsRef<Path>>(
        &self,
        destination: &impl Fn() -> io::Result<D>,
        id: [u8; 32],
        remote: Ipv4Addr,
    ) -> Option<(D, PathBuf)> {
        if !self.observe_peer(id, remote) || self.state().grant.is_some() {
            return None;
        }
        let folder = destination().ok()?;
        let path = self.0.store.receive_destination(folder.as_ref())?;
        Some((folder, path))
    }

    fn prepare_receive<D: AsRef<Path>>(
        &self,
        address: Ipv4Addr,
        destination: &impl Fn() -> io::Result<D>,
        id: [u8; 32],
        remote: Ipv4Addr,
        cancel: &AtomicBool,
    ) -> Option<(ReceiveSession, GrantLease<'_>, D)> {
        let (folder, path) = self.available_destination(destination, id, remote)?;
        let session = ReceiveSession::bind_batch(address, &path).ok()?;
        let mut state = self.state();
        if state.grant.is_some() || state.find(id).is_none() || cancel.load(Ordering::Acquire) {
            return None;
        }
        let cancelled = Arc::default();
        state.grant = Some(Grant {
            id,
            cancelled: Arc::clone(&cancelled),
        });
        Some((
            session,
            GrantLease {
                trust: self,
                cancelled,
            },
            folder,
        ))
    }
}

impl State {
    fn armed(&self) -> bool {
        matches!(&self.pairing, Some(PairSlot::Armed(armed)) if Instant::now() < armed.expires)
    }

    fn expire_pairing(&mut self) {
        if matches!(&self.pairing, Some(PairSlot::Armed(armed)) if Instant::now() >= armed.expires)
        {
            self.pairing = None;
        }
    }

    fn cancel_pair(&mut self) {
        if let Some(PairSlot::Active(flag)) = self.pairing.take() {
            flag.store(true, Ordering::Release);
        }
    }

    fn find(&self, id: [u8; 32]) -> Option<&StoredPeer> {
        self.peers.iter().find(|peer| peer.id == id)
    }

    fn check_new_peer(&self, own_id: [u8; 32], id: [u8; 32]) -> io::Result<()> {
        if id == own_id || self.find(id).is_some() || self.peers.len() >= MAX_PEERS {
            Err(invalid("device cannot be paired"))
        } else {
            Ok(())
        }
    }
}

struct ListenerLease<'a>(&'a Trust);

impl Drop for ListenerLease<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.listener = None;
        state.cancel_pair();
        if let Some(grant) = &state.grant {
            grant.cancelled.store(true, Ordering::Release);
        }
    }
}

struct PairLease<'a> {
    trust: &'a Trust,
    cancelled: Arc<AtomicBool>,
}

impl Drop for PairLease<'_> {
    fn drop(&mut self) {
        let mut state = self.trust.state();
        if matches!(&state.pairing, Some(PairSlot::Active(current)) if Arc::ptr_eq(current, &self.cancelled))
        {
            state.pairing = None;
        }
    }
}

struct GrantLease<'a> {
    trust: &'a Trust,
    cancelled: Arc<AtomicBool>,
}

impl Drop for GrantLease<'_> {
    fn drop(&mut self) {
        let mut state = self.trust.state();
        if state
            .grant
            .as_ref()
            .is_some_and(|grant| Arc::ptr_eq(&grant.cancelled, &self.cancelled))
        {
            state.grant = None;
        }
    }
}

fn decode_invite(code: &str) -> io::Result<(SocketAddrV4, [u8; 32], [u8; 32])> {
    let encoded = code
        .strip_prefix(PREFIX)
        .ok_or_else(|| invalid("invalid Sliploom pairing code"))?;
    let mut packed = [0; 70];
    if code.len() != INVITE_LEN
        || URL_SAFE_NO_PAD.decode_slice(encoded, &mut packed).ok() != Some(70)
        || URL_SAFE_NO_PAD.encode(packed) != encoded
    {
        return Err(invalid("invalid Sliploom pairing code"));
    }
    let [a, b, c, d, p, q, rest @ ..] = packed;
    let address = Ipv4Addr::new(a, b, c, d);
    if !address.is_private() || u16::from_be_bytes([p, q]) != PORT {
        return Err(invalid("invalid Sliploom pairing endpoint"));
    }
    let [pin, nonce] = rest.as_chunks::<32>().0 else {
        unreachable!()
    };
    Ok((SocketAddrV4::new(address, PORT), *pin, *nonce))
}

fn pair_prompt(
    nonce: [u8; 32],
    server: [u8; 32],
    client: [u8; 32],
    platform: Platform,
) -> PairPrompt {
    let mut hash = Sha256::new();
    hash.update(b"SEAM5 pair\x00");
    hash.update(nonce);
    hash.update(server);
    hash.update(client);
    let digest: [u8; 32] = hash.finalize().into();
    let [a, b, c, d, ..] = digest;
    let code = u32::from_be_bytes([a, b, c, d]) % 1_000_000;
    PairPrompt {
        code: format!("{code:06}"),
        label: platform.label().into(),
    }
}

fn set_pair_deadline(socket: &mut lan::BoundedTcp<'_>) {
    socket.deadline = Instant::now() + PAIR_TIMEOUT;
    socket.idle_deadline = socket.deadline;
}
