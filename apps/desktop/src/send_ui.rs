use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Cursor, Read};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
#[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

use image::{ImageFormat, ImageReader, Limits};
use receiver_service::lan::batch::{BatchOutcome, BatchStatus, MAX_BATCH_BYTES, MAX_FILES};
use receiver_service::lan::sender::{SendInvite, SendOutcome};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::scene::Color;
use zgx::semantics::Live;
use zgx_desktop::{FileDialogOptions, FileDialogs, FileFilter, TaskSpawner, WindowContext};

use crate::material::{metadata, quiet_action, receive_primary};
#[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
use crate::mtp;
use crate::{byte_size, file_count, inline_status, trusted_ui};

const TICK: Duration = Duration::from_millis(75);
const QR_FILE_LIMIT: u64 = 16 * 1024 * 1024;
const QR_SIDE_LIMIT: u32 = 4096;

#[derive(Clone, Copy, PartialEq)]
enum Activity {
    Idle,
    Picking,
    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    Checking,
    Transferring,
    Cancelling,
}

enum WifiBlock {
    Files,
    MissingCode,
    InvalidCode,
    OlderCode,
}

enum TransferOutcome {
    Wifi(BatchOutcome),
    Legacy(SendOutcome),
    Trusted(BatchOutcome),
    TrustedUnavailable,
    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    Cable(mtp::BatchCopy),
}

#[derive(Default)]
struct Progress {
    connected: AtomicBool,
    bytes: AtomicU64,
    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    confirmed: AtomicUsize,
}

#[derive(Clone, Default)]
pub(crate) struct Worker {
    cancel: Arc<AtomicBool>,
    thread: Rc<RefCell<Option<thread::JoinHandle<()>>>>,
}

impl Worker {
    fn spawn<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> io::Result<mpsc::Receiver<T>> {
        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        cancel.store(false, Ordering::Release);
        let worker = thread::Builder::new().name(name.into()).spawn(move || {
            let _ = tx.send(work(&cancel));
        })?;
        self.thread.replace(Some(worker));
        Ok(rx)
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    fn join(&self) {
        if let Some(thread) = self.thread.borrow_mut().take() {
            let _ = thread.join();
        }
    }

    pub(crate) fn shutdown(&self) {
        self.cancel();
        self.join();
    }
}

#[derive(Clone)]
pub(crate) struct Card {
    files: Signal<Vec<PathBuf>>,
    code: Signal<String>,
    scanned: Signal<Option<SendInvite>>,
    active_target: Signal<Option<Ipv4Addr>>,
    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    cable_mode: Signal<bool>,
    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    cable_target: Signal<Option<mtp::Target>>,
    trusted: trusted_ui::Panel,
    active_trusted_label: Signal<Option<String>>,
    activity: Signal<Activity>,
    status: Signal<String>,
    worker: Worker,
    dialogs: FileDialogs,
    tasks: TaskSpawner,
}

impl Card {
    pub(crate) fn new(cx: &mut WindowContext, worker: Worker, trusted: trusted_ui::Panel) -> Self {
        let runtime = &cx.ui.runtime;
        let card = Self {
            files: runtime.signal(Vec::new()),
            code: runtime.signal(String::new()),
            scanned: runtime.signal(None),
            active_target: runtime.signal(None),
            #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
            cable_mode: runtime.signal(false),
            #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
            cable_target: runtime.signal(None),
            trusted,
            active_trusted_label: runtime.signal(None),
            activity: runtime.signal(Activity::Idle),
            status: runtime.signal(String::new()),
            worker,
            dialogs: cx.dialogs.clone(),
            tasks: cx.tasks.clone(),
        };
        let closing = card.clone();
        cx.on_closed(move || {
            closing.worker.cancel();
            closing.clear_code();
        });
        card
    }

    pub(crate) fn busy(&self) -> bool {
        self.activity.get() != Activity::Idle
    }

    pub(crate) fn has_status_update(&self) -> bool {
        !self.busy() && !self.status.with(String::is_empty)
    }

    pub(crate) fn status_view(&self) -> View {
        let card = self.clone();
        metadata(text_signal(move || {
            let message = card.status.get();
            if !message.is_empty() {
                return message;
            }
            #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
            if card.over_cable() {
                return if card.cable_target.with(Option::is_some) {
                    "Destination: Android Downloads.".into()
                } else {
                    "Connect and unlock your phone, then select USB File transfer.".into()
                };
            }
            "Open Receive on the receiving device for a one-time code.".into()
        }))
        .live(Live::Polite)
    }

    pub(crate) fn over_cable(&self) -> bool {
        #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
        return self.cable_mode.get();
        #[cfg(not(any(target_os = "windows", all(unix, feature = "unix-mtp"))))]
        false
    }

    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    pub(crate) fn transport_switch(&self) -> View {
        let label = self.clone();
        let busy = self.clone();
        let action = self.clone();
        quiet_action(text_signal(move || {
            if label.over_cable() {
                "Use Wi-Fi instead"
            } else {
                "Use cable instead"
            }
            .into()
        }))
        .disabled_when(move || busy.busy())
        .on_click(move || {
            if action.busy() {
                return;
            }
            let cable = !action.over_cable();
            action.clear_code();
            action.cable_target.set(None);
            action.status.set(String::new());
            action.cable_mode.set(cable);
            if cable {
                action.check_cable();
            }
        })
    }

    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    fn check_cable(&self) {
        if self.busy() || !self.over_cable() {
            return;
        }
        self.activity.set(Activity::Checking);
        self.cable_target.set(None);
        self.status.set("Checking USB File transfer…".into());
        let Ok(rx) = self.worker.spawn("sliploom-check-mtp", mtp::detect) else {
            self.finish("Couldn’t start the USB check.".into());
            return;
        };
        let card = self.clone();
        self.tasks.spawn(async move {
            let result = loop {
                match rx.try_recv() {
                    Ok(result) => break Some(result),
                    Err(TryRecvError::Disconnected) => break None,
                    Err(TryRecvError::Empty) => zgx::timer::sleep(TICK).await,
                }
            };
            if card.worker.cancel.load(Ordering::Acquire) {
                card.finish(String::new());
                return;
            }
            let message = match result {
                Some(Ok(target)) => {
                    card.cable_target.set(Some(target));
                    String::new()
                }
                Some(Err(error)) => error.message().into(),
                None => "Couldn’t check USB File transfer.".into(),
            };
            card.finish(message);
        });
    }

    fn clear_code(&self) {
        self.code.set(String::new());
        self.scanned.mutate(|slot| slot.take().is_some());
    }

    pub(crate) fn reset(&self) {
        if self.busy() {
            return;
        }
        #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
        {
            self.cable_mode.set(false);
            self.cable_target.set(None);
        }
        self.clear_code();
        self.files.set(Vec::new());
        self.status.set(String::new());
    }

    pub(crate) fn can_send_trusted(&self) -> bool {
        !self.busy()
            && !self.over_cable()
            && self
                .files
                .with(|paths| (1..=MAX_FILES).contains(&paths.len()))
    }

    pub(crate) fn send_trusted(&self, id: [u8; 32]) -> bool {
        if !self.can_send_trusted() {
            return false;
        }
        let Some((trust, peer)) = self.trusted.ready_peer(id) else {
            return false;
        };
        let paths = self.files.get();
        self.clear_code();
        self.active_target.set(Some(peer.address));
        self.active_trusted_label
            .set(Some(trusted_ui::device_name(&peer.label, peer.id)));
        self.transfer(false, paths.len(), move |cancel, progress| {
            if cancel.load(Ordering::Acquire) {
                return TransferOutcome::TrustedUnavailable;
            }
            match trust.open_receive(&peer, cancel) {
                Ok(invite) if invite.is_batch() => TransferOutcome::Trusted(invite.send_batch(
                    &paths,
                    cancel,
                    || progress.connected.store(true, Ordering::Release),
                    |bytes| progress.bytes.store(bytes, Ordering::Release),
                )),
                _ => TransferOutcome::TrustedUnavailable,
            }
        });
        self.busy()
    }

    fn destination(&self) -> Option<(Ipv4Addr, bool)> {
        let details = |invite: &SendInvite| (invite.address(), invite.is_batch());
        self.scanned
            .with(|invite| invite.as_ref().map(details))
            .or_else(|| {
                self.code
                    .with(|code| SendInvite::parse(code.trim()).as_ref().map(details))
            })
    }

    fn wifi_readiness(&self) -> Result<Ipv4Addr, WifiBlock> {
        let count = self.files.with(Vec::len);
        if !(1..=MAX_FILES).contains(&count) {
            return Err(WifiBlock::Files);
        }
        match self.destination() {
            Some((address, batch)) if batch || count == 1 => Ok(address),
            Some(_) => Err(WifiBlock::OlderCode),
            None if self.code.with(|code| code.trim().is_empty()) => Err(WifiBlock::MissingCode),
            None => Err(WifiBlock::InvalidCode),
        }
    }

    fn pick_files(&self) {
        if self.busy() {
            return;
        }
        self.activity.set(Activity::Picking);
        let card = self.clone();
        self.tasks.spawn(async move {
            let options = FileDialogOptions::new("Choose files to send");
            match card.dialogs.open_files(options).await {
                Ok(Some(paths)) if paths.len() > MAX_FILES => {
                    card.files.set(Vec::new());
                    card.status
                        .set(format!("Choose up to {MAX_FILES} files at a time."));
                }
                Ok(Some(paths)) if !paths.is_empty() => {
                    card.files.set(paths);
                    card.status.set(String::new());
                }
                Ok(_) => {}
                Err(_) => {
                    card.status.set("Couldn’t open the file picker.".into());
                }
            }
            card.activity.set(Activity::Idle);
        });
    }

    fn pick_qr(&self) {
        if self.busy() || self.over_cable() {
            return;
        }
        self.activity.set(Activity::Picking);
        let card = self.clone();
        self.tasks.spawn(async move {
            let options = FileDialogOptions::new("Open Sliploom QR image")
                .filter(FileFilter::new("PNG or JPEG image", ["png", "jpg", "jpeg"]));
            match card.dialogs.open_file(options).await {
                Ok(Some(path)) => {
                    card.clear_code();
                    card.status.set("Reading QR image…".into());
                    match scan_image(path).await {
                        Ok(invite) => {
                            card.scanned.mutate(|slot| {
                                *slot = Some(invite);
                                true
                            });
                            card.status.set(String::new());
                        }
                        Err(message) => {
                            card.status.set(message.into());
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    card.status.set("Couldn’t open the QR image picker.".into());
                }
            }
            card.activity.set(Activity::Idle);
        });
    }

    fn start(&self, cable: bool) {
        if self.busy() || self.over_cable() != cable {
            return;
        }
        let paths = self.files.get();
        let total = paths.len();
        if total == 0 || total > MAX_FILES {
            return;
        }
        #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
        if cable {
            if let Some(target) = self.cable_target.get() {
                self.transfer(true, total, move |cancel, progress| {
                    TransferOutcome::Cable(mtp::send_batch(
                        &target,
                        &paths,
                        cancel,
                        |confirmed, sent| {
                            progress.bytes.store(sent, Ordering::Release);
                            progress.confirmed.store(confirmed, Ordering::Release);
                        },
                    ))
                });
            }
            return;
        }
        if self.wifi_readiness().is_err() {
            return;
        }
        let mut invite = None;
        self.scanned.mutate(|slot| {
            invite = slot.take();
            invite.is_some()
        });
        let Some(invite) = invite.or_else(|| self.code.with(|code| SendInvite::parse(code.trim())))
        else {
            return;
        };
        self.active_target.set(Some(invite.address()));
        self.clear_code();
        self.transfer(false, total, move |cancel, progress| {
            let connected = || progress.connected.store(true, Ordering::Release);
            let sent = |bytes| progress.bytes.store(bytes, Ordering::Release);
            if invite.is_batch() {
                TransferOutcome::Wifi(invite.send_batch(&paths, cancel, connected, sent))
            } else {
                TransferOutcome::Legacy(invite.send(&paths[0], cancel, connected, sent))
            }
        });
    }

    fn transfer(
        &self,
        cable: bool,
        total: usize,
        work: impl FnOnce(&AtomicBool, &Progress) -> TransferOutcome + Send + 'static,
    ) {
        self.activity.set(Activity::Transferring);
        self.status.set(if cable {
            format!("Preparing {} for USB copy…", file_count(total))
        } else {
            format!("Preparing {} and connecting…", file_count(total))
        });
        let progress = Arc::new(Progress::default());
        let worker_progress = progress.clone();
        let Ok(rx) = self.worker.spawn("sliploom-send", move |cancel| {
            work(cancel, &worker_progress)
        }) else {
            let one_time = !cable && self.active_trusted_label.with(Option::is_none);
            self.fail_transfer(if one_time {
                "Couldn’t start the sender. Get a new one-time code from the receiver."
            } else {
                "Couldn’t start the sender."
            });
            return;
        };
        let card = self.clone();
        self.tasks.spawn(async move {
            loop {
                match rx.try_recv() {
                    Ok(outcome) => {
                        match outcome {
                            TransferOutcome::Wifi(outcome) => card.finish(batch_message(outcome, false)),
                            TransferOutcome::Legacy(outcome) => card.finish(legacy_message(outcome)),
                            TransferOutcome::Trusted(outcome) => card.finish(batch_message(outcome, true)),
                            TransferOutcome::TrustedUnavailable => {
                                let message = if card.worker.cancel.load(Ordering::Acquire) {
                                    "Send cancelled."
                                } else {
                                    "Couldn’t start trusted send. Check that the device is still ready and try again."
                                };
                                card.finish(message.into());
                            }
                            #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
                            TransferOutcome::Cable(outcome) => {
                                let confirmed = outcome.confirmed;
                                match outcome.result {
                                    Ok(()) => card.finish(format!("USB confirmed {confirmed}/{total} copies to Downloads.")),
                                    Err(error) => card.fail_transfer(&format!("USB confirmed {confirmed}/{total} copies. {}", error.message())),
                                }
                            }
                        }
                        return;
                    }
                    Err(TryRecvError::Disconnected) => {
                        #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
                        if cable {
                            let confirmed = progress.confirmed.load(Ordering::Acquire);
                            card.fail_transfer(&format!("USB confirmed {confirmed}/{total} copies. Check Downloads before retrying."));
                            return;
                        }
                        card.fail_transfer("Send ended without a result. Check the receiver before retrying.");
                        return;
                    }
                    Err(TryRecvError::Empty) => {
                        if card.activity.get() == Activity::Transferring {
                            let bytes = progress.bytes.load(Ordering::Acquire);
                            #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
                            if cable {
                                let confirmed = progress.confirmed.load(Ordering::Acquire);
                                if bytes > 0 || confirmed > 0 {
                                    card.update_status(format!("Copying… {confirmed}/{total} confirmed · {} written", byte_size(bytes)));
                                }
                            }
                            if !cable && progress.connected.load(Ordering::Acquire) {
                                let trusted = card.active_trusted_label.with(Option::is_some);
                                card.update_status(if bytes == 0 && !trusted {
                                    format!("Connected · {} · check the receiver", file_count(total))
                                } else {
                                    format!("Sending {} · {} sent", file_count(total), byte_size(bytes))
                                });
                            }
                        }
                        zgx::timer::sleep(TICK).await;
                    }
                }
            }
        });
    }

    fn update_status(&self, message: String) {
        if self.status.with(|status| status != &message) {
            self.status.set(message);
        }
    }

    fn fail_transfer(&self, message: &str) {
        #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
        if self.over_cable() {
            self.cable_target.set(None);
        }
        self.finish(message.into());
    }

    fn stop(&self) {
        if self.activity.get() == Activity::Transferring {
            self.worker.cancel();
            self.activity.set(Activity::Cancelling);
            self.status.set("Cancelling…".into());
        }
    }

    fn finish(&self, message: String) {
        self.worker.join();
        self.active_target.set(None);
        self.active_trusted_label.set(None);
        self.status.set(message);
        self.activity.set(Activity::Idle);
    }

    pub(crate) fn view(&self) -> View {
        let files = self.files.clone();
        let file_mode = self.clone();
        let pick_file = self.clone();
        let transport = self.clone();
        let controls = self.clone();
        column()
            .gap(8.)
            .child(
                row()
                    .gap(8.)
                    .items_center()
                    .child(
                        switch(
                            move || !file_mode.over_cable() && file_mode.files.with(Vec::is_empty),
                            move |primary, _| {
                                let busy = pick_file.clone();
                                let action = pick_file.clone();
                                let button = if primary {
                                    receive_primary("Choose files")
                                } else {
                                    quiet_action(text("Choose files"))
                                };
                                button
                                    .disabled_when(move || busy.busy())
                                    .on_click(move || action.pick_files())
                            },
                        )
                        .display_contents(),
                    )
                    .child(
                        metadata(text_signal(move || {
                            let count = files.with(Vec::len);
                            if count == 0 {
                                "No files selected".into()
                            } else {
                                format!("{} selected", file_count(count))
                            }
                        }))
                        .grow()
                        .min_w(0.)
                        .truncate(),
                    ),
            )
            .child(
                switch(
                    move || {
                        let active = matches!(
                            transport.activity.get(),
                            Activity::Transferring | Activity::Cancelling
                        );
                        (transport.over_cable(), active)
                    },
                    move |(cable, active), _| {
                        #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
                        if cable {
                            return controls.cable_entry(active);
                        }
                        #[cfg(not(any(target_os = "windows", all(unix, feature = "unix-mtp"))))]
                        let _ = cable;
                        if active {
                            return column()
                                .gap(8.)
                                .child(controls.cancel_button())
                                .child(controls.wifi_destination_view());
                        }
                        controls.entry()
                    },
                )
                .display_contents(),
            )
            .child(inline_status(self.status.clone()))
    }

    fn wifi_destination_view(&self) -> View {
        let destination = self.clone();
        switch(
            move || destination.wifi_hint(),
            move |hint, _| match hint {
                Some(hint) => metadata(text(hint)),
                None => div().hidden(),
            },
        )
        .display_contents()
    }

    fn wifi_hint(&self) -> Option<String> {
        if let Some(label) = self.active_trusted_label.get() {
            return Some(format!("Destination: {label}"));
        }
        if let Some(address) = self.active_target.get() {
            return Some(format!("Destination: {address}"));
        }
        let hint = match self.wifi_readiness() {
            Ok(address) => return Some(format!("Ready for {address}")),
            Err(WifiBlock::Files) => return None,
            Err(WifiBlock::OlderCode) => {
                "This older code can send only one image or PDF. Choose one file."
            }
            Err(WifiBlock::InvalidCode) => {
                "Invalid code. Paste the full one-time code from Receive."
            }
            Err(WifiBlock::MissingCode) => {
                if !self.status.with(String::is_empty) {
                    return None;
                }
                if self.trusted.available() {
                    return Some(
                        "Use the full code or QR from Receive, or send from Trusted devices."
                            .into(),
                    );
                }
                "Paste the full code or open the QR image from Receive."
            }
        };
        Some(hint.into())
    }

    fn cancel_button(&self) -> View {
        let cancel = self.clone();
        let activity = self.activity.clone();
        receive_primary("Cancel")
            .disabled_when(move || activity.get() == Activity::Cancelling)
            .on_click(move || cancel.stop())
    }

    #[cfg(any(target_os = "windows", all(unix, feature = "unix-mtp")))]
    fn cable_entry(&self, active: bool) -> View {
        let target = self.cable_target.clone();
        let action = if active {
            self.cancel_button()
        } else {
            let ready = self.clone();
            let send = self.clone();
            let busy = self.clone();
            let check = self.clone();
            row()
                .gap(8.)
                .items_center()
                .child(
                    receive_primary("Send")
                        .disabled_when(move || {
                            ready.busy()
                                || ready.files.with(Vec::is_empty)
                                || !ready.cable_target.with(Option::is_some)
                        })
                        .on_click(move || send.start(true)),
                )
                .child(
                    quiet_action(text("Check again"))
                        .disabled_when(move || busy.busy())
                        .on_click(move || check.check_cable()),
                )
        };
        column()
            .gap(8.)
            .child(
                row()
                    .gap(8.)
                    .items_center()
                    .child(
                        metadata(text_signal(move || {
                            target.with(|device| match device {
                                Some(device) => format!("Device: {}", device.name()),
                                None => "No phone selected".into(),
                            })
                        }))
                        .min_w(0.)
                        .text_wrap(false)
                        .truncate(),
                    )
                    .child(metadata(text("• Android Downloads")).shrink_0()),
            )
            .child(action)
    }

    fn entry(&self) -> View {
        let scanned = self.scanned.clone();
        let has_files = self.files.clone();
        let entry = self.clone();
        let ready = self.clone();
        let send = self.clone();
        column()
            .gap(8.)
            .child(
                switch(
                    move || scanned.with(Option::is_some),
                    move |has_qr, _| {
                        let busy = entry.clone();
                        let action = entry.clone();
                        if has_qr {
                            return row()
                                .gap(8.)
                                .items_center()
                                .child(metadata(text("QR image loaded")).grow())
                                .child(
                                    quiet_action(text("Enter code"))
                                        .disabled_when(move || busy.busy())
                                        .on_click(move || {
                                            if !action.busy() {
                                                action.clear_code();
                                            }
                                        }),
                                );
                        }
                        let read_only = entry.clone();
                        column()
                            .gap(4.)
                            .child(metadata(text("One-time code")))
                            .child(
                                row()
                                    .gap(8.)
                                    .items_center()
                                    .child(
                                        text_input("One-time code", entry.code.clone())
                                            .secure(true)
                                            .autocorrect(false)
                                            .read_only_when(move || read_only.busy())
                                            .grow()
                                            .min_w(0.)
                                            .h(36.)
                                            .rounded(8.)
                                            .bg(Color(0, 0, 0, 48))
                                            .border_color(Color(118, 118, 118, 192))
                                            .focus(|s| s.border_color(Color(176, 176, 176, 255))),
                                    )
                                    .child(
                                        quiet_action(text("Open QR image"))
                                            .disabled_when(move || busy.busy())
                                            .on_click(move || action.pick_qr()),
                                    ),
                            )
                    },
                )
                .display_contents(),
            )
            .child(self.wifi_destination_view())
            .child_when(
                move || (!has_files.with(Vec::is_empty)).then_some(()),
                move |(), _| {
                    let ready = ready.clone();
                    let send = send.clone();
                    receive_primary("Send")
                        .disabled_when(move || ready.busy() || ready.wifi_readiness().is_err())
                        .on_click(move || send.start(false))
                },
            )
    }
}

fn batch_message(outcome: BatchOutcome, trusted: bool) -> String {
    let BatchOutcome {
        total,
        confirmed,
        unconfirmed,
        status,
    } = outcome;
    let counts = format!("Receiver confirmed {confirmed}/{total} files.");
    let detail = match status {
        BatchStatus::Complete => String::new(),
        BatchStatus::Declined => "Receiver declined the batch.".into(),
        BatchStatus::Cancelled => "Send cancelled.".into(),
        BatchStatus::Failed if trusted => {
            "Send ended early. Check the receiver before trying again.".into()
        }
        BatchStatus::Failed => "Send ended early.".into(),
        BatchStatus::TooLarge => format!(
            "Choose up to {MAX_FILES} files totaling no more than {}.",
            byte_size(MAX_BATCH_BYTES)
        ),
        BatchStatus::Unsupported => "Choose readable regular files with safe filenames.".into(),
    };
    let next = if unconfirmed {
        "One file may have been saved without confirmation. Check the receiver before retrying."
    } else if !trusted && matches!(status, BatchStatus::Cancelled | BatchStatus::Failed) {
        match confirmed {
            0 => "Get a new one-time code from the receiver.",
            confirmed if confirmed < total => "Choose only the remaining files to send.",
            _ => "",
        }
    } else {
        ""
    };
    [counts.as_str(), &detail, next]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn legacy_message(outcome: SendOutcome) -> String {
    match outcome {
        SendOutcome::Saved(bytes) => {
            format!("Receiver confirmed it saved the file ({bytes} bytes).")
        }
        SendOutcome::Unconfirmed => {
            "Couldn’t confirm it was saved. Check the receiver before retrying.".into()
        }
        SendOutcome::Cancelled => {
            "Send cancelled. Get a new one-time code from the receiver.".into()
        }
        SendOutcome::Failed => {
            "Couldn’t send. Check Wi-Fi and get a new one-time code from the receiver.".into()
        }
        SendOutcome::TooLarge => "This file is too large to send.".into(),
        SendOutcome::Unsupported => "Choose a supported image or PDF.".into(),
    }
}

async fn scan_image(path: PathBuf) -> Result<SendInvite, &'static str> {
    const FAILED: &str = "Couldn’t read the QR image.";
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("sliploom-read-qr".into())
        .spawn(move || {
            let _ = tx.send(read_invite(&path));
        })
        .map_err(|_| FAILED)?;
    loop {
        match rx.try_recv() {
            Ok(result) => return result,
            Err(TryRecvError::Disconnected) => return Err(FAILED),
            Err(TryRecvError::Empty) => zgx::timer::sleep(TICK).await,
        }
    }
}

fn read_invite(path: &Path) -> Result<SendInvite, &'static str> {
    const INVALID: &str =
        "Choose a readable PNG or JPEG QR image (up to 16 MB and 4096 px per side).";
    let file = File::open(path).map_err(|_| INVALID)?;
    if !file.metadata().map_err(|_| INVALID)?.is_file() {
        return Err(INVALID);
    }
    let mut data = Vec::new();
    file.take(QR_FILE_LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|_| INVALID)?;
    if data.len() as u64 > QR_FILE_LIMIT {
        return Err(INVALID);
    }
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| INVALID)?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(INVALID);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(QR_SIDE_LIMIT);
    limits.max_image_height = Some(QR_SIDE_LIMIT);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| INVALID)?.into_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(image);
    let mut answer = None;
    for grid in prepared.detect_grids() {
        let mut payload = Vec::new();
        let invite = grid
            .decode_to(&mut payload)
            .ok()
            .and_then(|_| std::str::from_utf8(&payload).ok())
            .and_then(SendInvite::parse);
        payload.fill(0);
        if let Some(invite) = invite {
            if answer.is_some() {
                return Err("Multiple codes found. Choose an image with one Sliploom code.");
            }
            answer = Some(invite);
        }
    }
    answer.ok_or("No valid Sliploom code found in this QR image.")
}
