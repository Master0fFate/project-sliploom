use zgx::canvas::{Canvas, LineCap, LineJoin, Path, Stroke};
use zgx::compose::prelude::*;
use zgx::scene::Color;
use zgx_desktop::{FontData, Glass, WindowContext};

const FONT_FAMILY: &str = "Roboto";

const INK: Color = Color(16, 16, 16, 255);
const PAPER: Color = Color(250, 250, 250, 255);
const MUTED: Color = Color(189, 189, 189, 255);
const CONTROL: Color = Color(39, 39, 39, 255);
const EDGE: Color = Color(119, 119, 119, 255);

pub(crate) fn roboto() -> Result<FontData, &'static str> {
    FontData::new(include_bytes!("../assets/Roboto.ttf").as_slice())
}

pub(crate) fn native_glass() -> Option<Glass> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Version {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            service_pack: [u16; 128],
        }

        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn RtlGetVersion(version: *mut Version) -> i32;
        }

        let mut version = Version {
            size: size_of::<Version>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            service_pack: [0; 128],
        };
        (unsafe { RtlGetVersion(&mut version) } == 0
            && (version.major, version.build) >= (10, 22621))
            .then(|| Glass::new(zgx_desktop::GlassMaterial::Hud))
    }
    #[cfg(not(windows))]
    None
}

pub(crate) fn backdrop(cx: &WindowContext, native_glass: bool) -> View {
    let theme = cx.ui.theme().clone();
    column()
        .w_full()
        .h_full()
        .font_family(FONT_FAMILY)
        .reactive_style(move || {
            let preferences = theme.system_preferences();
            Styles::new().bg(
                if native_glass
                    && !preferences.reduced_transparency
                    && !preferences.increased_contrast
                {
                    Color(16, 16, 16, 192)
                } else {
                    INK
                },
            )
        })
}

pub(crate) fn pane(content: View) -> View {
    column()
        .w_full()
        .max_w(400.)
        .p(24.)
        .gap(24.)
        .rounded(22.)
        .bg(Color(30, 30, 30, 230))
        .border(1.)
        .border_color(Color(250, 250, 250, 46))
        .child(content)
}

pub(crate) fn wordmark() -> View {
    const MARK_WIDTH: f32 = 36.;
    const MARK_HEIGHT: f32 = 16.;
    const CAP_CENTER_OFFSET: f32 = -0.25;
    let mark = canvas(|_| {
        let top = MARK_HEIGHT * 0.26;
        let bottom = MARK_HEIGHT * 0.74;
        let start = 2.;
        let end = MARK_WIDTH - start;
        let mut path = Path::builder();
        path.move_to(start, top)
            .line_to(MARK_WIDTH * 0.42, top)
            .line_to(MARK_WIDTH * 0.62, bottom)
            .line_to(end, bottom)
            .move_to(start, bottom)
            .line_to(MARK_WIDTH * 0.36, bottom)
            .move_to(MARK_WIDTH * 0.68, top)
            .line_to(end, top);
        let mut drawing = Canvas::new();
        if let Ok(path) = path.build() {
            drawing.stroke(
                path,
                PAPER,
                Stroke {
                    width: 1.75,
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    ..Stroke::default()
                },
            );
        }
        drawing
    })
    .size(MARK_WIDTH, MARK_HEIGHT)
    .translate(0., CAP_CENTER_OFFSET);
    row().shrink_0().gap(8.).items_center().child(mark).child(
        text("SEAM")
            .text_size(14.)
            .font_weight(600)
            .letter_spacing(2.)
            .text_color(PAPER),
    )
}

pub(crate) fn headline(label: &str) -> View {
    text(label)
        .heading(1)
        .text_size(28.)
        .font_weight(600)
        .text_color(PAPER)
        .text_wrap(true)
}

pub(crate) fn metadata(view: View) -> View {
    view.text_size(14.).text_color(MUTED).text_wrap(true)
}

pub(crate) fn secondary(label: &str) -> View {
    action(text(label), false)
}

pub(crate) fn action(label: View, primary: bool) -> View {
    let (fill, ink, border, hover, active) = if primary {
        (PAPER, INK, PAPER, Color(224, 224, 224, 255), MUTED)
    } else {
        (
            CONTROL,
            PAPER,
            EDGE,
            Color(53, 53, 53, 255),
            Color(64, 64, 64, 255),
        )
    };
    button()
        .h(44.)
        .px(16.)
        .rounded(11.)
        .bg(fill)
        .border(1.)
        .border_color(border)
        .text_color(ink)
        .justify_center()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(active))
        .focus(|s| s.outline(2., PAPER).outline_offset(3.))
        .disabled_style(|s| s.bg(CONTROL).text_color(MUTED).border_color(EDGE))
        .child(
            label
                .text_size(16.)
                .font_weight(if primary { 600 } else { 400 }),
        )
}

pub(crate) fn mode_action(label: View) -> View {
    button()
        .h(44.)
        .px(16.)
        .rounded(11.)
        .border(0.)
        .bg(Color(0, 0, 0, 0))
        .hover(|s| s.bg(CONTROL))
        .active(|s| s.bg(Color(53, 53, 53, 255)))
        .focus(|s| s.outline(2., PAPER).outline_offset(3.))
        .child(label.text_size(16.).text_color(MUTED))
}

pub(crate) fn field(view: View) -> View {
    view.h(44.)
        .px(16.)
        .rounded(11.)
        .bg(CONTROL)
        .border(1.)
        .border_color(EDGE)
        .text_size(16.)
        .text_color(PAPER)
        .focus(|s| s.outline(2., PAPER).outline_offset(3.))
}
