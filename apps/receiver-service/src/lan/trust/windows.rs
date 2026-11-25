use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    ptr,
};

use tempfile::NamedTempFile;

use super::{LOCK, MAX_BLOB, Sensitive, wipe};
use crate::{Result, invalid, lan::check_directory};

pub(super) const IDENTITY: &str = "identity.dpapi";
pub(super) const PEERS: &str = "peers.dpapi";

pub(in crate::lan::trust) struct Store {
    directory: PathBuf,
    _lock: File,
}

impl Store {
    pub(in crate::lan::trust) fn open(directory: &Path) -> Result<Self> {
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

    pub(super) fn read(&self, file: &str) -> Result<Option<Sensitive>> {
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

    pub(super) fn write(&self, file: &str, plaintext: &[u8]) -> Result<()> {
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

impl Store {
    pub(in crate::lan::trust) fn receive_destination(&self, folder: &Path) -> Option<PathBuf> {
        let path = crate::lan::destination(folder).ok()?;
        has_space(&path).then_some(path)
    }
}

fn has_space(folder: &Path) -> bool {
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
