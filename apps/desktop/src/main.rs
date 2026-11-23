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
use zgx_desktop::{Application, FileDialogOptions, WindowContext, WindowOptions};

use material::{
    backdrop, field, headline, metadata, mode_action, native_glass, pane, secondary, wordmark,
};

#[cfg(target_os = "windows")]
mod adb;
#[cfg(target_os = "windows")]
mod cable_ui;
mod material;

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

    fn stop(&self) {
        if self.activity.get() == Activity::Receiving {
            self.cancel.store(true, Ordering::Release);
            self.activity.set(Activity::Cancelling);
            self.invitation.set(String::new());
            self.status.set("Cancelling…".into());
        }
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
    const MODULE: f32 = 4.;
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
    let status = screen.status.clone();
    let folder = screen.folder.clone();
    let invitation = screen.invitation.clone();
    let invitation_visible = invitation.clone();
    let status_visible = status.clone();
    column()
        .gap(16.)
        .child(headline("Receive over Wi-Fi"))
        .child(
            column()
                .gap(8.)
                .child(metadata(text("Private IPv4 address")))
                .child(
                    field(text_input("Private IPv4 address", screen.address))
                        .placeholder("e.g. 192.168.1.20")
                        .disabled_when(controls_disabled.clone()),
                ),
        )
        .child(
            column()
                .gap(8.)
                .child(
                    row().child(
                        secondary("Choose save folder")
                            .disabled_when(controls_disabled.clone())
                            .on_click(pick_folder),
                    ),
                )
                .child(metadata(text_signal(move || {
                    folder.with(|folder| {
                        folder.as_ref().map_or_else(
                            || "No folder selected".into(),
                            |path| path.display().to_string(),
                        )
                    })
                }))),
        )
        .child(row().child(switch(
            move || matches!(activity.get(), Activity::Receiving | Activity::Cancelling),
            move |active, _| {
                let screen = actions.clone();
                let tasks = tasks.clone();
                if active {
                    let activity = screen.activity.clone();
                    secondary("Cancel")
                        .disabled_when(move || activity.get() == Activity::Cancelling)
                        .on_click(move || screen.stop())
                } else {
                    secondary("Receive once")
                        .disabled_when(controls_disabled.clone())
                        .on_click(move || screen.start(&tasks))
                }
            },
        )))
        .child(
            switch(
                move || !status_visible.with(String::is_empty),
                move |visible, _| {
                    if visible {
                        let status = status.clone();
                        metadata(text_signal(move || status.get())).live(Live::Polite)
                    } else {
                        div().hidden()
                    }
                },
            )
            .display_contents(),
        )
        .child(
            switch(
                move || !invitation_visible.with(String::is_empty),
                move |visible, _| {
                    if !visible {
                        return div().hidden();
                    }
                    let qr = invitation.with(|value| QrCode::new(value.as_bytes()));
                    let scan = match qr {
                        Ok(code) => column()
                            .gap(8.)
                            .child(metadata(text("Scan invitation").heading(2)))
                            .child(invitation_qr(code)),
                        Err(_) => metadata(text(
                            "Couldn’t make a QR code. Enter the invitation manually.",
                        )),
                    };
                    let invitation = invitation.clone();
                    column()
                        .gap(8.)
                        .child(scan)
                        .child(metadata(text("Manual invitation")))
                        .child(metadata(text_signal(move || invitation.get())).selectable(true))
                },
            )
            .display_contents(),
        )
}

fn receive_screen(
    cx: &mut WindowContext,
    glass: bool,
    #[cfg(target_os = "windows")] workers: cable_ui::Workers,
) {
    cx.window.set_theme(Some(zgx::theme::Appearance::Dark));
    let runtime = &cx.ui.runtime;
    let wifi = runtime.signal(false);
    let screen = Screen {
        address: runtime.signal(String::new()),
        folder: runtime.signal(None),
        activity: runtime.signal(Activity::Idle),
        status: runtime.signal(String::new()),
        invitation: runtime.signal(String::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let cancel = screen.cancel.clone();
    cx.on_closed(move || cancel.store(true, Ordering::Release));
    #[cfg(target_os = "windows")]
    let cable = cable_ui::Cable::new(cx, workers);
    let mode = wifi.clone();
    let body_mode = wifi.clone();
    let dialogs = cx.dialogs.clone();
    let tasks = cx.tasks.clone();
    let wifi_screen = screen.clone();
    #[cfg(target_os = "windows")]
    let cable_screen = cable.clone();
    let offset = cx.ui.runtime.signal(0.);
    let reset_scroll = offset.clone();
    let content = switch(
        move || body_mode.get(),
        move |wifi_active, _| {
            let view = if wifi_active {
                wifi_view(wifi_screen.clone(), dialogs.clone(), tasks.clone())
            } else {
                #[cfg(target_os = "windows")]
                let view = cable_screen.view();
                #[cfg(not(target_os = "windows"))]
                let view = column()
                    .gap(8.)
                    .child(headline("Receive by cable"))
                    .child(metadata(text("Cable receive is available on Windows.")));
                view
            };
            pane(view)
        },
    )
    .w_full()
    .items_center()
    .shrink_0();
    let viewport = cx.viewport.clone();
    let mode_switch = mode_action(text_signal(move || {
        if wifi.get() {
            "Use cable instead"
        } else {
            "Use Wi-Fi instead"
        }
        .into()
    }))
    .on_click(move || {
        let wifi_active = mode.get();
        if wifi_active {
            screen.stop();
        } else if screen.address.with(String::is_empty)
            && let Some(ip) = likely_address()
        {
            screen.address.set(ip.to_string());
        }
        #[cfg(target_os = "windows")]
        cable.enable(wifi_active);
        mode.set(!wifi_active);
        reset_scroll.set(0.);
    });
    cx.render(
        backdrop(cx, glass).child(
            scroll(offset)
                .w_full()
                .grow()
                .min_h(0.)
                .scrollbar(true)
                .child(
                    column()
                        .w_full()
                        .shrink_0()
                        .p(24.)
                        .gap(8.)
                        .items_center()
                        .justify_center()
                        .reactive_style(move || Styles::new().min_h(viewport.get().1))
                        .child(
                            column()
                                .w_full()
                                .shrink_0()
                                .items_center()
                                .gap(24.)
                                .child(wordmark())
                                .child(content),
                        )
                        .child(mode_switch),
                ),
        ),
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    let workers = cable_ui::Workers::default();
    let glass = native_glass();
    let result = Application::new()
        .window(WindowOptions {
            title: "SEAM".into(),
            width: 480.,
            height: 480.,
            min_size: Some((360., 380.)),
            glass,
            ..Default::default()
        })
        .run(|cx| {
            receive_screen(
                cx,
                glass.is_some(),
                #[cfg(target_os = "windows")]
                workers.clone(),
            )
        });
    #[cfg(target_os = "windows")]
    workers.shutdown();
    result
}
