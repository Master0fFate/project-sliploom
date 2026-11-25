use std::{net::Ipv4Addr, ptr, sync::Arc};

use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::{
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    sign::CertifiedKey,
};
use time::OffsetDateTime;

use super::{MAX_PEERS, Platform, StoredPeer};
use crate::{Result, invalid};

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;

pub(super) use platform::Store;
use platform::{IDENTITY, PEERS};

const ID_MAGIC: &[u8] = b"SEAM5 identity";
const PEER_MAGIC: &[u8] = b"SEAM5 peers";
const LOCK: &str = "trust.lock";
const MAX_BLOB: usize = 16 * 1024;
const RECORD_LEN: usize = 37;

impl Store {
    pub(super) fn identity(&self, provider: &CryptoProvider) -> Result<Arc<CertifiedKey>> {
        let mut existing = true;
        let plaintext = if let Some(plaintext) = self.read(IDENTITY)? {
            plaintext
        } else {
            if self.read(PEERS)?.is_some() {
                return Err(invalid("protected peer store has no local identity").into());
            }
            existing = false;
            let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
            let now = OffsetDateTime::now_utc();
            let mut params = CertificateParams::default();
            params.not_before = now - time::Duration::minutes(5);
            params.not_after = now + time::Duration::days(3650);
            params.distinguished_name = rcgen::DistinguishedName::new();
            params
                .distinguished_name
                .push(rcgen::DnType::CommonName, "SEAM");
            let cert = params.self_signed(&key)?;
            let mut bytes = ID_MAGIC.to_vec();
            bytes.extend_from_slice(&u16::try_from(cert.der().len())?.to_be_bytes());
            bytes.extend_from_slice(cert.der().as_ref());
            bytes.extend_from_slice(key.serialized_der());
            Sensitive(bytes)
        };
        let (size, bytes) = plaintext
            .0
            .strip_prefix(ID_MAGIC)
            .and_then(|bytes| bytes.split_first_chunk::<2>())
            .ok_or_else(|| invalid("invalid protected identity"))?;
        let (cert, key) = bytes
            .split_at_checked(usize::from(u16::from_be_bytes(*size)))
            .ok_or_else(|| invalid("invalid protected identity"))?;
        let cert = CertificateDer::from(cert.to_vec());
        let key = PrivatePkcs8KeyDer::from(key.to_vec());
        let certified = CertifiedKey::from_der(vec![cert], key.into(), provider)?;
        certified.keys_match()?;
        if !existing {
            self.write(IDENTITY, &plaintext.0)?;
        }
        Ok(Arc::new(certified))
    }

    pub(super) fn peers(&self, identity: &[u8; 32]) -> Result<Vec<StoredPeer>> {
        let Some(plaintext) = self.read(PEERS)? else {
            return Ok(Vec::new());
        };
        let (count, bytes) = plaintext
            .0
            .strip_prefix(PEER_MAGIC)
            .and_then(|bytes| bytes.strip_prefix(identity))
            .and_then(|bytes| bytes.split_first())
            .ok_or_else(|| invalid("invalid protected peers"))?;
        let count = usize::from(*count);
        if count > MAX_PEERS || bytes.len() != count * RECORD_LEN {
            return Err(invalid("invalid protected peers").into());
        }
        let mut peers: Vec<StoredPeer> = Vec::with_capacity(count);
        for record in bytes.as_chunks::<RECORD_LEN>().0 {
            let [id @ .., a, b, c, d, platform] = *record;
            let address = Ipv4Addr::new(a, b, c, d);
            if !address.is_private() || peers.iter().any(|peer| peer.id == id) {
                return Err(invalid("invalid protected peer identity or address").into());
            }
            peers.push(StoredPeer::new(id, address, Platform::parse(platform)?));
        }
        Ok(peers)
    }

    pub(super) fn save_peers(&self, identity: &[u8; 32], peers: &[StoredPeer]) -> Result<()> {
        if peers.len() > MAX_PEERS {
            return Err(invalid("paired device limit reached").into());
        }
        let mut bytes = Vec::with_capacity(PEER_MAGIC.len() + 33 + peers.len() * RECORD_LEN);
        bytes.extend_from_slice(PEER_MAGIC);
        bytes.extend_from_slice(identity);
        bytes.push(peers.len() as u8);
        for peer in peers {
            bytes.extend_from_slice(&peer.id);
            bytes.extend_from_slice(&peer.address.octets());
            bytes.push(peer.platform as u8);
        }
        self.write(PEERS, &bytes)
    }
}

struct Sensitive(Vec<u8>);

impl Drop for Sensitive {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { ptr::write_volatile(byte, 0) };
    }
}
