use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use receiver_service::lan::Receipt;
use receiver_service::lan::cable::{CableReceiver, Offer};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::semantics::Live;
use zgx_desktop::{FileDialogs, TaskSpawner, WindowContext};

use crate::adb::AdbBridge;
use crate::history::History;
use crate::material::{
    headline, metadata, primary_action, quiet_action, receive_actions, receive_primary,
};
use crate::{destination_label, inline_status, pick_destination};

const TICK: Duration = Duration::from_millis(100);
const DECISION_TIME: Duration = Duration::from_secs(30);
const BRIDGE_CHECK: Duration = Duration::from_secs(5);
const WAITING: &str = "Cable ready. On your phone, use Share → Sliploom.";
const IDLE: u8 = 0;
const BUSY: u8 = 1;
const PAUSED: u8 = 2;

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
            Self::Saved(bytes) => format!("Last save: {bytes} bytes."),
            Self::SavedWithUncertainConfirmation(bytes) => {
                format!("Last save: {bytes} bytes. The phone may not have received confirmation.")
            }
        }
    }
}

enum Event {
    Status(String),
    DefaultFolder(PathBuf),
    Busy(bool),
    Prompt(Prompt),
    Receipt(Receipt),
}

#[derive(Default, Clone)]
pub(crate) struct Workers(Rc<RefCell<Option<Worker>>>);

struct Worker {
    cancel: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
    handle: JoinHandle<()>,
}

impl Workers {
    fn pause_idle(&self) -> bool {
        let worker = self.0.borrow();
        let Some(worker) = worker.as_ref() else {
            return true;
        };
        if worker
            .phase
            .compare_exchange(IDLE, PAUSED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        worker.cancel.store(true, Ordering::Release);
        true
    }

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
    default_folder: Signal<Option<PathBuf>>,
    busy: Signal<bool>,
    picking: Signal<bool>,
    folder_error: Signal<String>,
    folder_warning: Signal<String>,
    dialogs: FileDialogs,
    prompt: Signal<Option<Prompt>>,
    receipt: Signal<Option<ReceiptOutcome>>,
    history: History,
    retry: Signal<bool>,
    workers: Workers,
    tasks: TaskSpawner,
}

impl Cable {
    pub(crate) fn new(
        cx: &mut WindowContext,
        workers: Workers,
        history: History,
        folder: Signal<Option<PathBuf>>,
        default_folder: Signal<Option<PathBuf>>,
        folder_warning: Signal<String>,
    ) -> Self {
        let runtime = &cx.ui.runtime;
        let cable = Self {
            enabled: runtime.signal(true),
            status: runtime.signal(String::new()),
            folder,
            default_folder,
            busy: runtime.signal(false),
            picking: runtime.signal(false),
            folder_error: runtime.signal(String::new()),
            folder_warning,
            dialogs: cx.dialogs.clone(),
            prompt: runtime.signal(None),
            receipt: runtime.signal(None),
            history,
            retry: runtime.signal(false),
            workers,
            tasks: cx.tasks.clone(),
        };
        let close = cable.clone();
        cx.on_closed(move || {
            close.enabled.set(false);
            close.workers.cancel();
        });
        cable.start();
        cable
    }

    pub(crate) fn switch_blocked(&self) -> impl Fn() -> bool + use<> {
        let busy = self.busy.clone();
        let picking = self.picking.clone();
        move || busy.get() || picking.get()
    }

    pub(crate) fn switch_mode(&self, enabled: bool) -> bool {
        if self.picking.get() || (!enabled && !self.workers.pause_idle()) {
            return false;
        }
        self.enabled.set(enabled);
        if enabled {
            self.start();
        }
        true
    }

    pub(crate) async fn wait_for_worker(&self) {
        while self.workers.0.borrow().is_some() {
            zgx::timer::sleep(TICK).await;
        }
    }

    fn choose(&self) {
        if !self.enabled.get() || (self.switch_blocked())() || !self.workers.pause_idle() {
            return;
        }
        self.picking.set(true);
        self.folder_error.set(String::new());
        self.status.set("Choosing a save folder…".into());
        let ui = self.clone();
        self.tasks.spawn(async move {
            ui.wait_for_worker().await;
            let current = ui.folder.get().or_else(|| ui.default_folder.get());
            if let Err(message) =
                pick_destination(ui.dialogs.clone(), current, &ui.folder, &ui.folder_warning).await
            {
                ui.folder_error.set(message.into());
            }
            ui.picking.set(false);
            ui.start();
        });
    }

    fn start(&self) {
        if !self.enabled.get() || self.picking.get() || self.workers.0.borrow().is_some() {
            return;
        }
        self.retry.set(false);
        self.status.set("Preparing cable receive…".into());
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let phase = Arc::new(AtomicU8::new(IDLE));
        let worker_phase = phase.clone();
        let folder = self.folder.get();
        let handle = thread::Builder::new()
            .name("sliploom-cable".into())
            .spawn(move || run(&flag, &worker_phase, &tx, folder.as_deref()));
        match handle {
            Ok(handle) => {
                *self.workers.0.borrow_mut() = Some(Worker {
                    cancel: cancel.clone(),
                    phase,
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
                let finished = {
                    let mut slot = ui.workers.0.borrow_mut();
                    match slot.as_ref() {
                        Some(worker) if worker.handle.is_finished() => slot.take(),
                        Some(_) => None,
                        None => return,
                    }
                };
                if let Some(worker) = finished {
                    let failed = worker.handle.join().is_err();
                    ui.prompt.set(None);
                    ui.busy.set(false);
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
        loop {
            match rx.try_recv() {
                Ok(Event::Receipt(receipt)) => {
                    let outcome = if receipt.confirmed {
                        ReceiptOutcome::Saved(receipt.bytes)
                    } else {
                        ReceiptOutcome::SavedWithUncertainConfirmation(receipt.bytes)
                    };
                    self.history.record(receipt.path, receipt.bytes);
                    self.receipt.set(Some(outcome));
                    if !stopped(cancel) {
                        self.status.set(WAITING.into());
                    }
                }
                Ok(_) if stopped(cancel) => {}
                Ok(Event::Status(message)) => {
                    self.prompt.set(None);
                    self.status.set(message);
                }
                Ok(Event::DefaultFolder(path)) => {
                    self.default_folder.set(Some(path));
                }
                Ok(Event::Busy(busy)) => {
                    self.busy.set(busy);
                }
                Ok(Event::Prompt(prompt)) if prompt.deadline > Instant::now() => {
                    self.prompt.set(Some(prompt));
                    self.status.set(String::new());
                }
                Ok(Event::Prompt(_)) => {}
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

    pub(crate) fn heading(&self) -> View {
        let prompt = self.prompt.clone();
        switch(
            move || prompt.with(|value| value.as_ref().map(|offer| offer.is_pdf)),
            |offer, _| match offer {
                Some(true) => headline("Receive PDF?").live(Live::Assertive),
                Some(false) => headline("Receive image?").live(Live::Assertive),
                None => headline("Receive by cable"),
            },
        )
        .display_contents()
    }

    pub(crate) fn has_status_update(&self) -> bool {
        self.retry.get()
            || !self.folder_error.with(String::is_empty)
            || !self.folder_warning.with(String::is_empty)
            || self.receipt.with(Option::is_some)
    }

    pub(crate) fn status_view(&self) -> View {
        let status = self.status.clone();
        let receipt = self.receipt.clone();
        let error = self.folder_error.clone();
        let error_text = error.clone();
        column()
            .gap(8.)
            .child(
                metadata(text_signal(move || {
                    status.with(|message| {
                        if message.is_empty() {
                            "Waiting for approval.".into()
                        } else {
                            message.clone()
                        }
                    })
                }))
                .live(Live::Polite),
            )
            .child_when(
                move || receipt.get(),
                |receipt, _| metadata(text(receipt.message())).live(Live::Polite),
            )
            .child_when(
                move || (!error.with(String::is_empty)).then_some(()),
                move |(), _| {
                    let error = error_text.clone();
                    metadata(text_signal(move || error.get())).live(Live::Polite)
                },
            )
            .child(inline_status(self.folder_warning.clone()))
    }

    pub(crate) fn view(&self) -> View {
        let prompt = self.prompt.clone();
        let ui = self.clone();
        let retry = self.retry.clone();

        switch(
            move || prompt.get(),
            move |prompt, _| {
                if let Some(prompt) = prompt {
                    return ui
                        .prompt_view(prompt)
                        .child(inline_status(ui.folder_warning.clone()));
                }
                let actions = ui.clone();
                let chooser = ui.clone();
                let retry = retry.clone();
                column()
                    .gap(8.)
                    .child(receive_actions(
                        switch(
                            move || retry.get(),
                            move |retry, _| {
                                let ui = actions.clone();
                                if retry {
                                    receive_primary("Retry cable")
                                        .disabled_when(ui.switch_blocked())
                                        .on_click(move || ui.start())
                                } else {
                                    let available = ui.clone();
                                    receive_primary("Open folder")
                                        .disabled_when(move || {
                                            (available.switch_blocked())()
                                                || (available.folder.get().is_none()
                                                    && available.default_folder.get().is_none())
                                        })
                                        .on_click(move || {
                                            let selected = ui.folder.get();
                                            let expected = selected
                                                .clone()
                                                .or_else(|| ui.default_folder.get());
                                            let _ = thread::Builder::new()
                                                .name("sliploom-open-folder".into())
                                                .spawn(move || {
                                                    if let Ok(folder) =
                                                        open_destination(selected.as_deref(), false)
                                                        && expected.as_ref() == Some(&folder.path)
                                                    {
                                                        folder.explore();
                                                    }
                                                });
                                        })
                                }
                            },
                        )
                        .display_contents(),
                        quiet_action(text("Choose save folder"))
                            .disabled_when(ui.switch_blocked())
                            .on_click(move || chooser.choose()),
                    ))
                    .child(destination_label(
                        ui.folder.clone(),
                        ui.default_folder.clone(),
                    ))
                    .child(inline_status(ui.folder_warning.clone()))
            },
        )
        .display_contents()
    }

    fn prompt_view(&self, prompt: Prompt) -> View {
        let decline = self.clone();
        let accept = self.clone();
        let detail = prompt.bytes.map_or_else(
            || "Cable share · Size unknown".into(),
            |bytes| format!("Cable share · {bytes} bytes"),
        );
        let no = prompt.clone();
        column().gap(16.).child(metadata(text(detail))).child(
            column()
                .max_w(304.)
                .items_center()
                .gap(8.)
                .child(primary_action("Accept").on_click(move || accept.decide(&prompt, true)))
                .child(quiet_action(text("Decline")).on_click(move || decline.decide(&no, false))),
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

pub(crate) use private_folder::Folder;

pub(crate) fn open_destination(
    selected: Option<&Path>,
    create_default: bool,
) -> io::Result<Folder> {
    match selected {
        Some(path) => private_folder::chosen(path),
        None => private_folder::open(crate::APP_NAME, create_default),
    }
}

pub(crate) fn open_settings_directory(create: bool) -> io::Result<Folder> {
    private_folder::open(private_folder::SETTINGS_DIRECTORY, create)
}

pub(crate) fn open_trust_directory(
    local: &Path,
    has_legacy_state: impl FnOnce(&Path) -> io::Result<bool>,
) -> io::Result<Folder> {
    private_folder::trust(local, has_legacy_state)
}

fn run(cancel: &AtomicBool, phase: &AtomicU8, tx: &Sender<Event>, selected: Option<&Path>) {
    let Ok(folder) = open_destination(selected, true) else {
        status(
            tx,
            if selected.is_some() {
                "Can’t use this save folder. Choose another folder."
            } else {
                "Can’t use the default save folder. Cable receive is off."
            },
        );
        return;
    };
    if selected.is_none() {
        let _ = tx.send(Event::DefaultFolder(folder.path.clone()));
    }
    let retry_delays = [0, 1, 2, 4, 8, 12, 16];
    let mut failures = 0;
    while let Some(&delay) = retry_delays.get(failures) {
        pause(cancel, Duration::from_secs(delay));
        if stopped(cancel) {
            return;
        }
        let Ok(receiver) = CableReceiver::bind(&folder.path) else {
            status(tx, "Couldn’t start cable receive in the save folder.");
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
            let receiving = scope.spawn(move || receive(receiver, cancel, phase, connection, tx));
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
    phase: &AtomicU8,
    connection: &AtomicBool,
    tx: &Sender<Event>,
) {
    while !stopped(cancel) && !stopped(connection) {
        let mut outcome = "No file was saved. Share again from the phone to retry.";
        let mut next_progress = Instant::now();
        let mut active = false;
        let result = receiver.receive_next(
            cancel,
            connection,
            |offer| {
                if phase
                    .compare_exchange(IDLE, BUSY, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return false;
                }
                active = true;
                let _ = tx.send(Event::Busy(true));
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
        if active {
            phase.store(IDLE, Ordering::Release);
            let _ = tx.send(Event::Busy(false));
        }
        match result {
            Ok(Some(receipt)) => {
                let _ = tx.send(Event::Receipt(receipt));
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
    pub(super) const SETTINGS_DIRECTORY: &str = "Sliploom Settings";

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
        fn ConvertStringSidToSidW(text: *const u16, sid: *mut Ptr) -> i32;
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
        fn IsWellKnownSid(sid: Ptr, kind: u32) -> i32;
        fn GetAce(acl: *mut Acl, index: u32, ace: *mut *mut AceHeader) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Ptr;
        fn LocalFree(memory: Ptr) -> Ptr;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetVolumeInformationW(
            root: *const u16,
            label: *mut u16,
            label_size: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            filesystem: *mut u16,
            filesystem_size: u32,
        ) -> i32;
        fn CreateDirectoryW(path: *const u16, attributes: *const SecurityAttributes) -> i32;
        fn CompareStringOrdinal(
            first: *const u16,
            first_len: i32,
            second: *const u16,
            second_len: i32,
            ignore_case: i32,
        ) -> i32;
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

    pub(crate) struct Folder {
        pub(crate) path: PathBuf,
        _directories: Vec<File>,
    }

    impl AsRef<Path> for Folder {
        fn as_ref(&self) -> &Path {
            &self.path
        }
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

    pub(super) fn open(name: &str, create: bool) -> io::Result<Folder> {
        let base = known_folder(&LOCAL_APP_DATA)?;
        let profile = known_folder(&PROFILE)?;
        if base == profile || !base.starts_with(&profile) {
            return Err(invalid());
        }
        let mut directories = lock_local_data(&base)?;
        let expected = private_descriptor()?;
        let path = base.join(name);
        if create {
            create_private(&path, &expected)?;
        }
        let folder = lock_directory(&path, true)?;
        check_descriptor(&folder, &expected)?;
        directories.push(folder);
        Ok(Folder {
            path,
            _directories: directories,
        })
    }

    pub(super) fn trust(
        local: &Path,
        has_legacy_state: impl FnOnce(&Path) -> io::Result<bool>,
    ) -> io::Result<Folder> {
        let local = PathBuf::from(native(local)?);
        let mut directories = lock_local_data(&local)?;
        let expected = private_descriptor()?;
        let current = local.join(crate::APP_NAME);
        let path = current.join("Trust");
        let current_dir = existing_private(&current, &expected)?;
        if current_dir.is_some()
            && let Some(trust_dir) = existing_private(&path, &expected)?
        {
            directories.extend(current_dir);
            directories.push(trust_dir);
            return Ok(Folder {
                path,
                _directories: directories,
            });
        }

        let legacy = local.join("SEAM");
        let legacy_path = legacy.join("Trust");
        if let Some(legacy_dir) = existing_private(&legacy, &expected)?
            && let Some(trust_dir) = existing_private(&legacy_path, &expected)?
            && has_legacy_state(&legacy_path)?
        {
            directories.extend([legacy_dir, trust_dir]);
            return Ok(Folder {
                path: legacy_path,
                _directories: directories,
            });
        }

        let current_dir = match current_dir {
            Some(directory) => directory,
            None => {
                create_private(&current, &expected)?;
                lock_private(&current, &expected)?
            }
        };
        create_private(&path, &expected)?;
        let trust_dir = lock_private(&path, &expected)?;
        directories.extend([current_dir, trust_dir]);
        Ok(Folder {
            path,
            _directories: directories,
        })
    }

    fn lock_local_data(base: &Path) -> io::Result<Vec<File>> {
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
        base.ancestors()
            .filter(|path| path.parent().is_some())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|path| lock_directory(path, false))
            .collect()
    }

    fn create_private(path: &Path, expected: &Local) -> io::Result<()> {
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
        Ok(())
    }

    fn lock_private(path: &Path, expected: &Local) -> io::Result<File> {
        let folder = lock_directory(path, true)?;
        check_chosen_acl(&folder, expected, true, true, false)?;
        Ok(folder)
    }

    fn existing_private(path: &Path, expected: &Local) -> io::Result<Option<File>> {
        let folder = match lock_directory(path, true) {
            Ok(folder) => folder,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        check_chosen_acl(&folder, expected, true, true, false)?;
        Ok(Some(folder))
    }

    pub(super) fn chosen(input: &Path) -> io::Result<Folder> {
        let input = native(input)?;
        let profile = native(&known_folder(&PROFILE)?)?;
        let profile = normalized(Path::new(&profile))?;
        let root = wide(OsStr::new(&input[..3]));
        let mut flags = 0;
        unsafe {
            if GetDriveTypeW(root.as_ptr()) != 3 {
                return Err(invalid());
            }
            check(GetVolumeInformationW(
                root.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                &mut flags,
                null_mut(),
                0,
            ))?;
        }
        if flags & 8 == 0 {
            return Err(invalid());
        }
        let expected = private_descriptor()?;
        let path = Path::new(&input);
        let mut parents = path.ancestors().collect::<Vec<_>>();
        parents.reverse();
        let mut directories = Vec::with_capacity(parents.len());
        let mut inside_profile = false;
        for parent in parents {
            let handle = lock_directory(parent, true)?;
            let canonical = normalized(parent)?;
            let final_dir = parent == path;
            if final_dir && !inside_profile {
                return Err(invalid());
            }
            inside_profile |= canonical == profile;
            check_chosen_acl(
                &handle,
                &expected,
                final_dir,
                inside_profile,
                parent.parent().is_none(),
            )?;
            directories.push(handle);
            if final_dir {
                let path = PathBuf::from(canonical);
                native(&path)?;
                if is_settings_destination(&path) {
                    return Err(invalid());
                }
                return Ok(Folder {
                    path,
                    _directories: directories,
                });
            }
        }
        Err(invalid())
    }

    fn is_settings_destination(path: &Path) -> bool {
        let Ok(base) = known_folder(&LOCAL_APP_DATA) else {
            return false;
        };
        let settings = normalized(&base)
            .map(PathBuf::from)
            .unwrap_or(base)
            .join(SETTINGS_DIRECTORY);
        let settings = wide(settings.as_os_str());
        path.ancestors().any(|ancestor| {
            let ancestor = wide(ancestor.as_os_str());
            ancestor.len() == settings.len()
                && unsafe {
                    CompareStringOrdinal(ancestor.as_ptr(), -1, settings.as_ptr(), -1, 1) == 2
                }
        })
    }

    fn normalized(path: &Path) -> io::Result<String> {
        let canonical = std::fs::canonicalize(path)?;
        let canonical = canonical
            .to_str()
            .and_then(|s| s.strip_prefix(r"\\?\"))
            .ok_or_else(invalid)?;
        if !canonical.eq_ignore_ascii_case(path.to_str().ok_or_else(invalid)?) {
            return Err(invalid());
        }
        Ok(canonical.into())
    }

    fn native(path: &Path) -> io::Result<String> {
        let path = path.to_str().ok_or_else(invalid)?;
        let bytes = path.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
            return Err(invalid());
        }
        if path[3..].split('\\').any(|part| {
            let stem = part
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_end_matches(' ')
                .to_ascii_uppercase();
            part.is_empty()
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c <= '\u{1f}' || "<>:\"/|?*".contains(c))
                || matches!(
                    stem.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
                )
                || stem
                    .strip_prefix("COM")
                    .or_else(|| stem.strip_prefix("LPT"))
                    .is_some_and(|n| {
                        matches!(
                            n,
                            "0" | "1"
                                | "2"
                                | "3"
                                | "4"
                                | "5"
                                | "6"
                                | "7"
                                | "8"
                                | "9"
                                | "¹"
                                | "²"
                                | "³"
                        )
                    })
        }) {
            return Err(invalid());
        }
        Ok(format!(
            "{}{}",
            (bytes[0] as char).to_ascii_uppercase(),
            &path[1..]
        ))
    }

    fn check_chosen_acl(
        folder: &File,
        expected: &Local,
        final_dir: bool,
        inside_profile: bool,
        drive_root: bool,
    ) -> io::Result<()> {
        unsafe {
            let (mut owner, mut user, mut acl, mut descriptor) =
                (null_mut(), null_mut(), null_mut(), null_mut());
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
            let _descriptor = Local(descriptor);
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
            let mut defaulted = 0;
            check(GetSecurityDescriptorOwner(
                expected.0,
                &mut user,
                &mut defaulted,
            ))?;
            let trusted = |sid: Ptr| {
                !sid.is_null()
                    && (EqualSid(sid, user) != 0
                        || IsWellKnownSid(sid, 22) != 0
                        || IsWellKnownSid(sid, 26) != 0)
            };
            if owner.is_null()
                || user.is_null()
                || acl.is_null()
                || !(trusted(owner) || (drive_root && is_windows_servicing_sid(owner)))
                || (final_dir && EqualSid(owner, user) == 0)
            {
                return Err(invalid());
            }
            const INHERIT_ONLY: u8 = 8;
            const READ_TRAVERSE: u32 = 0xa012_00a9;
            const ADD_SUBDIRECTORY: u32 = 4;
            let public_parent_access =
                READ_TRAVERSE | if inside_profile { 0 } else { ADD_SUBDIRECTORY };
            for index in 0..(*acl).count {
                let mut ace = null_mut();
                check(GetAce(acl, u32::from(index), &mut ace))?;
                match (*ace).kind {
                    1 => {}
                    0 if (*ace).size >= 16 => {
                        let mask = *ace.cast::<u32>().add(1);
                        let sid = ace.cast::<u32>().add(2).cast();
                        if !trusted(sid)
                            && (final_dir
                                || ((*ace).flags & INHERIT_ONLY == 0
                                    && mask & !public_parent_access != 0))
                        {
                            return Err(invalid());
                        }
                    }
                    _ => return Err(invalid()),
                }
            }
            Ok(())
        }
    }

    fn is_windows_servicing_sid(sid: Ptr) -> bool {
        let text = wide(OsStr::new(
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
        ));
        let mut raw = null_mut();
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut raw) } == 0 {
            return false;
        }
        let expected = Local(raw);
        unsafe { EqualSid(sid, expected.0) != 0 }
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
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not a private Sliploom folder",
        )
    }
}
