use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use receiver_service::lan::cable::{CableReceiver, Offer};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::semantics::Live;
use zgx_desktop::{TaskSpawner, WindowContext};

use crate::adb::AdbBridge;
use crate::material::{action, headline, metadata, secondary};

const TICK: Duration = Duration::from_millis(100);
const DECISION_TIME: Duration = Duration::from_secs(30);
const BRIDGE_CHECK: Duration = Duration::from_secs(5);
const WAITING: &str = "Cable ready. On your phone, use Share → SEAM.";

#[derive(Clone)]
struct Prompt {
    deadline: Instant,
    is_pdf: bool,
    bytes: Option<u64>,
    answer: SyncSender<bool>,
}

impl PartialEq for Prompt {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiptOutcome {
    Saved(u64),
    SavedWithUncertainConfirmation(u64),
}

impl ReceiptOutcome {
    fn message(self) -> String {
        match self {
            Self::Saved(bytes) => format!("Saved here ({bytes} bytes)."),
            Self::SavedWithUncertainConfirmation(bytes) => {
                format!("Saved here ({bytes} bytes). The phone may not have received confirmation.")
            }
        }
    }
}

enum Event {
    Status(String),
    Folder(PathBuf),
    Prompt(Prompt),
    Receipt(ReceiptOutcome),
}

#[derive(Default, Clone)]
pub(crate) struct Workers(Rc<RefCell<Option<Worker>>>);

struct Worker {
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Workers {
    fn cancel(&self) {
        if let Some(worker) = self.0.borrow().as_ref() {
            worker.cancel.store(true, Ordering::Release);
        }
    }

    pub(crate) fn shutdown(&self) {
        self.cancel();
        if let Some(worker) = self.0.borrow_mut().take() {
            let _ = worker.handle.join();
        }
    }
}

#[derive(Clone)]
pub(crate) struct Cable {
    enabled: Signal<bool>,
    status: Signal<String>,
    folder: Signal<Option<PathBuf>>,
    prompt: Signal<Option<Prompt>>,
    receipt: Signal<Option<ReceiptOutcome>>,
    retry: Signal<bool>,
    workers: Workers,
    tasks: TaskSpawner,
}

impl Cable {
    pub(crate) fn new(cx: &mut WindowContext, workers: Workers) -> Self {
        let runtime = &cx.ui.runtime;
        let cable = Self {
            enabled: runtime.signal(true),
            status: runtime.signal(String::new()),
            folder: runtime.signal(None),
            prompt: runtime.signal(None),
            receipt: runtime.signal(None),
            retry: runtime.signal(false),
            workers,
            tasks: cx.tasks.clone(),
        };
        let close = cable.clone();
        cx.on_closed(move || close.enable(false));
        cable.start();
        cable
    }

    pub(crate) fn enable(&self, enabled: bool) {
        self.enabled.set(enabled);
        if enabled {
            self.start();
        } else {
            self.workers.cancel();
            self.prompt.set(None);
            self.folder.set(None);
            self.retry.set(false);
            self.status.set("Preparing cable receive…".into());
        }
    }

    fn start(&self) {
        if !self.enabled.get() || self.workers.0.borrow().is_some() {
            return;
        }
        self.retry.set(false);
        self.status.set("Preparing cable receive…".into());
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let handle = thread::Builder::new()
            .name("seam-cable".into())
            .spawn(move || run(&flag, &tx));
        match handle {
            Ok(handle) => {
                *self.workers.0.borrow_mut() = Some(Worker {
                    cancel: cancel.clone(),
                    handle,
                });
            }
            Err(_) => {
                self.status.set("Couldn’t start cable receive.".into());
                self.retry.set(true);
                return;
            }
        }
        let ui = self.clone();
        self.tasks.spawn(async move {
            ui.read_events(&cancel, rx).await;
            loop {
                if ui
                    .workers
                    .0
                    .borrow()
                    .as_ref()
                    .is_some_and(|worker| worker.handle.is_finished())
                {
                    let worker = ui.workers.0.borrow_mut().take().unwrap();
                    let failed = worker.handle.join().is_err();
                    ui.prompt.set(None);
                    if ui.enabled.get() {
                        if stopped(&cancel) {
                            ui.start();
                        } else {
                            if failed {
                                ui.status.set("Cable receive stopped unexpectedly.".into());
                            }
                            ui.retry.set(true);
                        }
                    }
                    return;
                }
                zgx::timer::sleep(TICK).await;
            }
        });
    }

    async fn read_events(&self, cancel: &AtomicBool, rx: Receiver<Event>) {
        while !stopped(cancel) {
            match rx.try_recv() {
                Ok(Event::Status(message)) => {
                    self.prompt.set(None);
                    self.status.set(message);
                }
                Ok(Event::Folder(path)) => {
                    self.folder.set(Some(path));
                }
                Ok(Event::Prompt(prompt)) => {
                    self.receipt.set(None);
                    if prompt.deadline > Instant::now() {
                        self.prompt.set(Some(prompt));
                        self.status.set(String::new());
                    }
                }
                Ok(Event::Receipt(receipt)) => {
                    self.status.set(WAITING.into());
                    self.receipt.set(Some(receipt));
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => zgx::timer::sleep(TICK).await,
            }
        }
    }

    fn decide(&self, prompt: &Prompt, accepted: bool) {
        if !self.enabled.get() || !self.prompt.with(|current| current.as_ref() == Some(prompt)) {
            return;
        }
        self.prompt.set(None);
        if Instant::now() < prompt.deadline && prompt.answer.try_send(accepted).is_ok() {
            let message = if accepted {
                "Receiving…"
            } else {
                "Declined."
            };
            self.status.set(message.into());
        }
    }

    pub(crate) fn view(&self) -> View {
        let prompt = self.prompt.clone();
        let ui = self.clone();
        let status = self.status.clone();
        let retry = self.retry.clone();
        let folder = self.folder.clone();
        switch(
            move || prompt.get(),
            move |prompt, _| {
                if let Some(prompt) = prompt {
                    return ui.prompt_view(prompt);
                }
                let status = status.clone();
                let receipt = ui.receipt.clone();
                let actions = ui.clone();
                let retry = retry.clone();
                let folder = folder.clone();
                column()
                    .gap(16.)
                    .child(
                        column()
                            .gap(8.)
                            .child(headline("Receive by cable"))
                            .child(metadata(text_signal(move || status.get())).live(Live::Polite))
                            .child_when(
                                move || receipt.get(),
                                |receipt, _| metadata(text(receipt.message())).live(Live::Polite),
                            ),
                    )
                    .child(switch(
                        move || (retry.get(), folder.with(Option::is_some)),
                        move |(retry, has_folder), _| {
                            let retry_ui = actions.clone();
                            let folder_ui = actions.clone();
                            row()
                                .gap(8.)
                                .when(retry, |row| {
                                    row.child(
                                        secondary("Retry cable").on_click(move || retry_ui.start()),
                                    )
                                })
                                .when(has_folder, |row| {
                                    row.child(secondary("Open folder").on_click(move || {
                                        if let Some(path) = folder_ui.folder.get() {
                                            let _ = thread::Builder::new()
                                                .name("seam-open-folder".into())
                                                .spawn(move || {
                                                    if let Ok(folder) = private_folder::open(false)
                                                        && folder.path == path
                                                    {
                                                        folder.explore();
                                                    }
                                                });
                                        }
                                    }))
                                })
                        },
                    ))
            },
        )
    }

    fn prompt_view(&self, prompt: Prompt) -> View {
        let decline = self.clone();
        let accept = self.clone();
        let question = if prompt.is_pdf {
            "Receive PDF?"
        } else {
            "Receive image?"
        };
        let detail = prompt.bytes.map_or_else(
            || "Cable share · Size unknown".into(),
            |bytes| format!("Cable share · {bytes} bytes"),
        );
        let no = prompt.clone();
        column()
            .gap(24.)
            .child(
                column()
                    .gap(8.)
                    .child(headline(question).live(Live::Assertive))
                    .child(metadata(text(detail))),
            )
            .child(
                row()
                    .gap(8.)
                    .child(
                        secondary("Decline")
                            .grow()
                            .h(48.)
                            .on_click(move || decline.decide(&no, false)),
                    )
                    .child(
                        action(text("Accept"), true)
                            .grow()
                            .h(48.)
                            .on_click(move || accept.decide(&prompt, true)),
                    ),
            )
    }
}

fn stopped(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Acquire)
}

fn status(tx: &Sender<Event>, message: impl Into<String>) {
    let _ = tx.send(Event::Status(message.into()));
}

fn pause(cancel: &AtomicBool, duration: Duration) {
    let until = Instant::now() + duration;
    while !stopped(cancel) {
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(remaining.min(TICK));
    }
}

fn run(cancel: &AtomicBool, tx: &Sender<Event>) {
    let Ok(folder) = private_folder::open(true) else {
        status(
            tx,
            "Can’t use the private SEAM folder. Cable receive is off.",
        );
        return;
    };
    let _ = tx.send(Event::Folder(folder.path.clone()));
    let retry_delays = [0, 1, 2, 4, 8, 12, 16];
    let mut failures = 0;
    while let Some(&delay) = retry_delays.get(failures) {
        pause(cancel, Duration::from_secs(delay));
        if stopped(cancel) {
            return;
        }
        let Ok(receiver) = CableReceiver::bind(&folder.path) else {
            status(
                tx,
                "Couldn’t start cable receive in the private SEAM folder.",
            );
            return;
        };
        let Ok(bridge) = AdbBridge::open(receiver.port()) else {
            status(tx, "Waiting for an authorized USB connection…");
            failures += 1;
            continue;
        };
        if stopped(cancel) {
            return;
        }
        failures = 0;
        status(tx, WAITING);
        let connection_cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            let connection = &connection_cancel;
            let receiving = scope.spawn(move || receive(receiver, cancel, connection, tx));
            let mut next_check = Instant::now() + BRIDGE_CHECK;
            while !stopped(cancel) && !receiving.is_finished() {
                if Instant::now() >= next_check {
                    if !bridge.connected() {
                        break;
                    }
                    next_check = Instant::now() + BRIDGE_CHECK;
                }
                thread::sleep(TICK);
            }
            connection_cancel.store(true, Ordering::Release);
        });
        drop(bridge);
        if !stopped(cancel) {
            status(tx, "Cable disconnected. Reconnecting…");
        }
    }
    status(
        tx,
        "Cable not ready. Connect your phone by USB, then choose Retry cable.",
    );
}

fn receive(
    receiver: CableReceiver,
    cancel: &AtomicBool,
    connection: &AtomicBool,
    tx: &Sender<Event>,
) {
    while !stopped(cancel) && !stopped(connection) {
        let mut outcome = "No file was saved. Share again from the phone to retry.";
        let mut next_progress = Instant::now();
        let result = receiver.receive_next(
            cancel,
            connection,
            |offer| {
                let (accepted, message) = approve(offer, cancel, connection, tx);
                outcome = message;
                if accepted {
                    status(tx, "Receiving…");
                }
                accepted
            },
            |bytes| {
                if Instant::now() >= next_progress {
                    status(tx, format!("Receiving… {bytes} bytes"));
                    next_progress = Instant::now() + Duration::from_millis(500);
                }
            },
        );
        match result {
            Ok(Some(receipt)) => {
                let outcome = if receipt.confirmed {
                    ReceiptOutcome::Saved(receipt.bytes)
                } else {
                    ReceiptOutcome::SavedWithUncertainConfirmation(receipt.bytes)
                };
                let _ = tx.send(Event::Receipt(outcome));
            }
            _ if stopped(cancel) || stopped(connection) => return,
            Ok(None) => status(tx, outcome),
            Err(_) => {
                status(tx, outcome);
                pause(connection, Duration::from_millis(500));
            }
        }
    }
}

fn approve(
    offer: Offer,
    cancel: &AtomicBool,
    connection: &AtomicBool,
    tx: &Sender<Event>,
) -> (bool, &'static str) {
    let deadline = Instant::now() + DECISION_TIME;
    let (answer, decision) = mpsc::sync_channel(1);
    if tx
        .send(Event::Prompt(Prompt {
            deadline,
            is_pdf: offer.is_pdf,
            bytes: offer.bytes,
            answer,
        }))
        .is_err()
    {
        return (false, "Declined.");
    }
    loop {
        if stopped(cancel) || stopped(connection) {
            return (false, "Declined.");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return (false, "Offer expired. Share again from the phone to retry.");
        }
        match decision.recv_timeout(TICK.min(remaining)) {
            Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => return (false, "Declined."),
            Ok(true) if !stopped(cancel) && !stopped(connection) && Instant::now() < deadline => {
                return (
                    true,
                    "No file was saved. Share again from the phone to retry.",
                );
            }
            Ok(true) => return (false, "Offer expired. Share again from the phone to retry."),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

mod private_folder {
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Component, Path, PathBuf, Prefix};
    use std::ptr::{null, null_mut};

    type Ptr = *mut c_void;
    const REPARSE_POINT: u32 = 0x400;
    const DIRECTORY: u32 = 0x10;
    const PROTECTED_DACL: u16 = 0x1000;

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const PROFILE: Guid = Guid(
        0x5e6c858f,
        0x0e22,
        0x4760,
        [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
    );
    const LOCAL_APP_DATA: Guid = Guid(
        0xf1b32785,
        0x6fba,
        0x4fcf,
        [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
    );

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: Ptr,
        inherit: i32,
    }

    #[repr(C)]
    struct Acl {
        revision: u8,
        reserved: u8,
        size: u16,
        count: u16,
        reserved2: u16,
    }

    #[repr(C)]
    struct AceHeader {
        kind: u8,
        flags: u8,
        size: u16,
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: Ptr, desired: u32, token: *mut Ptr) -> i32;
        fn GetTokenInformation(
            token: Ptr,
            class: u32,
            buffer: Ptr,
            length: u32,
            needed: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: Ptr, text: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut Ptr,
            size: *mut u32,
        ) -> i32;
        fn GetSecurityInfo(
            handle: Ptr,
            object: u32,
            info: u32,
            owner: *mut Ptr,
            group: *mut Ptr,
            dacl: *mut *mut Acl,
            sacl: *mut *mut Acl,
            descriptor: *mut Ptr,
        ) -> u32;
        fn GetSecurityDescriptorOwner(descriptor: Ptr, owner: *mut Ptr, defaulted: *mut i32)
        -> i32;
        fn GetSecurityDescriptorDacl(
            descriptor: Ptr,
            present: *mut i32,
            acl: *mut *mut Acl,
            defaulted: *mut i32,
        ) -> i32;
        fn GetSecurityDescriptorControl(
            descriptor: Ptr,
            control: *mut u16,
            revision: *mut u32,
        ) -> i32;
        fn EqualSid(first: Ptr, second: Ptr) -> i32;
        fn GetAce(acl: *mut Acl, index: u32, ace: *mut *mut AceHeader) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Ptr;
        fn LocalFree(memory: Ptr) -> Ptr;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn CreateDirectoryW(path: *const u16, attributes: *const SecurityAttributes) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            id: *const Guid,
            flags: u32,
            token: Ptr,
            path: *mut *mut u16,
        ) -> i32;
        fn ShellExecuteW(
            window: Ptr,
            operation: *const u16,
            file: *const u16,
            params: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: Ptr);
        fn CoInitializeEx(reserved: Ptr, flags: u32) -> i32;
        fn CoUninitialize();
    }

    struct Local(Ptr);

    impl Drop for Local {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0) };
        }
    }

    pub(super) struct Folder {
        pub(super) path: PathBuf,
        _directories: Vec<File>,
    }

    impl Folder {
        pub(super) fn explore(&self) {
            let verb = wide(OsStr::new("open"));
            let path = wide(self.path.as_os_str());
            unsafe {
                if CoInitializeEx(null_mut(), 2) >= 0 {
                    ShellExecuteW(null_mut(), verb.as_ptr(), path.as_ptr(), null(), null(), 1);
                    CoUninitialize();
                }
            }
        }
    }

    pub(super) fn open(create: bool) -> io::Result<Folder> {
        let base = known_folder(&LOCAL_APP_DATA)?;
        let profile = known_folder(&PROFILE)?;
        if base == profile || !base.starts_with(&profile) {
            return Err(invalid());
        }
        let mut components = base.components();
        let root_is_local = match (components.next(), components.next()) {
            (Some(Component::Prefix(prefix)), Some(Component::RootDir)) => {
                if let Prefix::Disk(drive) = prefix.kind() {
                    let root = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
                    unsafe { GetDriveTypeW(root.as_ptr()) == 3 }
                } else {
                    false
                }
            }
            _ => false,
        };
        if !root_is_local || !components.all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(invalid());
        }
        let mut directories = base
            .ancestors()
            .filter(|path| path.parent().is_some())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|path| lock_directory(path, false))
            .collect::<io::Result<Vec<_>>>()?;
        let expected = private_descriptor()?;
        let path = base.join("SEAM");
        if create {
            let attributes = SecurityAttributes {
                length: std::mem::size_of::<SecurityAttributes>() as u32,
                descriptor: expected.0,
                inherit: 0,
            };
            if unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes) } == 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        let folder = lock_directory(&path, true)?;
        check_descriptor(&folder, &expected)?;
        directories.push(folder);
        Ok(Folder {
            path,
            _directories: directories,
        })
    }

    fn lock_directory(path: &Path, check_security: bool) -> io::Result<File> {
        let handle = OpenOptions::new()
            .access_mode(0x81 | if check_security { 0x20000 } else { 0 })
            .share_mode(3)
            .custom_flags(0x02200000)
            .open(path)?;
        if handle.metadata()?.file_attributes() & (DIRECTORY | REPARSE_POINT) != DIRECTORY {
            return Err(invalid());
        }
        Ok(handle)
    }

    fn known_folder(id: &Guid) -> io::Result<PathBuf> {
        let mut pointer = null_mut();
        unsafe {
            let result = SHGetKnownFolderPath(id, 0, null_mut(), &mut pointer);
            let path = if result >= 0 && !pointer.is_null() {
                Ok(PathBuf::from(wide_string(pointer)))
            } else {
                Err(invalid())
            };
            CoTaskMemFree(pointer.cast());
            path
        }
    }

    fn private_descriptor() -> io::Result<Local> {
        unsafe {
            let mut token = null_mut();
            check(OpenProcessToken(GetCurrentProcess(), 8, &mut token))?;
            let token = OwnedHandle::from_raw_handle(token);
            let mut needed = 0;
            let handle = token.as_raw_handle();
            GetTokenInformation(handle, 1, null_mut(), 0, &mut needed);
            let word = std::mem::size_of::<usize>();
            if needed < word as u32 {
                return Err(invalid());
            }
            let mut buffer = vec![0usize; (needed as usize).div_ceil(word)];
            let ptr = buffer.as_mut_ptr().cast::<c_void>();
            check(GetTokenInformation(handle, 1, ptr, needed, &mut needed))?;
            let mut sid = null_mut();
            check(ConvertSidToStringSidW(*ptr.cast::<Ptr>(), &mut sid))?;
            let sid_allocation = Local(sid.cast());
            let sid = wide_string(sid_allocation.0.cast());
            let sid = sid.to_str().ok_or_else(invalid)?;
            let sddl = wide(OsStr::new(&format!("O:{sid}D:P(A;OICI;FA;;;{sid})")));
            let mut descriptor = null_mut();
            check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            ))?;
            Ok(Local(descriptor))
        }
    }

    fn check_descriptor(folder: &File, expected: &Local) -> io::Result<()> {
        unsafe {
            let (mut owner, mut wanted_owner, mut descriptor) =
                (null_mut(), null_mut(), null_mut());
            let (mut acl, mut wanted_acl) = (null_mut(), null_mut());
            let code = GetSecurityInfo(
                folder.as_raw_handle(),
                1,
                5,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            );
            let descriptor = Local(descriptor);
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
            let (mut defaulted, mut present, mut revision, mut control) = (0, 0, 0, 0);
            check(GetSecurityDescriptorControl(
                descriptor.0,
                &mut control,
                &mut revision,
            ))?;
            check(GetSecurityDescriptorOwner(
                expected.0,
                &mut wanted_owner,
                &mut defaulted,
            ))?;
            check(GetSecurityDescriptorDacl(
                expected.0,
                &mut present,
                &mut wanted_acl,
                &mut defaulted,
            ))?;
            if control & PROTECTED_DACL == 0
                || owner.is_null()
                || wanted_owner.is_null()
                || EqualSid(owner, wanted_owner) == 0
                || acl.is_null()
                || wanted_acl.is_null()
                || (*acl).count != 1
                || (*wanted_acl).count != 1
            {
                return Err(invalid());
            }
            let (mut ace, mut wanted_ace) = (null_mut(), null_mut());
            check(GetAce(acl, 0, &mut ace))?;
            check(GetAce(wanted_acl, 0, &mut wanted_ace))?;
            let bytes = |ace: *const AceHeader| {
                std::slice::from_raw_parts(ace.cast::<u8>(), (*ace).size as usize)
            };
            if bytes(ace) != bytes(wanted_ace) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain([0]).collect()
    }

    unsafe fn wide_string(value: *const u16) -> OsString {
        let mut count = 0;
        unsafe {
            while *value.add(count) != 0 {
                count += 1;
            }
            OsString::from_wide(std::slice::from_raw_parts(value, count))
        }
    }

    fn check(result: i32) -> io::Result<()> {
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn invalid() -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, "not a private SEAM folder")
    }
}
