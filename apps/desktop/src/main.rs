#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::cell::RefCell;
use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

use qrcode::{Color as QrColor, QrCode};
use receiver_service::lan::sender::SendInvite;
use receiver_service::lan::{Receipt, ReceiveSession};
use zgx::canvas::{Canvas, Path};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::scene::{Color, Rect};
use zgx::semantics::Live;
use zgx_desktop::{Application, FileDialogOptions, Titlebar, WindowContext, WindowOptions};

use history::History;
use material::{
    backdrop, headline, metadata, primary_action, quiet_action, receive_actions, receive_primary,
};

mod acrylic;
#[cfg(target_os = "windows")]
mod adb;
#[cfg(target_os = "windows")]
mod cable_ui;
mod dashboard;
mod history;
mod material;
#[cfg(target_os = "windows")]
mod mtp;
mod send_ui;
mod settings;
mod titlebar;
#[cfg(target_os = "windows")]
mod trusted_ui;

const APP_NAME: &str = "Sliploom";
const WIFI_TICK: Duration = Duration::from_millis(75);

#[derive(Clone, Copy, PartialEq)]
enum Activity {
    Idle,
    PickingFolder,
    Receiving,
    Cancelling,
}

#[derive(Clone)]
struct Screen {
    folder: Signal<Option<PathBuf>>,
    default_folder: Signal<Option<PathBuf>>,
    activity: Signal<Activity>,
    status: Signal<String>,
    folder_warning: Signal<String>,
    invitation: Signal<String>,
    manual_code: Signal<bool>,
    history: History,
    worker: WifiWorker,
}

#[derive(Clone, Default)]
struct WifiWorker {
    cancel: Arc<AtomicBool>,
    thread: Rc<RefCell<Option<thread::JoinHandle<()>>>>,
}

impl WifiWorker {
    fn shutdown(&self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.thread.borrow_mut().take() {
            let _ = thread.join();
        }
    }
}

enum Event {
    Invitation(String),
    BatchStarted(usize),
    Saved(Receipt),
    Finished(&'static str),
}

fn receive_wifi(
    folder: PathBuf,
    #[cfg(target_os = "windows")] selected: Option<PathBuf>,
    tx: &mpsc::Sender<Event>,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> &'static str {
    #[cfg(target_os = "windows")]
    let _guard = match cable_ui::open_destination(selected.as_deref(), false) {
        Ok(guard) if guard.path == folder => guard,
        _ => return "Can’t use this save folder. Choose another folder.",
    };
    #[cfg(unix)]
    let folder = match settings::validate(folder) {
        Ok(folder) => folder,
        Err(_) => return "Can’t use this save folder. Choose another folder.",
    };
    let Some(address) = likely_address() else {
        return "No local network found. Connect to Wi-Fi and try again.";
    };
    let Ok(session) = ReceiveSession::bind_batch(address, &folder) else {
        return "Couldn’t start. Check your Wi-Fi and save folder.";
    };
    if cancel.load(Ordering::Acquire) {
        return "Receive cancelled.";
    }
    let invitation = session.invitation();
    if !invitation.starts_with("SEAM4:")
        || !SendInvite::parse(invitation).is_some_and(|i| i.is_batch())
    {
        return "Couldn’t create a valid batch code.";
    }
    if tx.send(Event::Invitation(invitation.to_owned())).is_err() {
        return "Receive cancelled.";
    }
    let result = session.receive_batch(
        cancel,
        |offer| {
            !cancel.load(Ordering::Acquire) && tx.send(Event::BatchStarted(offer.count)).is_ok()
        },
        |receipt| {
            let _ = tx.send(Event::Saved(receipt));
        },
        |bytes| progress.store(bytes, Ordering::Release),
    );
    match result {
        Ok(()) => "",
        _ if cancel.load(Ordering::Acquire) => "Receive cancelled.",
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::TimedOut) =>
        {
            "Receive timed out. Start again to retry."
        }
        _ => "Receive did not complete. Start again to retry.",
    }
}

impl Screen {
    fn has_save_folder(&self) -> bool {
        self.folder.with(Option::is_some) || self.default_folder.with(Option::is_some)
    }

    fn start(&self, tasks: &zgx_desktop::TaskSpawner) {
        if self.activity.get() != Activity::Idle {
            return;
        }
        let folder = self.folder.get();
        #[cfg(target_os = "windows")]
        let selected = folder.clone();
        let Some(folder) = folder.or_else(|| self.default_folder.get()) else {
            self.status.set("Choose a save folder first.".into());
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.worker.cancel.store(false, Ordering::Release);
        let cancel = self.worker.cancel.clone();
        let progress = Arc::new(AtomicU64::new(0));
        let worker_progress = progress.clone();
        let worker = thread::Builder::new()
            .name("sliploom-receive".into())
            .spawn(move || {
                let message = receive_wifi(
                    folder,
                    #[cfg(target_os = "windows")]
                    selected,
                    &tx,
                    &cancel,
                    &worker_progress,
                );
                let _ = tx.send(Event::Finished(message));
            });
        let Ok(worker) = worker else {
            self.status.set("Couldn’t start the receiver.".into());
            return;
        };
        self.worker.thread.replace(Some(worker));

        self.activity.set(Activity::Receiving);
        self.manual_code.set(false);
        self.status.set("Starting…".into());
        let screen = self.clone();
        tasks.spawn(async move {
            let mut total = 0;
            let mut saved = 0;
            let mut unconfirmed = 0;
            let mut last_progress = None;
            let message = loop {
                match rx.try_recv() {
                    Ok(Event::Invitation(invitation))
                        if screen.activity.get() == Activity::Receiving =>
                    {
                        screen.invitation.set(invitation);
                        screen.status.set(String::new());
                    }
                    Ok(Event::Invitation(_)) => {}
                    Ok(Event::BatchStarted(count)) => {
                        total = count;
                        screen.invitation.set(String::new());
                        screen.manual_code.set(false);
                        if screen.activity.get() == Activity::Receiving {
                            screen.status.set(format!("Receiving… 0/{total} saved"));
                        }
                    }
                    Ok(Event::Saved(receipt)) => {
                        saved += 1;
                        unconfirmed += usize::from(!receipt.confirmed);
                        screen.history.record(receipt.path, receipt.bytes);
                    }
                    Ok(Event::Finished(message)) => break message,
                    Err(TryRecvError::Disconnected) => break "The receiver stopped unexpectedly.",
                    Err(TryRecvError::Empty) => {
                        if screen.activity.get() == Activity::Receiving && total > 0 {
                            let bytes = progress.load(Ordering::Acquire);
                            if (bytes > 0 || saved > 0) && last_progress != Some((saved, bytes)) {
                                last_progress = Some((saved, bytes));
                                screen.status.set(format!(
                                    "Receiving… {saved}/{total} saved · {}",
                                    byte_size(bytes)
                                ));
                            }
                        }
                        zgx::timer::sleep(WIFI_TICK).await;
                    }
                }
            };
            let mut status = if saved == 0 {
                if message.is_empty() {
                    "Receive did not complete. Start again to retry.".into()
                } else {
                    message.to_owned()
                }
            } else if message.is_empty() && saved == total {
                format!("Saved {}.", file_count(saved))
            } else {
                format!("Saved {saved}/{total} files. {message}")
                    .trim_end()
                    .to_owned()
            };
            if unconfirmed > 0 {
                status.push_str(&format!(
                    " The sender may not have confirmation for {}.",
                    file_count(unconfirmed)
                ));
            }
            screen.finish(status).await;
        });
    }

    fn stop(&self) {
        if self.activity.get() == Activity::Receiving {
            self.worker.cancel.store(true, Ordering::Release);
            self.activity.set(Activity::Cancelling);
            self.invitation.set(String::new());
            self.status.set("Cancelling…".into());
        }
    }

    async fn finish(&self, message: String) {
        while self
            .worker
            .thread
            .borrow()
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            zgx::timer::sleep(Duration::from_millis(25)).await;
        }
        if let Some(worker) = self.worker.thread.borrow_mut().take() {
            let _ = worker.join();
        }
        self.invitation.set(String::new());
        self.manual_code.set(false);
        self.status.set(message);
        self.activity.set(Activity::Idle);
    }
}

pub(crate) fn file_count(count: usize) -> String {
    let unit = if count == 1 { "file" } else { "files" };
    format!("{count} {unit}")
}

pub(crate) fn byte_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.)
    } else {
        format!("{bytes} B")
    }
}

fn likely_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if address.is_private() => Some(address),
        _ => None,
    }
}

fn invitation_qr(code: QrCode, max_side: Option<usize>) -> View {
    const QUIET_ZONE: usize = 4;
    let width = code.width();
    let padded_width = width + 2 * QUIET_ZONE;
    let module = max_side.map_or(4, |side| (side / padded_width).max(1)) as f32;
    let size = padded_width as f32 * module;
    canvas(move |_| {
        let mut drawing = Canvas::new();
        drawing.fill(
            Path::rectangle(Rect::new(0., 0., size, size)),
            Color(255, 255, 255, 255),
        );
        for y in 0..width {
            for x in 0..width {
                if code[(x, y)] == QrColor::Dark {
                    drawing.fill(
                        Path::rectangle(Rect::new(
                            (x + QUIET_ZONE) as f32 * module,
                            (y + QUIET_ZONE) as f32 * module,
                            module,
                            module,
                        )),
                        Color(0, 0, 0, 255),
                    );
                }
            }
        }
        drawing
    })
    .size(size, size)
}

async fn pick_destination(
    dialogs: zgx_desktop::FileDialogs,
    current: Option<PathBuf>,
    folder: &Signal<Option<PathBuf>>,
    warning: &Signal<String>,
) -> Result<bool, &'static str> {
    let mut options = FileDialogOptions::new("Choose save folder");
    if let Some(folder) = current {
        options = options.directory(folder);
    }
    let Some(path) = dialogs
        .open_folder(options)
        .await
        .map_err(|_| "Couldn’t open the folder picker.")?
    else {
        return Ok(false);
    };
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("sliploom-folder-check".into())
        .spawn(move || {
            let _ = tx.send(settings::validate(path).map(|path| {
                let remembered = settings::save(&path).is_ok();
                (path, remembered)
            }));
        })
        .map_err(|_| "Couldn’t check the save folder.")?;
    loop {
        match rx.try_recv() {
            Ok(Ok((path, remembered))) => {
                folder.set(Some(path));
                let message = if remembered {
                    ""
                } else {
                    settings::NOT_REMEMBERED
                };
                warning.set(message.into());
                return Ok(true);
            }
            Ok(Err(_)) => {
                #[cfg(target_os = "windows")]
                return Err(
                    "Choose a private folder inside your Windows user profile on a fixed drive, outside Sliploom Settings.",
                );
                #[cfg(unix)]
                return Err("Choose an existing folder outside Sliploom’s settings folder.");
            }
            Err(TryRecvError::Disconnected) => return Err("Couldn’t check the save folder."),
            Err(TryRecvError::Empty) => zgx::timer::sleep(Duration::from_millis(25)).await,
        }
    }
}

#[cfg(target_os = "windows")]
fn destination_label(folder: Signal<Option<PathBuf>>, default: Signal<Option<PathBuf>>) -> View {
    metadata(text_signal(move || {
        folder.get().or_else(|| default.get()).map_or_else(
            || "No folder selected".into(),
            |path| path.display().to_string(),
        )
    }))
    .truncate()
}

fn wifi_view(
    screen: Screen,
    dialogs: zgx_desktop::FileDialogs,
    tasks: zgx_desktop::TaskSpawner,
) -> View {
    let picking = screen.clone();
    let picker_tasks = tasks.clone();
    let pick_folder = move || {
        if picking.activity.get() != Activity::Idle {
            return;
        }
        picking.activity.set(Activity::PickingFolder);
        let dialogs = dialogs.clone();
        let picking = picking.clone();
        picker_tasks.spawn(async move {
            let current = picking
                .folder
                .get()
                .or_else(|| picking.default_folder.get());
            match pick_destination(dialogs, current, &picking.folder, &picking.folder_warning).await
            {
                Ok(true) => {
                    picking.status.set(String::new());
                }
                Ok(false) => {}
                Err(error) => {
                    picking.status.set(error.into());
                }
            }
            picking.activity.set(Activity::Idle);
        });
    };

    let readiness = screen.clone();
    let actions = screen.clone();
    let action_row = switch(
        move || {
            let active = matches!(
                readiness.activity.get(),
                Activity::Receiving | Activity::Cancelling
            );
            (active, active || readiness.has_save_folder())
        },
        move |(active, show_receive_controls), _| {
            let screen = actions.clone();
            let activity = screen.activity.clone();
            let controls_disabled = move || activity.get() != Activity::Idle;
            if !show_receive_controls {
                return row().items_center().child(
                    primary_action("Choose save folder")
                        .w(184.)
                        .disabled_when(controls_disabled)
                        .on_click(pick_folder.clone()),
                );
            }
            let primary = if active {
                let activity = screen.activity.clone();
                receive_primary("Cancel")
                    .disabled_when(move || activity.get() == Activity::Cancelling)
                    .on_click(move || screen.stop())
            } else {
                let tasks = tasks.clone();
                receive_primary("Receive once")
                    .disabled_when(controls_disabled.clone())
                    .on_click(move || screen.start(&tasks))
            };
            receive_actions(
                primary,
                quiet_action(text("Choose save folder"))
                    .disabled_when(controls_disabled)
                    .on_click(pick_folder.clone()),
            )
        },
    )
    .display_contents();
    let destination = screen.clone();
    let folder_label = metadata(text_signal(move || {
        if destination.activity.get() == Activity::PickingFolder {
            return "Choosing save folder…".into();
        }
        destination
            .folder
            .get()
            .or_else(|| destination.default_folder.get())
            .map_or_else(
                || "Save folder required".into(),
                |path| path.display().to_string(),
            )
    }))
    .text_wrap(false)
    .truncate();
    let code = wifi_code(screen.invitation.clone());
    let code_controls = wifi_code_controls(screen.invitation.clone(), screen.manual_code.clone());
    let controls = column()
        .grow()
        .min_w(0.)
        .max_w(336.)
        .gap(12.)
        .child(folder_label)
        .child(code_controls);
    column()
        .gap(8.)
        .child(action_row)
        .child(row().gap(16.).items_start().child(controls).child(code))
        .child(inline_status(screen.status))
        .child(inline_status(screen.folder_warning))
}

pub(crate) fn inline_status(status: Signal<String>) -> View {
    let visible = status.clone();
    switch(
        move || !visible.with(String::is_empty),
        move |present, _| {
            if present {
                let status = status.clone();
                metadata(text_signal(move || status.get())).live(Live::Polite)
            } else {
                div().hidden()
            }
        },
    )
    .display_contents()
}

fn wifi_code(invitation: Signal<String>) -> View {
    let visible = invitation.clone();
    switch(
        move || !visible.with(String::is_empty),
        move |visible, _| {
            if !visible {
                return div().hidden();
            }
            match invitation.with(|value| QrCode::new(value.as_bytes())) {
                Ok(code) => invitation_qr(code, None).shrink_0(),
                Err(_) => metadata(text("QR unavailable. Use the manual code.")).w(184.),
            }
        },
    )
    .display_contents()
}

fn wifi_code_controls(invitation: Signal<String>, manual: Signal<bool>) -> View {
    let visible = invitation.clone();
    switch(
        move || !visible.with(String::is_empty),
        move |visible, _| {
            if !visible {
                return div().hidden();
            }
            let toggle = manual.clone();
            let expanded = manual.clone();
            let opened = manual.clone();
            let invitation = invitation.clone();
            column()
                .items_start()
                .gap(8.)
                .child(metadata(text("Scan with Sliploom").heading(2)))
                .child(metadata(text("Anyone with this code can send one batch.")))
                .child(
                    quiet_action(text_signal(move || {
                        if expanded.get() {
                            "Hide code"
                        } else {
                            "Manual code"
                        }
                        .into()
                    }))
                    .on_click(move || {
                        toggle.set(!toggle.get());
                    }),
                )
                .child_when(
                    move || opened.get().then_some(()),
                    move |(), _| metadata(text(invitation.get())).w_full().selectable(true),
                )
        },
    )
    .display_contents()
}

fn status_control(
    open: Signal<bool>,
    sending: Signal<bool>,
    send: send_ui::Card,
    wifi: Signal<bool>,
    screen: Screen,
    tasks: zgx_desktop::TaskSpawner,
    #[cfg(target_os = "windows")] cable: cable_ui::Cable,
) -> (View, View) {
    let mut hide = None;
    let visible = open.clone();
    let badge_sending = sending.clone();
    let badge_send = send.clone();
    #[cfg(target_os = "windows")]
    let badge_mode = wifi.clone();
    let badge_screen = screen.clone();
    #[cfg(target_os = "windows")]
    let badge_cable = cable.clone();
    let trigger = quiet_action(text_signal(move || {
        let update = match badge_sending.get() {
            true => badge_send.has_status_update(),
            #[cfg(target_os = "windows")]
            false if !badge_mode.get() => badge_cable.has_status_update(),
            false => {
                !badge_screen.folder_warning.with(String::is_empty)
                    || (badge_screen.activity.get() == Activity::Idle
                        && !badge_screen.status.with(String::is_empty))
            }
        };
        if update { "View status" } else { "Status" }.into()
    }))
    .w(100.)
    .on_click(move || {
        visible.set(true);
        let visible = visible.clone();
        hide.replace(tasks.spawn_scoped(async move {
            zgx::timer::sleep(Duration::from_secs(5)).await;
            visible.set(false);
        }));
    });
    let toast = switch(
        move || open.get(),
        move |visible, _| {
            if !visible {
                return div().hidden();
            }
            let sending = sending.clone();
            let send = send.clone();
            let wifi = wifi.clone();
            let screen = screen.clone();
            #[cfg(target_os = "windows")]
            let cable = cable.clone();
            column()
                .absolute()
                .right(16.)
                .bottom(16.)
                .w(288.)
                .p(12.)
                .rounded(12.)
                .bg(Color(35, 35, 35, 255))
                .border(1.)
                .border_color(Color(250, 250, 250, 32))
                .child(
                    switch(
                        move || (sending.get(), wifi.get()),
                        move |mode, _| match mode {
                            (true, _) => send.status_view(),
                            #[cfg(target_os = "windows")]
                            (false, false) => cable.status_view(),
                            (false, _) => {
                                let screen = screen.clone();
                                let warning = screen.folder_warning.clone();
                                column()
                                    .gap(8.)
                                    .child(
                                        metadata(text_signal(move || {
                                            if !screen.invitation.with(String::is_empty) {
                                                "QR ready. Scan it with Sliploom.".into()
                                            } else {
                                                screen.status.with(|message| {
                                                    if !message.is_empty() {
                                                        message.clone()
                                                    } else if screen.has_save_folder() {
                                                        "Choose Receive once to start.".into()
                                                    } else {
                                                        "Choose a save folder first.".into()
                                                    }
                                                })
                                            }
                                        }))
                                        .live(Live::Polite),
                                    )
                                    .child(inline_status(warning))
                            }
                        },
                    )
                    .display_contents(),
                )
        },
    )
    .display_contents();
    (trigger, toast)
}

fn receive_screen(
    cx: &mut WindowContext,
    wifi_worker: WifiWorker,
    send_worker: send_ui::Worker,
    saved_folder: &io::Result<Option<PathBuf>>,
    #[cfg(target_os = "windows")] workers: cable_ui::Workers,
    #[cfg(target_os = "windows")] trust_profile: Option<&trusted_ui::Profile>,
    #[cfg(target_os = "windows")] trust_workers: trusted_ui::Workers,
) {
    cx.window.set_theme(Some(zgx::theme::Appearance::Dark));
    let glass = acrylic::available(cx);
    #[cfg(target_os = "windows")]
    let trusted = trusted_ui::Panel::new(cx, trust_profile, trust_workers);
    let send = send_ui::Card::new(
        cx,
        send_worker,
        #[cfg(target_os = "windows")]
        trusted.clone(),
    );
    let runtime = &cx.ui.runtime;
    let wifi = runtime.signal(!cfg!(target_os = "windows"));
    let sending = runtime.signal(false);
    let history = History::new(cx);
    #[cfg(target_os = "windows")]
    trusted.watch(cx, history.clone(), sending.clone(), send.clone());
    #[cfg(target_os = "windows")]
    let trusted_trigger = trusted.view(sending.clone(), send.clone());
    let screen = Screen {
        folder: runtime.signal(saved_folder.as_ref().ok().cloned().flatten()),
        default_folder: runtime.signal(None),
        activity: runtime.signal(Activity::Idle),
        status: runtime.signal(String::new()),
        folder_warning: runtime.signal(if saved_folder.is_err() {
            settings::UNAVAILABLE.into()
        } else {
            String::new()
        }),
        invitation: runtime.signal(String::new()),
        manual_code: runtime.signal(false),
        history: history.clone(),
        worker: wifi_worker,
    };
    let status_open = runtime.signal(false);
    let cancel = screen.worker.cancel.clone();
    cx.on_closed(move || cancel.store(true, Ordering::Release));
    #[cfg(target_os = "windows")]
    let cable = cable_ui::Cable::new(
        cx,
        workers,
        history.clone(),
        screen.folder.clone(),
        screen.default_folder.clone(),
        screen.folder_warning.clone(),
    );
    let (status, toast) = status_control(
        status_open.clone(),
        sending.clone(),
        send.clone(),
        wifi.clone(),
        screen.clone(),
        cx.tasks.clone(),
        #[cfg(target_os = "windows")]
        cable.clone(),
    );
    let title_mode = wifi.clone();
    let title_sending = sending.clone();
    let title_send = send.clone();
    let body_mode = wifi.clone();
    let body_sending = sending.clone();
    let send_screen = send.clone();
    let dialogs = cx.dialogs.clone();
    let tasks = cx.tasks.clone();
    let wifi_screen = screen.clone();
    #[cfg(target_os = "windows")]
    let cable_screen = cable.clone();
    #[cfg(target_os = "windows")]
    let cable_heading = cable.clone();
    let heading = switch(
        move || {
            (
                title_sending.get(),
                title_mode.get(),
                title_send.over_cable(),
            )
        },
        move |mode, _| match mode {
            #[cfg(target_os = "windows")]
            (true, _, true) => headline("Send over cable"),
            (true, _, _) => headline("Send over Wi-Fi"),
            #[cfg(target_os = "windows")]
            (false, false, _) => cable_heading.heading(),
            (false, _, _) => headline("Receive over Wi-Fi"),
        },
    )
    .display_contents();
    let content = switch(
        move || (body_sending.get(), body_mode.get()),
        move |mode, _| match mode {
            (true, _) => send_screen.view(),
            #[cfg(target_os = "windows")]
            (false, false) => cable_screen.view(),
            (false, _) => wifi_view(wifi_screen.clone(), dialogs.clone(), tasks.clone()),
        },
    )
    .display_contents();
    let mode_activity = screen.activity.clone();
    #[cfg(target_os = "windows")]
    let cable_blocked = cable.switch_blocked();
    #[cfg(target_os = "windows")]
    let transition = cx.ui.runtime.signal(false);
    #[cfg(target_os = "windows")]
    let changing = transition.clone();
    #[cfg(target_os = "windows")]
    let mode_tasks = cx.tasks.clone();
    let receive_blocked = Rc::new(move || {
        if mode_activity.get() != Activity::Idle {
            return true;
        }
        #[cfg(target_os = "windows")]
        return changing.get() || cable_blocked();
        #[cfg(not(target_os = "windows"))]
        false
    });
    #[cfg(target_os = "windows")]
    let transport = {
        let transport_mode = sending.clone();
        let transport_sending = sending.clone();
        let transport_send = send.clone();
        let transport_wifi = wifi.clone();
        let transport_blocked = receive_blocked.clone();
        let transport_cable = cable.clone();
        let transport_transition = transition.clone();
        let transport_tasks = mode_tasks.clone();
        switch(
            move || !transport_mode.get(),
            move |receiving, _| {
                if !receiving {
                    return transport_send.transport_switch();
                }
                let label_mode = transport_wifi.clone();
                let mode = transport_wifi.clone();
                let blocked = transport_blocked.clone();
                let click_blocked = transport_blocked.clone();
                let sending = transport_sending.clone();
                let (cable, transition, tasks) = (
                    transport_cable.clone(),
                    transport_transition.clone(),
                    transport_tasks.clone(),
                );
                quiet_action(text_signal(move || {
                    if label_mode.get() {
                        "Use cable instead"
                    } else {
                        "Use Wi-Fi instead"
                    }
                    .into()
                }))
                .disabled_when(move || blocked())
                .on_click(move || {
                    let wifi_active = mode.get();
                    if sending.get() || click_blocked() {
                        return;
                    }
                    if !cable.switch_mode(wifi_active) {
                        return;
                    }
                    if !wifi_active {
                        transition.set(true);
                        let (mode, transition, cable) =
                            (mode.clone(), transition.clone(), cable.clone());
                        tasks.spawn(async move {
                            cable.wait_for_worker().await;
                            mode.set(true);
                            transition.set(false);
                        });
                        return;
                    }
                    mode.set(!wifi_active);
                })
            },
        )
        .display_contents()
    };
    let toggle_label = sending.clone();
    let toggle_send = send.clone();
    let toggle_blocked = receive_blocked.clone();
    let mode_switch = quiet_action(text_signal(move || {
        if toggle_label.get() {
            "Receive"
        } else {
            "Send"
        }
        .into()
    }))
    .disabled_when(move || toggle_send.busy() || toggle_blocked())
    .on_click(move || {
        if send.busy() || receive_blocked() {
            return;
        }
        if sending.get() {
            #[cfg(target_os = "windows")]
            if !wifi.get() && !cable.switch_mode(true) {
                return;
            }
            send.reset();
            status_open.set(false);
            sending.set(false);
            return;
        }
        #[cfg(target_os = "windows")]
        if !wifi.get() {
            if !cable.switch_mode(false) {
                return;
            }
            transition.set(true);
            let (sending, transition, cable, status_open) = (
                sending.clone(),
                transition.clone(),
                cable.clone(),
                status_open.clone(),
            );
            mode_tasks.spawn(async move {
                cable.wait_for_worker().await;
                status_open.set(false);
                sending.set(true);
                transition.set(false);
            });
            return;
        }
        status_open.set(false);
        sending.set(true);
    });
    let mode_controls = row().items_center().gap(4.);
    #[cfg(target_os = "windows")]
    let mode_controls = mode_controls.child(transport);
    let themes = cx.ui.theme().clone();
    cx.render(
        backdrop(glass, themes.clone())
            .child(titlebar::view(
                cx,
                #[cfg(target_os = "windows")]
                trusted_trigger,
                status,
            ))
            .child(dashboard::view(
                themes,
                heading,
                content,
                history.view(),
                mode_controls.child(mode_switch),
            ))
            .child(toast),
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    let workers = cable_ui::Workers::default();
    #[cfg(target_os = "windows")]
    let trust_profile = trusted_ui::open();
    #[cfg(target_os = "windows")]
    let trust_workers = trusted_ui::Workers::default();
    let wifi_worker = WifiWorker::default();
    let send_worker = send_ui::Worker::default();
    let saved_folder = settings::load();
    let result = Application::new()
        .font(material::roboto()?)
        .window(WindowOptions {
            title: APP_NAME.into(),
            width: 860.,
            height: 400.,
            min_size: Some((824., 400.)),
            titlebar: Titlebar::Hidden,
            transparent: acrylic::transparent_window(),
            ..Default::default()
        })
        .run(|cx| {
            receive_screen(
                cx,
                wifi_worker.clone(),
                send_worker.clone(),
                &saved_folder,
                #[cfg(target_os = "windows")]
                workers.clone(),
                #[cfg(target_os = "windows")]
                trust_profile.as_ref(),
                #[cfg(target_os = "windows")]
                trust_workers.clone(),
            )
        });
    wifi_worker.shutdown();
    send_worker.shutdown();
    #[cfg(target_os = "windows")]
    workers.shutdown();
    #[cfg(target_os = "windows")]
    trust_workers.shutdown();
    result
}
