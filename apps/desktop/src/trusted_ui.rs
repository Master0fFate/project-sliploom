use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io;
use std::net::Ipv4Addr;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use qrcode::QrCode;
use receiver_service::lan::Receipt;
use receiver_service::lan::trust::{PairPrompt, Peer, Trust};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FileDispositionInfo,
    SetFileInformationByHandle,
};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::scene::Color;
use zgx_desktop::WindowContext;

use crate::history::History;
use crate::material::{metadata, quiet_action, receive_primary};
use crate::{cable_ui, inline_status, invitation_qr, likely_address, send_ui};

type Id = [u8; 32];
type Cancel = Arc<AtomicBool>;
const TICK: Duration = Duration::from_millis(75);
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);
const PAIR_WINDOW: Duration = Duration::from_secs(120);
const PAIR_FINISH: Duration = Duration::from_secs(2);
const PROBE_INTERVAL: Duration = Duration::from_secs(3);
const LISTENER_RETRY: Duration = Duration::from_secs(10);
const RECEIVE_FLAG: &str = "trusted-receive-opt-in.v1.flag";
const READY_FOR: Duration = Duration::from_secs(12);
const PAIR_RETRY: &str =
    "Pairing didn’t finish. If either device lists the other, forget it and pair again.";

type Hwnd = *mut c_void;
type WinEvent = unsafe extern "system" fn(usize, u32, Hwnd, i32, i32, u32, u32);
const EVENT_SYSTEM_FOREGROUND: u32 = 3;
const WINEVENT_OUTOFCONTEXT: u32 = 0;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetActiveWindow() -> Hwnd;
    fn GetForegroundWindow() -> Hwnd;
    fn GetWindowThreadProcessId(hwnd: Hwnd, process: *mut u32) -> u32;
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: usize,
        callback: WinEvent,
        process: u32,
        thread: u32,
        flags: u32,
    ) -> usize;
    fn UnhookWinEvent(hook: usize) -> i32;
}

#[derive(Clone)]
struct Foreground {
    hwnd: usize,
    thread: u32,
    lost: Cancel,
}

impl Foreground {
    fn is_current(&self) -> bool {
        let mut process = 0;
        let hwnd = self.hwnd as Hwnd;
        !self.lost.load(Ordering::Acquire)
            && self.thread != 0
            && unsafe { GetForegroundWindow() } == hwnd
            && unsafe { GetWindowThreadProcessId(hwnd, &mut process) } == self.thread
            && process == std::process::id()
    }
}

thread_local! {
    static WATCH: RefCell<Option<(usize, Foreground)>> = const { RefCell::new(None) };
}

unsafe extern "system" fn foreground_changed(
    hook: usize,
    _: u32,
    hwnd: Hwnd,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let _ = WATCH.try_with(|watch| {
        if let Ok(active) = watch.try_borrow()
            && let Some((active_hook, key)) = active.as_ref()
            && *active_hook == hook
            && key.hwnd != hwnd as usize
        {
            key.lost.store(true, Ordering::Release);
        }
    });
}

struct ForegroundWatch {
    hook: usize,
    key: Foreground,
}

impl ForegroundWatch {
    fn capture() -> Option<Self> {
        let hwnd = unsafe { GetActiveWindow() };
        if hwnd.is_null() || WATCH.with(|watch| watch.borrow().is_some()) {
            return None;
        }
        let thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
        let key = Foreground {
            hwnd: hwnd as usize,
            thread,
            lost: Cancel::default(),
        };
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                0,
                foreground_changed,
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook == 0 {
            return None;
        }
        WATCH.with(|watch| watch.replace(Some((hook, key.clone()))));
        let watch = Self { hook, key };
        watch.key.is_current().then_some(watch)
    }
}

impl Drop for ForegroundWatch {
    fn drop(&mut self) {
        self.key.lost.store(true, Ordering::Release);
        let _ = WATCH.try_with(|watch| watch.borrow_mut().take());
        unsafe { UnhookWinEvent(self.hook) };
    }
}

type Worker = (Cancel, JoinHandle<()>);

#[derive(Clone, Default)]
pub(crate) struct Workers(Rc<RefCell<Vec<Worker>>>);

impl Workers {
    fn start(&self, name: &str, cancel: Cancel, f: impl FnOnce() + Send + 'static) -> bool {
        self.reap();
        let Ok(worker) = thread::Builder::new().name(name.into()).spawn(f) else {
            return false;
        };
        self.0.borrow_mut().push((cancel, worker));
        true
    }

    fn reap(&self) {
        self.0
            .borrow_mut()
            .retain(|(_, worker)| !worker.is_finished());
    }

    fn cancel(&self) {
        for (cancel, _) in self.0.borrow().iter() {
            cancel.store(true, Ordering::Release);
        }
    }

    pub(crate) fn shutdown(&self) {
        self.cancel();
        for (_, thread) in self.0.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) struct Profile {
    trust: Trust,
    directory: cable_ui::Folder,
}

pub(crate) fn open() -> Option<Profile> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let directory = cable_ui::open_trust_directory(&local, has_legacy_state).ok()?;
    let trust = Trust::open(&directory.path).ok()?;
    Some(Profile { trust, directory })
}

fn has_legacy_state(directory: &Path) -> io::Result<bool> {
    for name in ["identity.dpapi", "peers.dpapi", RECEIVE_FLAG] {
        match fs::symlink_metadata(directory.join(name)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn receive_opt_in(path: &Path, clear: bool) -> io::Result<bool> {
    let file = match OpenOptions::new()
        .access_mode(FILE_GENERIC_READ.0 | DELETE.0)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() != 0
        || metadata.file_attributes()
            & (FILE_ATTRIBUTE_DEVICE.0 | FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)
            != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid trusted receive flag",
        ));
    }
    if clear {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                (&raw const disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }?;
    }
    Ok(true)
}

struct Lease {
    cancel: Cancel,
    foreground: Foreground,
}

impl Lease {
    fn live(&self) -> bool {
        !self.cancel.load(Ordering::Acquire) && self.foreground.is_current()
    }
}

struct Approval {
    lease: Arc<Lease>,
    prompt: PairPrompt,
    until: Instant,
    reply: mpsc::SyncSender<bool>,
}

fn confirm(lease: Arc<Lease>, prompt: PairPrompt, events: &mpsc::Sender<Event>) -> bool {
    if !lease.live() {
        return false;
    }
    let until = Instant::now() + CONFIRM_TIMEOUT;
    let (reply, decision) = mpsc::sync_channel(1);
    if events
        .send(Event::Approval(Approval {
            lease: lease.clone(),
            prompt,
            until,
            reply,
        }))
        .is_err()
    {
        return false;
    }
    while lease.live() {
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match decision.recv_timeout(remaining.min(TICK)) {
            Ok(yes) => return yes && lease.live() && Instant::now() < until,
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    false
}

#[derive(Clone, PartialEq, Eq)]
struct Device {
    id: Id,
    address: Ipv4Addr,
    label: String,
}

impl From<Peer> for Device {
    fn from(peer: Peer) -> Self {
        Self {
            id: peer.id,
            address: peer.address,
            label: display_label(&peer.label),
        }
    }
}

fn fingerprint_hint(id: Id) -> String {
    format!("{:08X}", u32::from_be_bytes([id[0], id[1], id[2], id[3]]))
}

pub(crate) fn device_name(label: &str, id: Id) -> String {
    format!("{} · {}", display_label(label), fingerprint_hint(id))
}

fn display_label(label: &str) -> String {
    let safe: String = label.chars().filter(|ch| {
        !ch.is_control() && !matches!(*ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }).take(48).collect();
    if safe.trim().is_empty() {
        "Device".into()
    } else {
        safe.trim().into()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Devices,
    Paste,
    Qr,
    Waiting,
    Confirm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Receive {
    Off,
    Waiting,
    Allowed,
    Stopping,
    RevokeFailed,
}

struct Attempt {
    lease: Arc<Lease>,
    expires: Instant,
    inbound: bool,
    before: Vec<Id>,
}

enum Event {
    Approval(Approval),
    Paired(Arc<Lease>, bool),
    Forgotten(bool),
    Saved(Receipt),
    ListenerEnded(&'static str),
    Ping(Cancel, Id, Ipv4Addr, Option<Instant>),
    SweepDone(Cancel),
}

struct State {
    trust: Option<Trust>,
    workers: Workers,
    events: mpsc::Sender<Event>,
    incoming: RefCell<Option<mpsc::Receiver<Event>>>,
    open: Signal<bool>,
    page: Signal<Page>,
    status: Signal<String>,
    input: Signal<String>,
    qr: RefCell<Option<QrCode>>,
    receive: Signal<Receive>,
    receive_flag: Option<PathBuf>,
    receive_status: Signal<String>,
    save_folder: Arc<Mutex<Option<PathBuf>>>,
    next_listener: Cell<Instant>,
    devices: Signal<Vec<Device>>,
    ready: Signal<HashMap<Id, (Ipv4Addr, Instant)>>,
    forgetting: Signal<bool>,
    list_scroll: Signal<f32>,
    foreground: RefCell<Option<ForegroundWatch>>,
    attempt: RefCell<Option<Attempt>>,
    request: RefCell<Option<Approval>>,
    armed: Arc<Mutex<Option<Arc<Lease>>>>,
    listener: RefCell<Option<Cancel>>,
    probe: RefCell<Option<Cancel>>,
    next_probe: Cell<Instant>,
}

#[derive(Clone)]
pub(crate) struct Panel(Rc<State>);

impl Panel {
    pub(crate) fn new(
        cx: &mut WindowContext,
        profile: Option<&Profile>,
        workers: Workers,
        folder: Signal<Option<PathBuf>>,
    ) -> Self {
        let rt = &cx.ui.runtime;
        let save_folder = Arc::new(Mutex::new(folder.get()));
        let mirror = save_folder.clone();
        let folder_effect = rt.effect(move || {
            let selected = folder.get();
            *mirror.lock().expect("save folder lock poisoned") = selected;
        });
        let (events, rx) = mpsc::channel();
        let trust = profile.map(|profile| profile.trust.clone());
        let receive_flag = profile.map(|profile| profile.directory.path.join(RECEIVE_FLAG));
        let (receive, receive_status) = match receive_flag
            .as_deref()
            .map(|path| receive_opt_in(path, false))
        {
            Some(Ok(true)) => (Receive::Waiting, ""),
            Some(Err(_)) => (
                Receive::Off,
                "Can’t read the trusted receive setting. Trusted receive is off.",
            ),
            _ => (Receive::Off, ""),
        };
        let panel = Self(Rc::new(State {
            trust,
            workers,
            events,
            incoming: RefCell::new(Some(rx)),
            open: rt.signal(false),
            page: rt.signal(Page::Devices),
            status: rt.signal(String::new()),
            input: rt.signal(String::new()),
            qr: RefCell::default(),
            receive: rt.signal(receive),
            receive_flag,
            receive_status: rt.signal(receive_status.into()),
            save_folder,
            next_listener: Cell::new(Instant::now()),
            devices: rt.signal(Vec::new()),
            ready: rt.signal(HashMap::new()),
            forgetting: rt.signal(false),
            list_scroll: rt.signal(0.),
            foreground: RefCell::default(),
            attempt: RefCell::default(),
            request: RefCell::default(),
            armed: Arc::default(),
            listener: RefCell::default(),
            probe: RefCell::default(),
            next_probe: Cell::new(Instant::now()),
        }));
        let closed = panel.clone();
        let watch = panel.clone();
        let dismissal = rt.effect(move || {
            if !watch.0.open.get() {
                watch.dismiss();
            }
        });
        cx.on_closed(move || {
            closed.0.workers.cancel();
            closed.dismiss();
            drop(dismissal);
            drop(folder_effect);
        });
        panel.0.refresh_devices();
        panel
    }

    pub(crate) fn watch(
        &self,
        cx: &WindowContext,
        history: History,
        sending: Signal<bool>,
        send: send_ui::Card,
    ) {
        let Some(rx) = self.0.incoming.borrow_mut().take() else {
            return;
        };
        let panel = self.clone();
        cx.tasks.spawn(async move {
            loop {
                let now = Instant::now();
                if panel.0.open.get() && !panel.has_foreground() {
                    panel.close();
                }
                while let Ok(event) = rx.try_recv() {
                    panel.on_event(event, &history);
                }
                panel.check_pair(now);
                if panel.0.receive.get() == Receive::Waiting && now >= panel.0.next_listener.get() {
                    panel.start_listener();
                }
                panel.heartbeat(
                    now,
                    panel.0.open.get() || sending.get() && !send.over_cable(),
                );
                panel.0.workers.reap();
                let active = panel.0.open.get()
                    || panel.0.listener.borrow().is_some()
                    || sending.get() && !send.over_cable();
                zgx::timer::sleep(if active {
                    TICK
                } else {
                    Duration::from_millis(200)
                })
                .await;
            }
        });
    }

    pub(crate) fn available(&self) -> bool {
        self.0.trust.is_some()
    }

    fn ready_address(&self, id: Id) -> Option<Ipv4Addr> {
        let (address, until) = self.0.ready.with(|ready| ready.get(&id).copied())?;
        (!self.0.forgetting.get()
            && Instant::now() < until
            && self.0.devices.with(|devices| {
                devices
                    .iter()
                    .any(|device| device.id == id && device.address == address)
            }))
        .then_some(address)
    }

    pub(crate) fn is_ready(&self, id: Id) -> bool {
        self.ready_address(id).is_some()
    }

    pub(crate) fn ready_peer(&self, id: Id) -> Option<(Trust, Peer)> {
        let trust = self.0.trust.as_ref()?;
        let addr = self.ready_address(id)?;
        let peer = trust
            .peers()
            .into_iter()
            .find(|peer| peer.id == id && peer.address == addr)?;
        Some((trust.clone(), peer))
    }

    fn has_foreground(&self) -> bool {
        self.0
            .foreground
            .borrow()
            .as_ref()
            .is_some_and(|watch| watch.key.is_current())
    }

    fn show(&self) {
        let Some(watch) = ForegroundWatch::capture() else {
            return;
        };
        self.0.foreground.replace(Some(watch));
        self.0.refresh_devices();
        self.0.open.set(true);
    }

    fn close(&self) {
        self.dismiss();
        self.0.open.set(false);
    }

    fn dismiss(&self) {
        self.0.foreground.borrow_mut().take();
        if self.0.attempt.borrow().is_some() {
            self.0.status.set(PAIR_RETRY.into());
        }
        self.end_pair(true);
    }

    fn end_pair(&self, cancel: bool) {
        let state = &self.0;
        if let Some(attempt) = state.attempt.borrow_mut().take() {
            if cancel {
                attempt.lease.cancel.store(true, Ordering::Release);
            }
            if attempt.inbound {
                if let Ok(mut armed) = state.armed.lock() {
                    armed.take();
                }
                if cancel && let Some(trust) = &state.trust {
                    trust.cancel_pair();
                }
            }
        }
        state.request.borrow_mut().take();
        state.input.set(String::new());
        state.qr.borrow_mut().take();
        state.page.set(Page::Devices);
    }

    fn allow_receive(&self) {
        let state = &self.0;
        if !self.has_foreground() || state.receive.get() != Receive::Off {
            return;
        }
        let saved = state.receive_flag.as_ref().is_some_and(|path| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .is_ok()
        });
        if !saved {
            state
                .receive_status
                .set("Can’t save the trusted receive setting. Trusted receive is off.".into());
            return;
        }
        state.receive.set(Receive::Waiting);
        self.start_listener();
    }

    fn start_listener(&self) {
        let state = &self.0;
        let Some(trust) = state.trust.clone() else {
            return;
        };
        state.next_listener.set(Instant::now() + LISTENER_RETRY);
        let Some(address) = likely_address() else {
            state
                .receive_status
                .set("Trusted receive is waiting for a private Wi-Fi network.".into());
            return;
        };
        let events = state.events.clone();
        let armed = state.armed.clone();
        let save_folder = state.save_folder.clone();
        let cancel = Cancel::default();
        let stop = cancel.clone();
        if !state.workers.start("sliploom-trusted-receive", cancel.clone(), move || {
            let _ = trust.serve(
                address,
                || {
                    let selected = save_folder.lock().expect("save folder lock poisoned").clone();
                    cable_ui::open_destination(selected.as_deref(), true)
                },
                &stop,
                |prompt| {
                    let lease = armed.lock().ok().and_then(|mut armed| armed.take());
                    !stop.load(Ordering::Acquire)
                        && lease.is_some_and(|lease| confirm(lease, prompt, &events))
                },
                |receipt| {
                    let _ = events.send(Event::Saved(receipt));
                },
            );
            let _ = events.send(Event::ListenerEnded(
                "Couldn’t listen for trusted receive. Check Wi-Fi and whether the port is in use. Retrying…",
            ));
        }) {
            state.receive_status.set("Couldn’t start trusted receive. Retrying…".into());
            return;
        }
        state.listener.replace(Some(cancel));
        state.receive.set(Receive::Allowed);
        state.receive_status.set(String::new());
    }

    fn stop_listener(&self) {
        let state = &self.0;
        let running = if let Some(cancel) = state.listener.borrow().as_ref() {
            cancel.store(true, Ordering::Release);
            true
        } else {
            false
        };
        self.end_pair(true);
        let cleared = state
            .receive_flag
            .as_ref()
            .is_some_and(|path| receive_opt_in(path, true).is_ok());
        if cleared {
            state.receive.set(if running {
                Receive::Stopping
            } else {
                Receive::Off
            });
            state.receive_status.set("Trusted receive is off.".into());
        } else {
            state.receive.set(Receive::RevokeFailed);
            state.receive_status.set("Trusted receive is stopped for this session, but the saved permission couldn’t be cleared. Try again before restarting Sliploom.".into());
        }
    }

    fn begin_pair(&self, inbound: bool) -> Option<Arc<Lease>> {
        let state = &self.0;
        let foreground = state.foreground.borrow().as_ref()?.key.clone();
        if !foreground.is_current() {
            return None;
        }
        if !state.open.get() || state.attempt.borrow().is_some() || state.forgetting.get() {
            return None;
        }
        let trust = state.trust.as_ref()?;
        let lease = Arc::new(Lease {
            cancel: Cancel::default(),
            foreground,
        });
        state.attempt.replace(Some(Attempt {
            lease: lease.clone(),
            expires: Instant::now() + PAIR_WINDOW,
            inbound,
            before: trust.peers().into_iter().map(|peer| peer.id).collect(),
        }));
        state.status.set(String::new());
        Some(lease)
    }

    fn show_qr(&self) {
        let state = &self.0;
        if state.receive.get() != Receive::Allowed {
            return;
        }
        let Some(trust) = state.trust.as_ref() else {
            return;
        };
        let Some(lease) = self.begin_pair(true) else {
            return;
        };
        if let Ok(mut armed) = state.armed.lock() {
            *armed = Some(lease);
        }
        match trust
            .arm_pair()
            .ok()
            .and_then(|code| QrCode::new(code.as_bytes()).ok())
        {
            Some(qr) => {
                state.qr.replace(Some(qr));
                state.page.set(Page::Qr);
            }
            None => {
                self.end_pair(true);
                state
                    .status
                    .set("Couldn’t show a pairing QR. Check trusted receive and try again.".into());
            }
        }
    }

    fn pair_from_code(&self) {
        let state = &self.0;
        if state.page.get() != Page::Paste {
            return;
        }
        let Some(trust) = state.trust.clone() else {
            return;
        };
        let code = state.input.get();
        let code = code.trim();
        if !code.starts_with("SEAM5:") || code.len() > 1024 {
            state
                .status
                .set("Paste a SEAM5 pairing code from the phone.".into());
            return;
        }
        let Some(lease) = self.begin_pair(false) else {
            return;
        };
        let code = code.to_owned();
        state.input.set(String::new());
        let events = state.events.clone();
        if state
            .workers
            .start("sliploom-trusted-pair", lease.cancel.clone(), move || {
                let paired = lease.live()
                    && trust
                        .pair(&code, &lease.cancel, |prompt| {
                            confirm(lease.clone(), prompt, &events)
                        })
                        .is_ok();
                let _ = events.send(Event::Paired(lease, paired));
            })
        {
            state.page.set(Page::Waiting);
        } else {
            self.end_pair(true);
            state.status.set("Couldn’t start pairing.".into());
        }
    }

    fn answer(&self, lease: &Arc<Lease>, yes: bool) {
        let state = &self.0;
        if !state
            .request
            .borrow()
            .as_ref()
            .is_some_and(|request| Arc::ptr_eq(&request.lease, lease))
        {
            return;
        }
        let Some(request) = state.request.borrow_mut().take() else {
            return;
        };
        if !yes || !state.open.get() || !request.lease.live() || Instant::now() >= request.until {
            let _ = request.reply.try_send(false);
            self.end_pair(true);
            state
                .status
                .set(if yes { PAIR_RETRY } else { "Pairing declined." }.into());
            return;
        }
        if let Some(attempt) = state.attempt.borrow_mut().as_mut() {
            attempt.expires = request.until + PAIR_FINISH;
        }
        if request.reply.try_send(true).is_ok() {
            state.page.set(Page::Waiting);
        } else {
            self.end_pair(true);
            state.status.set(PAIR_RETRY.into());
        }
    }

    fn check_pair(&self, now: Instant) {
        let state = &self.0;
        let attempts = state.attempt.borrow();
        let Some(attempt) = attempts.as_ref() else {
            return;
        };
        let settled = attempt.inbound
            && state.page.get() == Page::Waiting
            && state.trust.as_ref().is_some_and(|trust| {
                trust
                    .peers()
                    .iter()
                    .any(|peer| !attempt.before.contains(&peer.id))
            });
        let expired = !attempt.lease.live() || now >= attempt.expires;
        drop(attempts);
        if settled || expired {
            self.end_pair(!settled);
            if settled {
                state.refresh_devices();
            }
            state.status.set(
                if settled {
                    "Device saved on this PC. If it isn’t listed on the phone, forget it here and pair again."
                } else {
                    PAIR_RETRY
                }
                .into(),
            );
        }
    }

    fn forget(&self, id: Id) {
        let state = &self.0;
        if !self.has_foreground() || state.forgetting.get() {
            return;
        }
        let Some(trust) = state.trust.clone() else {
            return;
        };
        let events = state.events.clone();
        let cancel = Cancel::default();
        state.forgetting.set(true);
        if !state
            .workers
            .start("sliploom-trusted-forget", cancel.clone(), move || {
                if !cancel.load(Ordering::Acquire) {
                    let _ = events.send(Event::Forgotten(trust.forget(id).is_ok()));
                }
            })
        {
            state.forgetting.set(false);
            state.status.set("Couldn’t forget this device.".into());
        }
    }

    fn on_event(&self, event: Event, history: &History) {
        let state = &self.0;
        match event {
            Event::Approval(request) => {
                let current = state
                    .attempt
                    .borrow()
                    .as_ref()
                    .is_some_and(|attempt| Arc::ptr_eq(&attempt.lease, &request.lease));
                if !current
                    || !state.open.get()
                    || !request.lease.live()
                    || Instant::now() >= request.until
                {
                    return;
                }
                if let Some(attempt) = state.attempt.borrow_mut().as_mut() {
                    attempt.expires = request.until;
                }
                state.request.replace(Some(request));
                state.qr.borrow_mut().take();
                state.page.set(Page::Confirm);
            }
            Event::Paired(lease, paired) => {
                if state
                    .attempt
                    .borrow()
                    .as_ref()
                    .is_some_and(|attempt| Arc::ptr_eq(&attempt.lease, &lease))
                {
                    self.end_pair(!paired);
                    state.refresh_devices();
                    state
                        .status
                        .set(if paired { "Device paired." } else { PAIR_RETRY }.into());
                }
            }
            Event::Forgotten(ok) => {
                state.forgetting.set(false);
                state.refresh_devices();
                state.status.set(
                    if ok {
                        "Device forgotten."
                    } else {
                        "Couldn’t forget this device."
                    }
                    .into(),
                );
            }
            Event::Saved(receipt) => history.record(receipt.path, receipt.bytes),
            Event::ListenerEnded(message) => {
                state.listener.borrow_mut().take();
                match state.receive.get() {
                    Receive::Allowed => {
                        state.next_listener.set(Instant::now() + LISTENER_RETRY);
                        state.receive.set(Receive::Waiting);
                        state.receive_status.set(message.into());
                    }
                    Receive::Stopping => {
                        state.receive.set(Receive::Off);
                    }
                    _ => {}
                }
                if state
                    .attempt
                    .borrow()
                    .as_ref()
                    .is_some_and(|attempt| attempt.inbound)
                {
                    self.end_pair(true);
                }
            }
            Event::Ping(sweep, id, addr, until) if state.is_probe(&sweep) => {
                state.ready.mutate(|ready| match until {
                    Some(until) => {
                        ready.insert(id, (addr, until));
                        true
                    }
                    None => ready.remove(&id).is_some(),
                });
            }
            Event::SweepDone(sweep) if state.is_probe(&sweep) => {
                state.probe.borrow_mut().take();
                state.next_probe.set(Instant::now() + PROBE_INTERVAL);
            }
            _ => {}
        }
    }

    fn heartbeat(&self, now: Instant, watching: bool) {
        let state = &self.0;
        if !watching {
            if let Some(probe) = state.probe.borrow_mut().take() {
                probe.store(true, Ordering::Release);
            }
            state
                .ready
                .mutate(|ready| !std::mem::take(ready).is_empty());
            state.next_probe.set(now);
            return;
        }
        state.ready.mutate(|ready| {
            let before = ready.len();
            ready.retain(|_, (_, until)| *until > now);
            ready.len() != before
        });
        if state.probe.borrow().is_some() || now < state.next_probe.get() {
            return;
        }
        state.refresh_devices();
        state.next_probe.set(now + PROBE_INTERVAL);
        let Some(trust) = state.trust.clone() else {
            return;
        };
        let peers = trust.peers();
        if peers.is_empty() {
            return;
        }
        let events = state.events.clone();
        let cancel = Cancel::default();
        let sweep = cancel.clone();
        if state
            .workers
            .start("sliploom-trusted-ping", cancel.clone(), move || {
                thread::scope(|scope| {
                    for peer in peers {
                        if sweep.load(Ordering::Acquire) {
                            break;
                        }
                        let (trust, sweep, events) = (&trust, &sweep, &events);
                        let _ = thread::Builder::new().spawn_scoped(scope, move || {
                            let until =
                                trust.ping(&peer, sweep).then(|| Instant::now() + READY_FOR);
                            let _ = events.send(Event::Ping(
                                sweep.clone(),
                                peer.id,
                                peer.address,
                                until,
                            ));
                        });
                    }
                });
                let _ = events.send(Event::SweepDone(sweep));
            })
        {
            state.probe.replace(Some(cancel));
        }
    }

    pub(crate) fn view(&self, sending: Signal<bool>, send: send_ui::Card) -> View {
        let action = self.clone();
        let trigger = quiet_action(text("Trusted devices"))
            .shrink_0()
            .on_click(move || {
                if action.0.open.get() {
                    action.close();
                } else {
                    action.show();
                }
            });
        let panel = self.clone();
        let page = self.0.page.clone();
        let heading_page = self.0.page.clone();
        let close = self.clone();
        let status = self.0.status.clone();
        let dialog = modal("Trusted devices", self.0.open.clone())
            .w(376.)
            .px(18.)
            .py(12.)
            .gap(8.)
            .rounded(12.)
            .bg(Color(35, 35, 35, 255))
            .border(1.)
            .border_color(Color(250, 250, 250, 32))
            .child(
                row()
                    .w_full()
                    .shrink_0()
                    .justify_between()
                    .items_center()
                    .child(metadata(
                        text_signal(move || {
                            if heading_page.get() == Page::Qr {
                                "Pair device".into()
                            } else {
                                "Trusted devices".into()
                            }
                        })
                        .heading(2),
                    ))
                    .child(quiet_action(text("Close")).on_click(move || close.close())),
            )
            .child(
                zgx::compose::switch_keeping_focus(
                    move || page.get(),
                    move |page, _| {
                        let body = column()
                            .w_full()
                            .gap(8.)
                            .child(panel.page(page, &sending, &send));
                        if page == Page::Qr {
                            body
                        } else {
                            body.child(inline_status(status.clone()))
                        }
                    },
                )
                .contents(),
            );
        row().shrink_0().child(trigger).child(dialog)
    }

    fn page(&self, page: Page, sending: &Signal<bool>, send: &send_ui::Card) -> View {
        let panel = self.clone();
        let cancel = || {
            let action = panel.clone();
            quiet_action(text("Cancel pairing")).on_click(move || {
                let attempted = action.0.attempt.borrow().is_some();
                action.end_pair(true);
                action.0.status.set(if attempted {
                    PAIR_RETRY.into()
                } else {
                    String::new()
                });
            })
        };
        match page {
            Page::Devices => self.devices_view(sending.clone(), send.clone()),
            Page::Paste => {
                let pair = self.clone();
                column()
                    .gap(8.)
                    .child(metadata(text(
                        "Paste a SEAM5 pairing code shown by Sliploom on your phone.",
                    )))
                    .child(
                        text_input("Phone pairing code", self.0.input.clone())
                            .secure(true)
                            .autocorrect(false)
                            .placeholder("SEAM5 pairing code")
                            .w_full()
                            .h(36.)
                            .rounded(8.)
                            .bg(Color(0, 0, 0, 48)),
                    )
                    .child(
                        row()
                            .gap(8.)
                            .child(receive_primary("Pair").on_click(move || pair.pair_from_code()))
                            .child(cancel()),
                    )
            }
            Page::Qr => {
                let qr = self
                    .0
                    .qr
                    .borrow()
                    .clone()
                    .map(|code| invitation_qr(code, Some(250)).accessible_label("Pairing QR code"))
                    .unwrap_or_else(|| div().hidden());
                let back = self.clone();
                column()
                    .w_full()
                    .gap(8.)
                    .items_center()
                    .child(metadata(text("Scan in Trusted devices on your phone.")))
                    .child(qr.shrink_0())
                    .child(
                        quiet_action(text("Back"))
                            .on_mount(|button| {
                                button.focus();
                            })
                            .on_click(move || back.end_pair(true)),
                    )
            }
            Page::Waiting => column()
                .items_start()
                .gap(8.)
                .child(metadata(text("Waiting for the phone…")))
                .child(cancel()),
            Page::Confirm => {
                let yes = self.clone();
                let no = self.clone();
                let request = self.0.request.borrow();
                let Some(request) = request.as_ref() else {
                    return div().hidden();
                };
                let approve = request.lease.clone();
                let reject = request.lease.clone();
                column().gap(8.)
                    .child(metadata(text(format!("Pair with {}?", display_label(&request.prompt.label))).heading(3)))
                    .child(metadata(text("Does Sliploom on your phone show the same confirmation code?")))
                    .child(text(request.prompt.code.clone()).heading(3))
                    .child(metadata(text("When trusted receive is on, this device can send to this PC without another prompt.")))
                    .child(row().gap(8.)
                        .child(receive_primary("Yes, trust device").on_click(move || yes.answer(&approve, true)))
                        .child(quiet_action(text("No")).on_click(move || no.answer(&reject, false))))
            }
        }
    }

    fn devices_view(&self, sending: Signal<bool>, send: send_ui::Card) -> View {
        if !self.available() {
            return metadata(text(
                "Can’t open trusted device storage. Receive once still works.",
            ));
        }
        let devices = self.0.devices.clone();
        let panel = self.clone();
        let receive = self.0.receive.clone();
        let toggle = self.clone();
        let toggle_ready = self.0.receive.clone();
        let qr = self.clone();
        let qr_ready = self.clone();
        let paste = self.clone();
        let paste_ready = self.0.forgetting.clone();
        column().gap(8.)
            .child(switch(move || devices.get(), move |devices, _| {
                if devices.is_empty() {
                    return metadata(text("No trusted devices yet."));
                }
                scroll(panel.0.list_scroll.clone()).max_h(124.).child(column().gap(4.).children(devices.into_iter().map(|device| {
                    let id = device.id;
                    let ready = panel.clone();
                    let forget = panel.clone();
                    let busy = panel.0.forgetting.clone();
                    let sending = sending.clone();
                    let visible_send = send.clone();
                    let send = send.clone();
                    let choose = panel.clone();
                    let suffix = fingerprint_hint(id);
                    row().gap(4.).items_center()
                        .child(column().grow().min_w(0.)
                            .child(metadata(text(device.label)).truncate())
                            .child(metadata(text_signal(move || {
                                let online = ready.is_ready(id);
                                format!("{} · {suffix}", if online { "Ready" } else { "Not ready" })
                            }))))
                        .child({
                            let still_sending = sending.clone();
                            switch(move || sending.get() && !visible_send.over_cable(), move |send_mode, _| {
                                if !send_mode { return div().hidden(); }
                                let action = send.clone();
                                let blocked = send.clone();
                                let ready = choose.clone();
                                let panel = choose.clone();
                                let still_sending = still_sending.clone();
                                quiet_action(text("Send"))
                                    .disabled_when(move || !blocked.can_send_trusted() || !ready.is_ready(id))
                                    .on_click(move || {
                                        if !still_sending.get() || !panel.0.open.get() || panel.0.page.get() != Page::Devices
                                            || !panel.has_foreground() {
                                            return;
                                        }
                                        if action.send_trusted(id) {
                                            panel.close();
                                        } else {
                                            panel.0.status.set("Couldn’t start the send. Check the files and device, then try again.".into());
                                        }
                                    })
                            }).display_contents()
                        })
                        .child(quiet_action(text("Forget")).disabled_when(move || busy.get()).on_click(move || forget.forget(id)))
                })))
            }).display_contents())
            .child(metadata(text("Allow trusted devices to save to the same folder as Receive once and cable receive without asking while Sliploom is open. This permission stays on until you stop trusted receive.")))
            .child(row().gap(4.).child(quiet_action(text_signal(move || match receive.get() {
                Receive::Off => "Allow trusted receive",
                Receive::Waiting | Receive::Allowed => "Stop trusted receive",
                Receive::Stopping => "Stopping…",
                Receive::RevokeFailed => "Retry clearing permission",
            }.into())).disabled_when(move || toggle_ready.get() == Receive::Stopping).on_click(move || match toggle.0.receive.get() {
                Receive::Off => toggle.allow_receive(),
                Receive::Waiting | Receive::Allowed | Receive::RevokeFailed => toggle.stop_listener(),
                Receive::Stopping => {}
            })))
            .child(inline_status(self.0.receive_status.clone()))
            .child(row().gap(4.)
                .child(quiet_action(text("Show pairing QR"))
                    .disabled_when(move || qr_ready.0.receive.get() != Receive::Allowed || qr_ready.0.forgetting.get())
                    .on_click(move || qr.show_qr()))
                .child(quiet_action(text("Paste phone code")).disabled_when(move || paste_ready.get()).on_click(move || {
                    paste.0.status.set(String::new());
                    paste.0.page.set(Page::Paste);
                })))
    }
}

impl State {
    fn refresh_devices(&self) {
        let Some(trust) = self.trust.as_ref() else {
            return;
        };
        let devices: Vec<_> = trust.peers().into_iter().map(Device::from).collect();
        self.ready.mutate(|ready| {
            let before = ready.len();
            ready.retain(|id, (address, _)| {
                devices
                    .iter()
                    .any(|device| device.id == *id && device.address == *address)
            });
            ready.len() != before
        });
        if self.devices.with(|current| *current != devices) {
            self.devices.set(devices);
        }
    }

    fn is_probe(&self, job: &Cancel) -> bool {
        self.probe
            .borrow()
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, job))
    }
}
