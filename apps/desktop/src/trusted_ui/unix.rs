use std::env;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::Cancel;

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

pub(super) fn open_destination(selected: Option<PathBuf>) -> io::Result<PathBuf> {
    let path = selected.ok_or_else(|| io::Error::other("no selected save folder"))?;
    fs::canonicalize(crate::settings::validate(path)?)
}
