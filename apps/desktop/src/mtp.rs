use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};

use receiver_service::lan::batch::{MAX_BATCH_BYTES, MAX_FILES};
use windows::Win32::Devices::PortableDevices::*;
use windows::Win32::Foundation::{
    E_ACCESSDENIED, GENERIC_READ, GENERIC_WRITE, HANDLE, PROPERTYKEY, S_FALSE, S_OK,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_TYPE_DISK,
    GetFileType,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, IStream, STGC_DEFAULT,
};
use windows::core::{GUID, Interface, PCWSTR, PWSTR, w};

const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;
const MAX_DEVICES: usize = 128;
const MAX_CHILDREN: usize = 65_536;
const ENUM_BATCH: usize = 16;

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    pnp_id: String,
    device_serial: Option<String>,
    storage_serial: Option<String>,
    storage_persistent_id: String,
    folder_persistent_id: String,
    folder_name: String,
}

#[derive(Clone, Eq)]
pub(crate) struct Target {
    identity: Identity,
    name: String,
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Target {
    pub(crate) fn name(&self) -> &str {
        &self.name
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
    Transfer,
    Unconfirmed,
}

impl Error {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "Windows couldn’t verify the USB file transfer connection.",
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
            Self::InvalidFile => "Choose a readable regular file with a safe filename.",
            Self::TooLarge => "Send over Cable supports files up to 64 MiB each.",
            Self::BatchTooLarge => "This batch is too large for Cable.",
            Self::DuplicateName => "Choose files with different names.",
            Self::NameConflict => "That name is already in use in the phone’s Download folder.",
            Self::Cancelled => "Send cancelled.",
            Self::Transfer => "Windows couldn’t transfer the file over USB.",
            Self::Unconfirmed => {
                "Couldn’t confirm the transfer. Check the phone’s Download folder before retrying."
            }
        }
    }
}

struct Apartment;

impl Apartment {
    fn enter() -> Result<Self, Error> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct TaskStrings<const N: usize>([PWSTR; N]);

impl<const N: usize> TaskStrings<N> {
    fn new() -> Self {
        Self([PWSTR::null(); N])
    }
}

impl<const N: usize> Drop for TaskStrings<N> {
    fn drop(&mut self) {
        for ptr in self.0 {
            if !ptr.is_null() {
                unsafe { CoTaskMemFree(Some(ptr.0.cast())) };
            }
        }
    }
}

fn text(ptr: PWSTR) -> Result<String, Error> {
    if ptr.is_null() {
        return Err(Error::Unavailable);
    }
    let s = unsafe { ptr.to_string() }.map_err(|_| Error::Unavailable)?;
    if s.is_empty() {
        return Err(Error::Unavailable);
    }
    Ok(s)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn values() -> windows::core::Result<IPortableDeviceValues> {
    unsafe { CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER) }
}

fn properties(
    source: &IPortableDeviceProperties,
    id: PCWSTR,
    keys: &[PROPERTYKEY],
) -> Result<IPortableDeviceValues, Error> {
    let get = || unsafe {
        let collection: IPortableDeviceKeyCollection =
            CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)?;
        for key in keys {
            collection.Add(key)?;
        }
        source.GetValues(id, &collection)
    };
    get().map_err(|_| Error::Unavailable)
}

fn string_property(values: &IPortableDeviceValues, key: &PROPERTYKEY) -> Result<String, Error> {
    let ptr = unsafe { values.GetStringValue(key) }.map_err(|_| Error::Unavailable)?;
    let owned = TaskStrings([ptr]);
    text(owned.0[0])
}

fn guid_property(values: &IPortableDeviceValues, key: &PROPERTYKEY) -> Result<GUID, Error> {
    unsafe { values.GetGuidValue(key) }.map_err(|_| Error::Unavailable)
}

fn number_property(values: &IPortableDeviceValues, key: &PROPERTYKEY) -> Result<u32, Error> {
    unsafe { values.GetUnsignedIntegerValue(key) }.map_err(|_| Error::Unavailable)
}

fn visit_children(
    content: &IPortableDeviceContent,
    parent: PCWSTR,
    cancel: &AtomicBool,
    mut visit: impl FnMut(PCWSTR) -> Result<(), Error>,
) -> Result<(), Error> {
    let children =
        unsafe { content.EnumObjects(0, parent, None) }.map_err(|_| Error::Unavailable)?;
    let mut total = 0;
    loop {
        check_cancel(cancel)?;
        let mut ids = TaskStrings::<ENUM_BATCH>::new();
        let mut fetched = 0;
        let hr = unsafe { children.Next(&mut ids.0, &mut fetched) };
        let fetched = fetched as usize;
        if !matches!(hr, S_OK | S_FALSE)
            || fetched > ENUM_BATCH
            || (hr == S_OK && fetched != ENUM_BATCH)
            || (hr == S_FALSE && fetched == ENUM_BATCH)
        {
            return Err(Error::Unavailable);
        }
        total += fetched;
        if total > MAX_CHILDREN {
            return Err(Error::Unavailable);
        }
        for ptr in &ids.0[..fetched] {
            check_cancel(cancel)?;
            if ptr.is_null() || unsafe { *ptr.0 } == 0 {
                return Err(Error::Unavailable);
            }
            visit(PCWSTR(ptr.0))?;
        }
        if hr == S_FALSE {
            return Ok(());
        }
    }
}

fn is_mtp(protocol: &str) -> bool {
    let protocol = protocol.trim();
    let token_len = protocol.find([':', ' ']).unwrap_or(protocol.len());
    let (token, rest) = protocol.split_at(token_len);
    if !token.eq_ignore_ascii_case("MTP") {
        return false;
    }
    let version = rest.trim().trim_start_matches(':').trim();
    rest.is_empty()
        || (!version.is_empty()
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
}

struct OpenDevice(IPortableDevice);

impl Drop for OpenDevice {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Close() };
    }
}

struct Storage {
    id: String,
    persistent_id: String,
    serial: Option<String>,
}

struct Candidate {
    content: IPortableDeviceContent,
    props: IPortableDeviceProperties,
    device: OpenDevice,
    pnp_id: String,
    serial: Option<String>,
    name: String,
    storage: Storage,
}

fn candidate(pnp_id: String, cancel: &AtomicBool) -> Result<Option<Candidate>, Error> {
    let connect = || unsafe {
        let client = values()?;
        client.SetStringValue(&WPD_CLIENT_NAME, w!("Sliploom"))?;
        client.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1)?;
        client.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0)?;
        client.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0)?;
        client.SetUnsignedIntegerValue(
            &WPD_CLIENT_DESIRED_ACCESS,
            GENERIC_READ.0 | GENERIC_WRITE.0,
        )?;
        let device: IPortableDevice =
            CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER)?;
        let id = wide(&pnp_id);
        device.Open(PCWSTR(id.as_ptr()), &client)?;
        Ok::<_, windows::core::Error>(OpenDevice(device))
    };
    let device = match connect() {
        Ok(device) => device,
        Err(e) if e.code() == E_ACCESSDENIED => return Ok(None),
        Err(_) => return Err(Error::Unavailable),
    };
    let actual_id = unsafe { device.0.GetPnPDeviceID() }.map_err(|_| Error::Unavailable)?;
    let actual_id = TaskStrings([actual_id]);
    if !text(actual_id.0[0])?.eq_ignore_ascii_case(&pnp_id) {
        return Err(Error::Unavailable);
    }
    let content = unsafe { device.0.Content() }.map_err(|_| Error::Unavailable)?;
    let props = unsafe { content.Properties() }.map_err(|_| Error::Unavailable)?;
    let root = properties(
        &props,
        WPD_DEVICE_OBJECT_ID,
        &[
            WPD_DEVICE_TRANSPORT,
            WPD_DEVICE_PROTOCOL,
            WPD_DEVICE_SERIAL_NUMBER,
            WPD_DEVICE_FRIENDLY_NAME,
        ],
    )?;
    let transport = number_property(&root, &WPD_DEVICE_TRANSPORT);
    let protocol = string_property(&root, &WPD_DEVICE_PROTOCOL);
    if protocol.as_ref().is_ok_and(|p| !is_mtp(p)) {
        return Ok(None);
    }
    match transport? as i32 {
        t if t == WPD_DEVICE_TRANSPORT_IP.0 || t == WPD_DEVICE_TRANSPORT_BLUETOOTH.0 => {
            return Ok(None);
        }
        t if t == WPD_DEVICE_TRANSPORT_USB.0 => (),
        _ => return Err(Error::Unavailable),
    }
    protocol?;
    let Some(storage) = primary_storage(&content, &props, cancel)? else {
        return Ok(None);
    };
    let serial = string_property(&root, &WPD_DEVICE_SERIAL_NUMBER).ok();
    let name = string_property(&root, &WPD_DEVICE_FRIENDLY_NAME)
        .ok()
        .map(|n| {
            n.chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect::<String>()
        })
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "USB device".into());
    Ok(Some(Candidate {
        content,
        props,
        device,
        pnp_id: pnp_id.to_ascii_lowercase(),
        serial,
        name,
        storage,
    }))
}

fn primary_storage(
    content: &IPortableDeviceContent,
    props: &IPortableDeviceProperties,
    cancel: &AtomicBool,
) -> Result<Option<Storage>, Error> {
    let mut selected = None;
    visit_children(content, WPD_DEVICE_OBJECT_ID, cancel, |id| {
        let v = properties(
            props,
            id,
            &[
                WPD_OBJECT_CONTENT_TYPE,
                WPD_FUNCTIONAL_OBJECT_CATEGORY,
                WPD_STORAGE_ACCESS_CAPABILITY,
                WPD_STORAGE_TYPE,
                WPD_OBJECT_PERSISTENT_UNIQUE_ID,
                WPD_STORAGE_SERIAL_NUMBER,
            ],
        )?;
        if guid_property(&v, &WPD_OBJECT_CONTENT_TYPE)? != WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT
            || guid_property(&v, &WPD_FUNCTIONAL_OBJECT_CATEGORY)?
                != WPD_FUNCTIONAL_CATEGORY_STORAGE
        {
            return Ok(());
        }
        match number_property(&v, &WPD_STORAGE_ACCESS_CAPABILITY)? as i32 {
            n if n == WPD_STORAGE_ACCESS_CAPABILITY_READ_ONLY_WITHOUT_OBJECT_DELETION.0
                || n == WPD_STORAGE_ACCESS_CAPABILITY_READ_ONLY_WITH_OBJECT_DELETION.0 =>
            {
                return Ok(());
            }
            n if n == WPD_STORAGE_ACCESS_CAPABILITY_READWRITE.0 => (),
            _ => return Err(Error::Unavailable),
        }
        if selected.is_some() {
            return Err(Error::AmbiguousDestination);
        }
        let fixed = number_property(&v, &WPD_STORAGE_TYPE)? == WPD_STORAGE_TYPE_FIXED_RAM.0 as u32;
        selected = Some((
            fixed,
            Storage {
                id: text(PWSTR(id.0.cast_mut()))?,
                persistent_id: string_property(&v, &WPD_OBJECT_PERSISTENT_UNIQUE_ID)?,
                serial: string_property(&v, &WPD_STORAGE_SERIAL_NUMBER).ok(),
            },
        ));
        Ok(())
    })?;
    match selected {
        Some((true, storage)) => Ok(Some(storage)),
        Some((false, _)) => Err(Error::NoDestination),
        None => Ok(None),
    }
}

struct Destination {
    target: Target,
    folder_id: String,
    content: IPortableDeviceContent,
    props: IPortableDeviceProperties,
    _device: OpenDevice,
}

fn locate(cancel: &AtomicBool) -> Result<Destination, Error> {
    check_cancel(cancel)?;
    let manager: IPortableDeviceManager =
        unsafe { CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| Error::Unavailable)?;
    let mut n = 0;
    unsafe {
        manager
            .RefreshDeviceList()
            .map_err(|_| Error::Unavailable)?;
        manager
            .GetDevices(null_mut(), &mut n)
            .map_err(|_| Error::Unavailable)?;
    }
    if n == 0 {
        return Err(Error::NoDevice);
    }
    let expected = n as usize;
    if expected > MAX_DEVICES {
        return Err(Error::Unavailable);
    }
    let mut ids = TaskStrings::<MAX_DEVICES>::new();
    n = MAX_DEVICES as u32;
    let hr = unsafe { (manager.vtable().GetDevices)(manager.as_raw(), ids.0.as_mut_ptr(), &mut n) };
    if hr != S_OK || n as usize != expected {
        return Err(Error::Unavailable);
    }
    let mut selected = None;
    for &id in &ids.0[..expected] {
        check_cancel(cancel)?;
        if let Some(device) = candidate(text(id)?, cancel)? {
            if selected.is_some() {
                return Err(Error::AmbiguousDevice);
            }
            selected = Some(device);
        }
    }
    destination(selected.ok_or(Error::NoDevice)?, cancel)
}

fn destination(device: Candidate, cancel: &AtomicBool) -> Result<Destination, Error> {
    let mut folder = None;
    let storage_id = wide(&device.storage.id);
    visit_children(&device.content, PCWSTR(storage_id.as_ptr()), cancel, |id| {
        let v = properties(
            &device.props,
            id,
            &[
                WPD_OBJECT_CONTENT_TYPE,
                WPD_OBJECT_NAME,
                WPD_OBJECT_ORIGINAL_FILE_NAME,
                WPD_OBJECT_PERSISTENT_UNIQUE_ID,
                WPD_OBJECT_PARENT_ID,
            ],
        )?;
        if guid_property(&v, &WPD_OBJECT_CONTENT_TYPE)? != WPD_CONTENT_TYPE_FOLDER {
            return Ok(());
        }
        let name = string_property(&v, &WPD_OBJECT_ORIGINAL_FILE_NAME)
            .or_else(|_| string_property(&v, &WPD_OBJECT_NAME))?;
        if name != "Download" {
            return Ok(());
        }
        if folder.is_some() {
            return Err(Error::AmbiguousDestination);
        }
        if string_property(&v, &WPD_OBJECT_PARENT_ID)? != device.storage.id {
            return Err(Error::Unavailable);
        }
        let persistent_id = string_property(&v, &WPD_OBJECT_PERSISTENT_UNIQUE_ID)?;
        if persistent_id == device.storage.persistent_id {
            return Err(Error::Unavailable);
        }
        folder = Some((text(PWSTR(id.0.cast_mut()))?, persistent_id, name));
        Ok(())
    })?;
    let (folder_id, folder_persistent_id, folder_name) = folder.ok_or(Error::NoDestination)?;
    Ok(Destination {
        target: Target {
            identity: Identity {
                pnp_id: device.pnp_id,
                device_serial: device.serial,
                storage_serial: device.storage.serial,
                storage_persistent_id: device.storage.persistent_id,
                folder_persistent_id,
                folder_name,
            },
            name: device.name,
        },
        folder_id,
        content: device.content,
        props: device.props,
        _device: device.device,
    })
}

pub(crate) fn detect(cancel: &AtomicBool) -> Result<Target, Error> {
    let _apartment = Apartment::enter()?;
    Ok(locate(cancel)?.target)
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn safe_name(path: &Path) -> Result<&str, Error> {
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && matches!(prefix.kind(), Prefix::DeviceNS(_) | Prefix::Verbatim(_))
    {
        return Err(Error::InvalidFile);
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(Error::InvalidFile)?;
    if name.is_empty()
        || name.encode_utf16().count() > 255
        || name.len() > 255
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(Error::InvalidFile);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|n| {
            matches!(
                n,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    {
        return Err(Error::InvalidFile);
    }
    Ok(name)
}

fn open_file(path: &Path) -> Result<(File, u64), Error> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|_| Error::InvalidFile)?;
    if unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK {
        return Err(Error::InvalidFile);
    }
    let meta = file.metadata().map_err(|_| Error::InvalidFile)?;
    if !meta.is_file() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(Error::InvalidFile);
    }
    if meta.len() > MAX_FILE_SIZE {
        return Err(Error::TooLarge);
    }
    Ok((file, meta.len()))
}

fn check_name(
    dest: &Destination,
    folder: PCWSTR,
    name: &str,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let folded = name.to_lowercase();
    visit_children(&dest.content, folder, cancel, |id| {
        let v = properties(
            &dest.props,
            id,
            &[
                WPD_OBJECT_ORIGINAL_FILE_NAME,
                WPD_OBJECT_NAME,
                WPD_OBJECT_CONTENT_TYPE,
            ],
        )?;
        let filename = string_property(&v, &WPD_OBJECT_ORIGINAL_FILE_NAME);
        let object_name = string_property(&v, &WPD_OBJECT_NAME);
        if [&filename, &object_name]
            .into_iter()
            .any(|s| s.as_ref().is_ok_and(|s| s.to_lowercase() == folded))
        {
            return Err(Error::NameConflict);
        }
        if filename.is_err()
            && (guid_property(&v, &WPD_OBJECT_CONTENT_TYPE)? != WPD_CONTENT_TYPE_FOLDER
                || object_name.is_err())
        {
            return Err(Error::Unavailable);
        }
        Ok(())
    })
}

struct PendingStream {
    stream: IStream,
    pending: bool,
}

impl PendingStream {
    fn revert(&mut self) -> Result<(), Error> {
        self.pending = false;
        unsafe { self.stream.Revert() }.map_err(|_| Error::Unconfirmed)
    }

    fn commit(mut self) -> Result<(), Error> {
        self.pending = false;
        unsafe { self.stream.Commit(STGC_DEFAULT) }.map_err(|_| Error::Unconfirmed)?;
        let data_stream = self
            .stream
            .cast::<IPortableDeviceDataStream>()
            .map_err(|_| Error::Unconfirmed)?;
        let id = unsafe { data_stream.GetObjectID() }.map_err(|_| Error::Unconfirmed)?;
        let owned = TaskStrings([id]);
        text(owned.0[0]).map_err(|_| Error::Unconfirmed)?;
        Ok(())
    }
}

impl Drop for PendingStream {
    fn drop(&mut self) {
        if self.pending {
            let _ = self.revert();
        }
    }
}

fn create_stream(
    content: &IPortableDeviceContent,
    folder: PCWSTR,
    name: &str,
    size: u64,
) -> Result<(PendingStream, usize), Error> {
    let prepare = || unsafe {
        let v = values()?;
        let name = wide(name);
        let name = PCWSTR(name.as_ptr());
        v.SetStringValue(&WPD_OBJECT_PARENT_ID, folder)?;
        v.SetStringValue(&WPD_OBJECT_NAME, name)?;
        v.SetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME, name)?;
        v.SetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE, size)?;
        v.SetGuidValue(&WPD_OBJECT_CONTENT_TYPE, &WPD_CONTENT_TYPE_GENERIC_FILE)?;
        v.SetGuidValue(&WPD_OBJECT_FORMAT, &WPD_OBJECT_FORMAT_UNSPECIFIED)?;
        Ok::<_, windows::core::Error>(v)
    };
    let v = prepare().map_err(|_| Error::Transfer)?;
    let mut stream = None;
    let mut optimal = 0;
    let result = unsafe {
        content.CreateObjectWithPropertiesAndData(&v, &mut stream, &mut optimal, null_mut())
    };
    let mut stream = stream.map(|stream| PendingStream {
        stream,
        pending: true,
    });
    if result.is_err() {
        if let Some(pending) = stream.as_mut() {
            pending.revert()?;
        }
        return Err(Error::Transfer);
    }
    let buffer_len = if optimal == 0 {
        64 * 1024
    } else {
        (optimal as usize).min(256 * 1024)
    };
    Ok((stream.ok_or(Error::Unconfirmed)?, buffer_len))
}

fn transfer(
    stream: &IStream,
    file: &mut File,
    size: u64,
    buffer_len: usize,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64),
) -> Result<(), Error> {
    let mut buffer = vec![0; buffer_len];
    let mut written = 0;
    while written < size {
        check_cancel(cancel)?;
        let count = (size - written).min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])
            .map_err(|_| Error::InvalidFile)?;
        check_cancel(cancel)?;
        let mut actual = 0;
        let hr = unsafe { stream.Write(buffer.as_ptr().cast(), count as u32, Some(&mut actual)) };
        if hr != S_OK || actual != count as u32 {
            return Err(Error::Transfer);
        }
        written += u64::from(actual);
        on_progress(written);
    }
    loop {
        check_cancel(cancel)?;
        match file.read(&mut [0]) {
            Ok(0) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::Interrupted => (),
            _ => return Err(Error::InvalidFile),
        }
    }
}

fn send(
    target: &Target,
    path: &Path,
    expected_size: u64,
    cancel: &AtomicBool,
    on_progress: impl FnMut(u64),
) -> Result<(), Error> {
    check_cancel(cancel)?;
    let name = safe_name(path)?;
    let (mut file, size) = open_file(path)?;
    if size != expected_size {
        return Err(Error::InvalidFile);
    }
    let _apartment = Apartment::enter()?;
    let dest = locate(cancel).map_err(|e| match e {
        Error::NoDevice | Error::NoDestination => Error::Changed,
        other => other,
    })?;
    if *target != dest.target {
        return Err(Error::Changed);
    }
    let folder_id = wide(&dest.folder_id);
    let folder = PCWSTR(folder_id.as_ptr());
    check_name(&dest, folder, name, cancel)?;
    check_cancel(cancel)?;
    let (mut pending, buffer_len) = create_stream(&dest.content, folder, name, size)?;
    if let Err(err) = transfer(
        &pending.stream,
        &mut file,
        size,
        buffer_len,
        cancel,
        on_progress,
    )
    .and_then(|()| check_cancel(cancel))
    {
        pending.revert()?;
        return Err(err);
    }
    pending.commit()
}

pub(crate) struct BatchCopy {
    pub(crate) confirmed: usize,
    pub(crate) result: Result<(), Error>,
}

fn preflight(paths: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<u64>, Error> {
    if paths.is_empty() || paths.len() > MAX_FILES {
        return Err(Error::BatchTooLarge);
    }
    let mut names = HashSet::with_capacity(paths.len());
    let mut sizes = Vec::with_capacity(paths.len());
    let mut total = 0u64;
    for path in paths {
        check_cancel(cancel)?;
        if !names.insert(safe_name(path)?.to_lowercase()) {
            return Err(Error::DuplicateName);
        }
        let (_, size) = open_file(path)?;
        total = total.checked_add(size).ok_or(Error::BatchTooLarge)?;
        if total > MAX_BATCH_BYTES {
            return Err(Error::BatchTooLarge);
        }
        sizes.push(size);
    }
    Ok(sizes)
}

pub(crate) fn send_batch(
    target: &Target,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(usize, u64),
) -> BatchCopy {
    let mut confirmed = 0;
    let mut bytes = 0;
    let result = (|| {
        let sizes = preflight(paths, cancel)?;
        for (index, (path, size)) in paths.iter().zip(sizes).enumerate() {
            send(target, path, size, cancel, |sent| {
                on_progress(index, bytes + sent);
            })?;
            confirmed = index + 1;
            bytes += size;
            on_progress(confirmed, bytes);
        }
        Ok(())
    })();
    BatchCopy { confirmed, result }
}
