use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rustix::{
    fs::{self, AtFlags, FlockOperation, Mode, OFlags},
    io::Errno,
    process::{geteuid, getuid},
};

use super::{LOCK, MAX_BLOB, Sensitive};
use crate::{Result, invalid, lan};

pub(super) const IDENTITY: &str = "identity.private";
pub(super) const PEERS: &str = "peers.private";
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::NOFOLLOW
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);
const DIR_MODE: Mode = Mode::from_bits_truncate(0o700);
const FILE_MODE: Mode = Mode::from_bits_truncate(0o600);

pub(in crate::lan::trust) struct Store {
    directory: File,
    _lock: File,
}

impl Store {
    pub(in crate::lan::trust) fn open(path: &Path) -> Result<Self> {
        let directory = private_directory(path, None)?;
        let lock = File::from(fs::openat(
            &directory,
            LOCK,
            FILE_FLAGS | OFlags::RDWR | OFlags::CREATE,
            FILE_MODE,
        )?);
        check_file(&lock)?;
        fs::flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub(super) fn read(&self, name: &str) -> Result<Option<Sensitive>> {
        let Some(file) = self.open_record(name)? else {
            return Ok(None);
        };
        let mut bytes = Sensitive(Vec::with_capacity(MAX_BLOB + 1));
        file.take(MAX_BLOB as u64 + 1).read_to_end(&mut bytes.0)?;
        if !(1..=MAX_BLOB).contains(&bytes.0.len()) {
            return Err(invalid("invalid private trust record length").into());
        }
        Ok(Some(bytes))
    }

    pub(super) fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        if !(1..=MAX_BLOB).contains(&bytes.len()) {
            return Err(invalid("invalid private trust record length").into());
        }
        self.open_record(name)?;
        let suffix = URL_SAFE_NO_PAD.encode(lan::random::<16>()?);
        let staging = format!(".trust-{suffix}");
        let mut file = File::from(fs::openat(
            &self.directory,
            &staging,
            FILE_FLAGS | OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
            FILE_MODE,
        )?);
        let result = (|| -> Result<()> {
            check_file(&file)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::renameat(&self.directory, &staging, &self.directory, name)?;
            self.directory.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::unlinkat(&self.directory, &staging, AtFlags::empty());
        }
        result
    }

    pub(in crate::lan::trust) fn receive_destination(&self, folder: &Path) -> Option<PathBuf> {
        const MIN_AVAILABLE: u64 = 512 * 1024 * 1024;
        let trust = self.directory.metadata().ok()?;
        let directory = private_directory(folder, Some((trust.dev(), trust.ino()))).ok()?;
        let space = fs::fstatvfs(&directory).ok()?;
        (space.f_bavail.saturating_mul(space.f_frsize) >= MIN_AVAILABLE)
            .then(|| folder.components().collect())
    }

    fn open_record(&self, name: &str) -> io::Result<Option<File>> {
        let file = match fs::openat(
            &self.directory,
            name,
            FILE_FLAGS | OFlags::RDONLY,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        check_file(&file)?;
        Ok(Some(file))
    }
}

fn private_directory(path: &Path, excluded: Option<(u64, u64)>) -> io::Result<File> {
    let uid = geteuid().as_raw();
    let mut components = path.components().peekable();
    if getuid().as_raw() != uid
        || components.next() != Some(Component::RootDir)
        || components.peek().is_none()
    {
        return Err(invalid("an absolute private directory is required"));
    }
    let mut current = File::from(fs::open("/", DIR_FLAGS, Mode::empty())?);
    check_directory(&current, uid, false, excluded)?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(invalid("invalid private directory path"));
        };
        let child = match fs::openat(&current, name, DIR_FLAGS, Mode::empty()) {
            Err(Errno::NOENT) if excluded.is_none() => {
                match fs::mkdirat(&current, name, DIR_MODE) {
                    Ok(()) | Err(Errno::EXIST) => (),
                    Err(error) => return Err(error.into()),
                }
                fs::openat(&current, name, DIR_FLAGS, Mode::empty())?
            }
            result => result?,
        };
        current = File::from(child);
        check_directory(&current, uid, components.peek().is_none(), excluded)?;
    }
    Ok(current)
}

fn check_directory(
    file: &File,
    uid: u32,
    private: bool,
    excluded: Option<(u64, u64)>,
) -> io::Result<()> {
    let metadata = file.metadata()?;
    if excluded == Some((metadata.dev(), metadata.ino())) {
        return Err(invalid("trust directory cannot be used for receiving"));
    }
    let mode = metadata.mode() & 0o7777;
    let owner = metadata.uid();
    if !metadata.is_dir()
        || if private {
            owner != uid || mode != 0o700
        } else {
            (owner != 0 && owner != uid) || mode & 0o022 != 0
        }
    {
        return Err(invalid(
            "insecure private directory ownership or permissions",
        ));
    }
    check_acl(file)
}

fn check_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(invalid("insecure trust record ownership or permissions"));
    }
    check_acl(file)
}

#[cfg(target_os = "linux")]
fn check_acl(_: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_acl(file: &File) -> io::Result<()> {
    use std::{ffi::c_void, os::fd::AsRawFd, ptr};

    const ACL_TYPE_EXTENDED: u32 = 0x100;
    const ACL_FIRST_ENTRY: i32 = 0;
    const ACL_NEXT_ENTRY: i32 = -1;
    const ACL_EXTENDED_DENY: u32 = 2;

    unsafe extern "C" {
        fn acl_get_fd_np(fd: i32, acl_type: u32) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: i32, entry: *mut *mut c_void) -> i32;
        fn acl_get_tag_type(entry: *mut c_void, tag_type: *mut u32) -> i32;
        fn acl_free(acl: *mut c_void) -> i32;
    }

    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    let result = (|| {
        let mut cursor = ACL_FIRST_ENTRY;
        loop {
            let mut entry = ptr::null_mut();
            if unsafe { acl_get_entry(acl, cursor, &mut entry) } != 0 {
                let error = io::Error::last_os_error();
                return if error.kind() == io::ErrorKind::InvalidInput {
                    Ok(())
                } else {
                    Err(error)
                };
            }
            let mut tag = 0;
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if tag != ACL_EXTENDED_DENY {
                return Err(invalid("private path has an extended ACL granting access"));
            }
            cursor = ACL_NEXT_ENTRY;
        }
    })();
    unsafe { acl_free(acl) };
    result
}
