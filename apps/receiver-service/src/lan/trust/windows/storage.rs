use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::Ipv4Addr,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    ptr,
    sync::Arc,
};

use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::{
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    sign::CertifiedKey,
};
use tempfile::NamedTempFile;
use time::OffsetDateTime;

use super::{MAX_PEERS, Platform, StoredPeer};
use crate::{Result, invalid, lan::check_directory};

const ID_MAGIC: &[u8] = b"SEAM5 identity\x01";
const PEER_MAGIC: &[u8] = b"SEAM5 peers\x01";
const MAX_BLOB: usize = 16 * 1024;
const LOCK: &str = "trust.lock";
const IDENTITY: &str = "identity.dpapi";
const PEERS: &str = "peers.dpapi";
const RECORD_LEN: usize = 37;

pub(super) struct Store {
    directory: PathBuf,
    _lock: File,
}

impl Store {
    pub(super) fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        check_directory(directory)?;
        let directory = fs::canonicalize(directory)?;
        check_directory(&directory)?;
        let lock_path = directory.join(LOCK);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(0x0020_0000)
            .open(lock_path)?;
        file_attributes(&lock)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

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

    fn read(&self, file: &str) -> Result<Option<Sensitive>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(0x0020_0000)
            .open(self.directory.join(file))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        file_attributes(&file)?;
        let mut encrypted = Vec::new();
        file.take(MAX_BLOB as u64 + 1).read_to_end(&mut encrypted)?;
        Ok(Some(transform(&encrypted, false)?))
    }

    fn write(&self, file: &str, plaintext: &[u8]) -> Result<()> {
        check_directory(&self.directory)?;
        let encrypted = transform(plaintext, true)?;
        let mut staging = NamedTempFile::new_in(&self.directory)?;
        staging.write_all(&encrypted.0)?;
        staging.as_file().sync_all()?;
        let staging = staging.into_temp_path();
        let from = wide(&staging)?;
        let to = wide(&self.directory.join(file))?;
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
}

fn file_attributes(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & (0x40 | 0x400) != 0 {
        return Err(invalid("protected trust store is not an ordinary file"));
    }
    Ok(())
}

pub(super) fn has_space(folder: &Path) -> bool {
    const MIN_AVAILABLE: u64 = 512 * 1024 * 1024;
    if check_directory(folder).is_err() {
        return false;
    }
    let Ok(folder) = wide(folder) else {
        return false;
    };
    let mut available = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            folder.as_ptr(),
            &mut available,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    ok != 0 && available >= MIN_AVAILABLE
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(invalid("invalid trust store path"));
    }
    wide.push(0);
    Ok(wide)
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

#[repr(C)]
struct Blob {
    size: u32,
    data: *mut u8,
}

impl Drop for Blob {
    fn drop(&mut self) {
        if !self.data.is_null() {
            let bytes = unsafe { std::slice::from_raw_parts_mut(self.data, self.size as usize) };
            wipe(bytes);
            unsafe { LocalFree(self.data.cast()) };
        }
    }
}

fn transform(data: &[u8], protect: bool) -> io::Result<Sensitive> {
    if !(1..=MAX_BLOB).contains(&data.len()) {
        return Err(invalid("invalid protected blob length"));
    }
    let input = std::mem::ManuallyDrop::new(Blob {
        size: data.len() as u32,
        data: data.as_ptr().cast_mut(),
    });
    let mut output = Blob {
        size: 0,
        data: ptr::null_mut(),
    };
    let result = unsafe {
        if protect {
            CryptProtectData(
                &*input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &*input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                1,
                &mut output,
            )
        }
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    if output.data.is_null() || !(1..=MAX_BLOB).contains(&(output.size as usize)) {
        return Err(invalid("invalid protected blob length"));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.data, output.size as usize) };
    Ok(Sensitive(bytes.to_vec()))
}

#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        data: *const Blob,
        description: *const u16,
        entropy: *const Blob,
        reserved: *const std::ffi::c_void,
        prompt: *const std::ffi::c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        data: *const Blob,
        description: *mut *mut u16,
        entropy: *const Blob,
        reserved: *const std::ffi::c_void,
        prompt: *const std::ffi::c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(handle: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn MoveFileExW(existing: *const u16, destination: *const u16, flags: u32) -> i32;
    fn GetDiskFreeSpaceExW(
        folder: *const u16,
        available: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> i32;
}
