use std::env;
use std::ffi::CStr;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::Cancel;

pub(super) const RECEIVE_FLAG: &CStr = c"trusted-receive-opt-in.v1.flag";

#[derive(Clone)]
pub(super) struct Foreground(Cancel);

impl Foreground {
    pub(super) fn is_current(&self) -> bool {
        !self.0.load(Ordering::Acquire)
    }
}

pub(super) struct ForegroundWatch {
    pub(super) key: Foreground,
}

impl ForegroundWatch {
    pub(super) fn capture(active: bool) -> Option<Self> {
        active.then(|| Self {
            key: Foreground(Cancel::default()),
        })
    }

    pub(super) fn invalidate(&self) {
        self.key.0.store(true, Ordering::Release);
    }
}

impl Drop for ForegroundWatch {
    fn drop(&mut self) {
        self.invalidate();
    }
}

pub(super) fn open_trust_directory() -> io::Result<PathBuf> {
    fn absolute_env(key: &str) -> Option<PathBuf> {
        env::var_os(key)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }
    #[cfg(target_os = "macos")]
    let base = absolute_env("HOME").map(|home| home.join("Library/Application Support"));
    #[cfg(not(target_os = "macos"))]
    let base = absolute_env("XDG_DATA_HOME")
        .or_else(|| absolute_env("HOME").map(|home| home.join(".local/share")));
    let base = base.ok_or_else(|| io::Error::other("no absolute trust directory base"))?;
    fs::create_dir_all(&base)?;
    let base = fs::canonicalize(base)?;
    let metadata = fs::metadata(&base)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe trust directory base",
        ));
    }
    #[cfg(target_os = "macos")]
    let directory = base.join("Sliploom Trust v1");
    #[cfg(not(target_os = "macos"))]
    let directory = base.join("sliploom-trust-v1");
    match DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    private_directory(&directory)?;
    Ok(directory)
}

fn private_directory(path: &Path) -> io::Result<File> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = directory.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe trust directory",
        ));
    }
    Ok(directory)
}

fn flag_directory(path: &Path) -> io::Result<File> {
    private_directory(
        path.parent()
            .ok_or_else(|| io::Error::other("missing trust directory"))?,
    )
}

fn flag_open(directory: &File, flags: libc::c_int) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            RECEIVE_FLAG.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600 as libc::c_int,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn flag_remove(directory: &File) -> io::Result<()> {
    if unsafe { libc::unlinkat(directory.as_raw_fd(), RECEIVE_FLAG.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn create_receive_flag(path: &Path) -> io::Result<()> {
    let directory = flag_directory(path)?;
    let file = flag_open(&directory, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
    if let Err(error) = file.set_permissions(Permissions::from_mode(0o600)) {
        let _ = flag_remove(&directory);
        return Err(error);
    }
    Ok(())
}

pub(super) fn receive_opt_in(path: &Path, clear: bool) -> io::Result<bool> {
    let directory = flag_directory(path)?;
    let file = match flag_open(&directory, libc::O_RDONLY) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid trusted receive flag",
        ));
    }
    if clear {
        flag_remove(&directory)?;
    }
    Ok(true)
}

pub(super) fn open_destination(selected: Option<PathBuf>, trust_dir: &Path) -> io::Result<PathBuf> {
    let path = selected.ok_or_else(|| io::Error::other("no selected save folder"))?;
    let canonical = fs::canonicalize(crate::settings::validate(path)?)?;
    let trust = private_directory(trust_dir)?.metadata()?;
    if canonical.ancestors().any(|path| {
        fs::metadata(path)
            .is_ok_and(|entry| entry.dev() == trust.dev() && entry.ino() == trust.ino())
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "reserved trust directory",
        ));
    }
    Ok(canonical)
}
