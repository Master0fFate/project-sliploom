#![cfg(all(unix, feature = "unix-mtp"))]

use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

use receiver_service::lan::batch::{MAX_BATCH_BYTES, MAX_FILES};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
static SESSION: Mutex<()> = Mutex::new(());
static INIT: Once = Once::new();

#[repr(C)]
struct NativeTarget {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn sliploom_mtp_init();
    fn sliploom_mtp_detect(
        cancelled: extern "C" fn(*const c_void) -> c_int,
        context: *const c_void,
        target: *mut *mut NativeTarget,
    ) -> c_int;
    fn sliploom_mtp_identity(target: *const NativeTarget, ids: *mut u32) -> *const c_char;
    fn sliploom_mtp_free(target: *mut NativeTarget);
    fn sliploom_mtp_send(
        serial: *const c_char,
        ids: *const u32,
        name: *const c_char,
        size: u64,
        cancelled: extern "C" fn(*const c_void) -> c_int,
        cancel_context: *const c_void,
        read: extern "C" fn(*mut c_void, u32, *mut u8) -> c_int,
        progress: extern "C" fn(*mut c_void, u64) -> c_int,
        io_context: *mut c_void,
    ) -> c_int;
}

struct Snapshot(NonNull<NativeTarget>);

impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe { sliploom_mtp_free(self.0.as_ptr()) };
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Target {
    serial: CString,
    ids: [u32; 5],
}

impl Target {
    pub(crate) fn name(&self) -> &str {
        "USB MTP device"
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Unavailable,
    NoDevice,
    AmbiguousDevice,
    NoDestination,
    AmbiguousDestination,
    Changed,
    InvalidFile,
    TooLarge,
    BatchTooLarge,
    DuplicateName,
    NameConflict,
    Cancelled,
    Unconfirmed,
}

impl Error {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "Couldn’t verify the USB file transfer connection.",
            Self::NoDevice => "Connect and unlock a phone, then choose USB File transfer (MTP).",
            Self::AmbiguousDevice => {
                "More than one USB file transfer device is available. Disconnect the others."
            }
            Self::NoDestination => {
                "Couldn’t verify a Download folder on writable internal storage."
            }
            Self::AmbiguousDestination => {
                "More than one writable storage or Download folder is available."
            }
            Self::Changed => {
                "The USB device or Download folder changed. Check the connection again."
            }
            Self::InvalidFile => {
                "Choose a readable, non-empty regular file with a safe ASCII name."
            }
            Self::TooLarge => "Send over Cable supports files up to 64 MiB each.",
            Self::BatchTooLarge => "This batch is too large for Cable.",
            Self::DuplicateName => "Choose files with different names.",
            Self::NameConflict => "That name is already in use in the phone’s Download folder.",
            Self::Cancelled => "Send cancelled.",
            Self::Unconfirmed => {
                "Couldn’t confirm the transfer. Check the phone’s Download folder before retrying."
            }
        }
    }
}

extern "C" fn cancelled(context: *const c_void) -> c_int {
    let cancel = unsafe { &*context.cast::<AtomicBool>() };
    c_int::from(cancel.load(Ordering::Acquire))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn status_result(status: c_int) -> Result<(), Error> {
    match status {
        0 => Ok(()),
        2 => Err(Error::NoDevice),
        3 => Err(Error::AmbiguousDevice),
        4 => Err(Error::NoDestination),
        5 => Err(Error::AmbiguousDestination),
        6 => Err(Error::Cancelled),
        7 => Err(Error::Changed),
        8 => Err(Error::NameConflict),
        9 => Err(Error::Unconfirmed),
        _ => Err(Error::Unavailable),
    }
}

pub(crate) fn detect(cancel: &AtomicBool) -> Result<Target, Error> {
    check_cancel(cancel)?;
    let _session = SESSION.try_lock().map_err(|_| Error::Unavailable)?;
    INIT.call_once(|| unsafe { sliploom_mtp_init() });
    let mut raw = null_mut();
    let status =
        unsafe { sliploom_mtp_detect(cancelled, (cancel as *const AtomicBool).cast(), &mut raw) };
    let snapshot = NonNull::new(raw).map(Snapshot);
    status_result(status)?;
    let snapshot = snapshot.ok_or(Error::Unavailable)?;
    let mut ids = [0; 5];
    let serial = unsafe { sliploom_mtp_identity(snapshot.0.as_ptr(), ids.as_mut_ptr()) };
    if serial.is_null() {
        return Err(Error::Unavailable);
    }
    let serial = unsafe { CStr::from_ptr(serial) }.to_owned();
    Ok(Target { serial, ids })
}

pub(crate) struct BatchCopy {
    pub(crate) confirmed: usize,
    pub(crate) result: Result<(), Error>,
}

struct Source {
    file: File,
    name: CString,
    size: u64,
}

fn safe_name(path: &Path) -> Result<&str, Error> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::InvalidFile)?;
    if name.len() > 254
        || !name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || name.ends_with([' ', '.'])
        || !name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b" ._-()".contains(&ch))
    {
        return Err(Error::InvalidFile);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit())
    {
        return Err(Error::InvalidFile);
    }
    Ok(name)
}

fn preflight(paths: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<Source>, Error> {
    check_cancel(cancel)?;
    if !(1..=MAX_FILES).contains(&paths.len()) {
        return Err(Error::BatchTooLarge);
    }
    let mut names = HashSet::with_capacity(paths.len());
    let mut sources = Vec::with_capacity(paths.len());
    let mut total = 0;
    for path in paths {
        check_cancel(cancel)?;
        let name = safe_name(path)?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(Error::DuplicateName);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| Error::InvalidFile)?;
        let metadata = file.metadata().map_err(|_| Error::InvalidFile)?;
        let size = metadata.len();
        if !metadata.is_file() || size == 0 {
            return Err(Error::InvalidFile);
        }
        if size > MAX_FILE_BYTES {
            return Err(Error::TooLarge);
        }
        total += size;
        if total > MAX_BATCH_BYTES {
            return Err(Error::BatchTooLarge);
        }
        let name = CString::new(name).map_err(|_| Error::InvalidFile)?;
        sources.push(Source { file, name, size });
    }
    Ok(sources)
}

struct Upload<'a, F> {
    source: &'a Source,
    cancel: &'a AtomicBool,
    progress: &'a mut F,
    offset: u64,
    index: usize,
    base: u64,
}

extern "C" fn read_source<F>(context: *mut c_void, length: u32, data: *mut u8) -> c_int {
    let upload = unsafe { &mut *context.cast::<Upload<'_, F>>() };
    if u64::from(length) > upload.source.size - upload.offset || data.is_null() {
        return 1;
    }
    let length = length as usize;
    let mut read = 0;
    while read < length {
        if upload.cancel.load(Ordering::Acquire) {
            return 1;
        }
        let n = unsafe {
            libc::pread(
                upload.source.file.as_raw_fd(),
                data.add(read).cast(),
                length - read,
                upload.offset as libc::off_t,
            )
        };
        if n > 0 {
            read += n as usize;
            upload.offset += n as u64;
        } else if n == 0 || io::Error::last_os_error().kind() != ErrorKind::Interrupted {
            return 1;
        }
    }
    c_int::from(upload.cancel.load(Ordering::Acquire))
}

extern "C" fn progress<F: FnMut(usize, u64)>(context: *mut c_void, payload: u64) -> c_int {
    let upload = unsafe { &mut *context.cast::<Upload<'_, F>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        (upload.progress)(upload.index, upload.base + payload);
    })) {
        Ok(()) => 0,
        Err(panic) => {
            std::mem::forget(panic);
            1
        }
    }
}

pub(crate) fn send_batch<F: FnMut(usize, u64)>(
    target: &Target,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    mut on_progress: F,
) -> BatchCopy {
    let mut confirmed = 0;
    let result = (|| {
        let sources = preflight(paths, cancel)?;
        check_cancel(cancel)?;
        let _session = SESSION.try_lock().map_err(|_| Error::Unavailable)?;
        INIT.call_once(|| unsafe { sliploom_mtp_init() });
        let mut bytes = 0;
        for (index, source) in sources.iter().enumerate() {
            check_cancel(cancel)?;
            if source
                .file
                .metadata()
                .map_err(|_| Error::InvalidFile)?
                .len()
                != source.size
            {
                return Err(Error::InvalidFile);
            }
            let mut upload = Upload {
                source,
                cancel,
                progress: &mut on_progress,
                offset: 0,
                index,
                base: bytes,
            };
            let status = unsafe {
                sliploom_mtp_send(
                    target.serial.as_ptr(),
                    target.ids.as_ptr(),
                    source.name.as_ptr(),
                    source.size,
                    cancelled,
                    (cancel as *const AtomicBool).cast(),
                    read_source::<F>,
                    progress::<F>,
                    (&mut upload as *mut Upload<'_, F>).cast(),
                )
            };
            status_result(status)?;
            if upload.offset != source.size
                || source
                    .file
                    .metadata()
                    .map_err(|_| Error::Unconfirmed)?
                    .len()
                    != source.size
            {
                return Err(Error::Unconfirmed);
            }
            confirmed = index + 1;
            bytes += source.size;
            on_progress(confirmed, bytes);
        }
        Ok(())
    })();
    BatchCopy { confirmed, result }
}
