use zgx::canvas::{Canvas, Path, Stroke};
use zgx::compose::prelude::*;
use zgx::scene::{Color, Rect};
use zgx_desktop::WindowContext;

use crate::material::wordmark;

pub(crate) const HEIGHT: f32 = 36.;
const INK: Color = Color(214, 214, 214, 255);

pub(crate) fn view(
    cx: &WindowContext,
    #[cfg(target_os = "windows")] trusted: View,
    status: View,
) -> View {
    let minimize = cx.window.clone();
    let close = cx.window.clone();

    let bar = row()
        .w_full()
        .h(HEIGHT)
        .shrink_0()
        .items_center()
        .gap(2.)
        .pr(8.)
        .child(
            row()
                .grow()
                .h_full()
                .pl(16.)
                .items_center()
                .window_drag()
                .child(wordmark()),
        );
    #[cfg(target_os = "windows")]
    let bar = bar.child(trusted);
    bar.child(status)
        .child(
            control(Glyph::Minimize, "Minimize")
                .on_click(move || minimize.set_minimized(true))
                .tooltip("Minimize"),
        )
        .children(maximize_controls(cx))
        .child(
            control(Glyph::Close, "Close")
                .on_click(move || close.request_close())
                .tooltip("Close"),
        )
}

#[cfg(windows)]
fn maximize_controls(cx: &WindowContext) -> [View; 1] {
    let ready = cx.window_info.ready.clone();
    let bounds = cx.window_info.bounds.clone();
    let viewport = cx.viewport.clone();
    let window = cx.window.clone();
    [zgx::compose::switch_keeping_focus(
        move || {
            bounds.with(|_| ());
            viewport.with(|_| ());
            if ready.get() {
                crate::acrylic::is_maximized()
            } else {
                None
            }
        },
        move |maximized, _| {
            let (glyph, label) = if maximized == Some(true) {
                (Glyph::Restore, "Restore")
            } else {
                (Glyph::Maximize, "Maximize")
            };
            let window = window.clone();
            control(glyph, label)
                .disabled(maximized.is_none())
                .on_click(move || {
                    if let Some(maximized) = crate::acrylic::is_maximized() {
                        window.set_maximized(!maximized);
                    }
                })
                .tooltip(label)
        },
    )
    .contents()]
}

#[cfg(not(windows))]
fn maximize_controls(cx: &WindowContext) -> [View; 2] {
    let maximize = cx.window.clone();
    let restore = cx.window.clone();
    [
        control(Glyph::Maximize, "Maximize")
            .on_click(move || maximize.set_maximized(true))
            .tooltip("Maximize"),
        control(Glyph::Restore, "Restore")
            .on_click(move || restore.set_maximized(false))
            .tooltip("Restore"),
    ]
}

fn control(glyph: Glyph, label: &'static str) -> View {
    button()
        .size(36., 30.)
        .shrink_0()
        .p(0.)
        .items_center()
        .justify_center()
        .rounded(4.)
        .border(0.)
        .bg(Color(0, 0, 0, 0))
        .hover(|s| s.bg(Color(255, 255, 255, 22)))
        .active(|s| s.bg(Color(255, 255, 255, 42)))
        .focus(|s| s.outline(2., INK).outline_offset(-2.))
        .accessible_label(label)
        .child(canvas(move |_| glyph.draw()).size(12., 12.))
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Glyph {
    fn draw(self) -> Canvas {
        let mut drawing = Canvas::new();
        let mut path = Path::builder();
        match self {
            Self::Minimize => {
                path.move_to(1.5, 6.).line_to(10.5, 6.);
            }
            Self::Maximize => {
                path.append(&Path::rectangle(Rect::new(1.5, 1.5, 9., 9.)));
            }
            Self::Restore => {
                path.append(&Path::rectangle(Rect::new(1.5, 4., 6.5, 6.5)));
                path.move_to(4., 4.)
                    .line_to(4., 1.5)
                    .line_to(10.5, 1.5)
                    .line_to(10.5, 8.)
                    .line_to(8., 8.);
            }
            Self::Close => {
                path.move_to(2.5, 2.5)
                    .line_to(9.5, 9.5)
                    .move_to(9.5, 2.5)
                    .line_to(2.5, 9.5);
            }
        }
        drawing.stroke(
            path.build().expect("valid titlebar glyph"),
            INK,
            Stroke {
                width: 1.25,
                ..Default::default()
            },
        );
        drawing
    }
}
