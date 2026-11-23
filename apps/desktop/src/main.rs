#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

use qrcode::{Color as QrColor, QrCode};
use receiver_service::lan::ReceiveSession;
use zgx::canvas::{Canvas, Path};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::scene::{Color, Rect};
use zgx::semantics::Live;
use zgx::theme::tok;
use zgx_desktop::{Application, FileDialogOptions, WindowContext, WindowOptions};

#[derive(Clone, Copy, PartialEq)]
enum Activity {
    Idle,
    PickingFolder,
    Receiving,
    Cancelling,
}

#[derive(Clone)]
struct Screen {
    address: Signal<String>,
    folder: Signal<Option<PathBuf>>,
    activity: Signal<Activity>,
    status: Signal<String>,
    invitation: Signal<String>,
    cancel: Arc<AtomicBool>,
}

enum Event {
    Invitation(String),
    Incoming,
    Finished(String),
}

impl Screen {
    fn start(&self, tasks: &zgx_desktop::TaskSpawner) {
        if self.activity.get() != Activity::Idle {
            return;
        }
        let Some(folder) = self.folder.get() else {
            self.status.set("Choose a save folder first.".into());
            return;
        };
        let Ok(address) = self.address.get().trim().parse::<Ipv4Addr>() else {
            self.status
                .set("Enter this computer’s private IPv4 address.".into());
            return;
        };
        if !address.is_private() {
            self.status
                .set("Use a private IPv4 address from your local network.".into());
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.cancel.store(false, Ordering::Release);
        let cancel = self.cancel.clone();
        let worker = thread::Builder::new()
            .name("seam-receive".into())
            .spawn(move || {
                let message = match ReceiveSession::bind(address, &folder) {
                    Ok(session) if !cancel.load(Ordering::Acquire) => {
                        if tx
                            .send(Event::Invitation(session.invitation().to_owned()))
                            .is_err()
                        {
                            return;
                        }
                        match session.receive_with_start(
                            &cancel,
                            || {
                                let _ = tx.send(Event::Incoming);
                            },
                            |_| {},
                        ) {
                            Ok(receipt) => {
                                let status = if receipt.confirmed {
                                    "Saved"
                                } else {
                                    "Saved here; phone may not have received confirmation"
                                };
                                format!(
                                    "{status} ({} bytes)\n{}",
                                    receipt.bytes,
                                    receipt.path.display()
                                )
                            }
                            Err(_) if cancel.load(Ordering::Acquire) => "Cancelled.".into(),
                            Err(error)
                                if error.downcast_ref::<io::Error>().is_some_and(|error| {
                                    error.kind() == io::ErrorKind::TimedOut
                                }) =>
                            {
                                "Receive timed out. Start again to retry.".into()
                            }
                            Err(_) => {
                                "Receive ended without a saved file. Start again to retry.".into()
                            }
                        }
                    }
                    _ if cancel.load(Ordering::Acquire) => "Cancelled.".into(),
                    Err(_) => "Couldn’t start. Check the IPv4 address and save folder.".into(),
                    Ok(_) => "Cancelled.".into(),
                };
                let _ = tx.send(Event::Finished(message));
            });
        if worker.is_err() {
            self.status.set("Couldn’t start the receiver.".into());
            return;
        }

        self.activity.set(Activity::Receiving);
        self.status.set("Starting…".into());
        let screen = self.clone();
        tasks.spawn(async move {
            loop {
                match rx.try_recv() {
                    Ok(Event::Invitation(invitation))
                        if screen.activity.get() == Activity::Receiving =>
                    {
                        screen.invitation.set(invitation);
                        screen
                            .status
                            .set("Share this one-time invitation only with your sender.".into());
                    }
                    Ok(Event::Incoming) if screen.activity.get() == Activity::Receiving => {
                        screen.invitation.set(String::new());
                        screen.status.set("Receiving…".into());
                    }
                    Ok(Event::Invitation(_) | Event::Incoming) => {}
                    Ok(Event::Finished(message)) => {
                        screen.finish(message);
                        return;
                    }
                    Err(TryRecvError::Disconnected) => {
                        screen.finish(
                            "The receiver stopped unexpectedly. Check your save folder.".into(),
                        );
                        return;
                    }
                    Err(TryRecvError::Empty) => zgx::timer::sleep(Duration::from_millis(75)).await,
                }
            }
        });
    }

    fn finish(&self, message: String) {
        self.invitation.set(String::new());
        self.status.set(message);
        self.activity.set(Activity::Idle);
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

fn invitation_qr(code: QrCode) -> View {
    const QUIET_ZONE: usize = 4;
    const MODULE: f32 = 6.;
    let width = code.width();
    let size = (width + 2 * QUIET_ZONE) as f32 * MODULE;
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
                            (x + QUIET_ZONE) as f32 * MODULE,
                            (y + QUIET_ZONE) as f32 * MODULE,
                            MODULE,
                            MODULE,
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

fn receive_screen(cx: &mut WindowContext) {
    let runtime = &cx.ui.runtime;
    let screen = Screen {
        address: runtime.signal(likely_address().map_or_else(String::new, |ip| ip.to_string())),
        folder: runtime.signal(None),
        activity: runtime.signal(Activity::Idle),
        status: runtime.signal(String::new()),
        invitation: runtime.signal(String::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let cancel = screen.cancel.clone();
    cx.on_closed(move || cancel.store(true, Ordering::Release));

    let dialogs = cx.dialogs.clone();
    let tasks = cx.tasks.clone();
    let picking = screen.clone();
    let pick_folder = move || {
        if picking.activity.get() != Activity::Idle {
            return;
        }
        picking.activity.set(Activity::PickingFolder);
        let dialogs = dialogs.clone();
        let picking = picking.clone();
        tasks.spawn(async move {
            let mut options = FileDialogOptions::new("Choose save folder");
            if let Some(folder) = picking.folder.get() {
                options = options.directory(folder);
            }
            match dialogs.open_folder(options).await {
                Ok(Some(folder)) => {
                    picking.folder.set(Some(folder));
                    picking.status.set(String::new());
                }
                Ok(None) => {}
                Err(_) => {
                    picking
                        .status
                        .set("Couldn’t open the folder picker.".into());
                }
            }
            picking.activity.set(Activity::Idle);
        });
    };

    let activity = screen.activity.clone();
    let controls_disabled = move || activity.get() != Activity::Idle;
    let activity = screen.activity.clone();
    let actions = screen.clone();
    let tasks = cx.tasks.clone();
    let status = screen.status.clone();
    let folder = screen.folder.clone();
    let invitation = screen.invitation.clone();
    let invitation_visible = invitation.clone();
    let content = column()
        .p(24.)
        .gap(16.)
        .child(text("Receive a file").heading(1).text_size(24.))
        .child(
            column().gap(6.).child(text("Private IPv4 address")).child(
                text_input("Private IPv4 address", screen.address)
                    .placeholder("e.g. 192.168.1.20")
                    .w(220.)
                    .disabled_when(controls_disabled.clone()),
            ),
        )
        .child(
            column()
                .gap(6.)
                .items_start()
                .child(
                    button()
                        .child(text("Choose save folder"))
                        .disabled_when(controls_disabled.clone())
                        .on_click(pick_folder),
                )
                .child(
                    text_signal(move || {
                        folder.with(|folder| {
                            folder.as_ref().map_or_else(
                                || "No folder selected".into(),
                                |path| path.display().to_string(),
                            )
                        })
                    })
                    .text_wrap(true),
                ),
        )
        .child(row().child(switch(
            move || matches!(activity.get(), Activity::Receiving | Activity::Cancelling),
            move |active, _| {
                let screen = actions.clone();
                let tasks = tasks.clone();
                if active {
                    let activity = screen.activity.clone();
                    button()
                        .child(text("Cancel"))
                        .disabled_when(move || activity.get() == Activity::Cancelling)
                        .on_click(move || {
                            screen.cancel.store(true, Ordering::Release);
                            screen.activity.set(Activity::Cancelling);
                            screen.invitation.set(String::new());
                            screen.status.set("Cancelling…".into());
                        })
                } else {
                    button()
                        .child(text("Receive once"))
                        .bg(tok::ACCENT)
                        .text_color(tok::ACCENT_TEXT)
                        .disabled_when(controls_disabled.clone())
                        .on_click(move || screen.start(&tasks))
                }
            },
        )))
        .child(
            text_signal(move || status.get())
                .text_wrap(true)
                .live(Live::Polite),
        )
        .child(switch(
            move || !invitation_visible.with(String::is_empty),
            move |visible, _| {
                if !visible {
                    return div();
                }
                let qr = invitation.with(|value| QrCode::new(value.as_bytes()));
                let scan = match qr {
                    Ok(code) => column()
                        .gap(8.)
                        .child(text("Scan invitation").heading(2))
                        .child(invitation_qr(code)),
                    Err(_) => text("Couldn’t make a QR code. Enter the invitation manually."),
                };
                let invitation = invitation.clone();
                column()
                    .gap(8.)
                    .child(scan)
                    .child(text("Manual invitation"))
                    .child(
                        text_signal(move || invitation.get())
                            .text_wrap(true)
                            .selectable(true)
                            .p(12.)
                            .bg(tok::INPUT_BG)
                            .border(1.)
                            .border_color(tok::BORDER_STRONG),
                    )
            },
        ));
    let offset = cx.ui.runtime.signal(0.);
    cx.render(
        scroll(offset)
            .scrollbar(true)
            .bg(tok::SURFACE_BASE)
            .child(content),
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Application::new()
        .window(WindowOptions {
            title: "SEAM".into(),
            width: 560.,
            height: 660.,
            min_size: Some((440., 440.)),
            ..Default::default()
        })
        .run(receive_screen)
}
