use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use zgx::canvas::{Brush, Canvas, GradientStop, LineCap, LineJoin, Path, Point, Stroke};
use zgx::compose::prelude::*;
use zgx::reactive::Signal;
use zgx::scene::{BoxShadow, Color, Rect, Transform};
use zgx::theme::Themes;
use zgx_desktop::FontData;

const FONT_FAMILY: &str = "Roboto";

const INK: Color = Color(16, 16, 16, 255);
const PAPER: Color = Color(250, 250, 250, 255);
const MUTED: Color = Color(189, 189, 189, 255);
const CONTROL: Color = Color(39, 39, 39, 255);

pub(crate) fn roboto() -> Result<FontData, &'static str> {
    FontData::new(include_bytes!("../assets/Roboto.ttf").as_slice())
}

fn opaque_material(themes: &Themes) -> bool {
    let preferences = themes.system_preferences();
    preferences.increased_contrast || preferences.reduced_transparency
}

const fn neutral_stop(offset: f32, value: u8, alpha: u8) -> GradientStop {
    GradientStop {
        offset,
        color: Color(value, value, value, alpha),
    }
}

pub(crate) fn backdrop(acrylic: Signal<bool>, themes: Rc<Themes>) -> View {
    column()
        .w_full()
        .h_full()
        .font_family(FONT_FAMILY)
        .reactive_style(move || {
            if opaque_material(&themes) {
                Styles::new().bg(INK)
            } else {
                let alpha = if acrylic.get() { 144 } else { 255 };
                Styles::new().bg_radial(
                    (0.5, 0.46),
                    1.,
                    [neutral_stop(0., 43, alpha), neutral_stop(1., 16, alpha)],
                )
            }
        })
}

pub(crate) fn pane(content: View, themes: Rc<Themes>) -> View {
    const RADIUS: f32 = 22.;
    let rim_themes = themes.clone();
    let rim = canvas(move |(width, height)| {
        let mut drawing = Canvas::new();
        if width <= 1. || height <= 1. {
            return drawing;
        }
        let bounds = Rect::new(0.5, 0.5, width - 1., height - 1.);
        let Ok(path) = Path::rounded_rect(bounds, RADIUS - 0.5) else {
            return drawing;
        };
        let brush = if opaque_material(&rim_themes) {
            Color(119, 119, 119, 255).into()
        } else {
            Brush::linear(
                Point::new(0., 0.),
                Point::new(0., height),
                [
                    neutral_stop(0., 119, 255),
                    neutral_stop(0.42, 71, 255),
                    neutral_stop(1., 57, 255),
                ],
            )
            .expect("valid pane rim gradient")
        };
        drawing.stroke(path, brush, Stroke::default());
        drawing
    })
    .absolute()
    .top(0.)
    .right(0.)
    .bottom(0.)
    .left(0.)
    .pointer_events(false);

    column()
        .w_full()
        .p(17.)
        .gap(16.)
        .relative()
        .rounded(RADIUS)
        .reactive_style(move || {
            if opaque_material(&themes) {
                Styles::new().bg(Color(36, 36, 36, 255))
            } else {
                Styles::new()
                    .bg_gradient(
                        FRAC_PI_2,
                        [neutral_stop(0., 53, 189), neutral_stop(1., 30, 189)],
                    )
                    .shadows([BoxShadow {
                        color: Color(0, 0, 0, 80),
                        offset: Transform { x: 0., y: 3. },
                        blur_radius: 5.,
                        spread: 0.,
                    }])
            }
        })
        .child(rim)
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
        text(crate::APP_NAME.to_ascii_uppercase())
            .text_size(14.)
            .font_weight(600)
            .letter_spacing(2.)
            .text_color(PAPER),
    )
}

pub(crate) fn headline(label: &str) -> View {
    text(label)
        .heading(1)
        .text_size(24.)
        .font_weight(500)
        .text_color(PAPER)
        .text_wrap(true)
}

pub(crate) fn metadata(view: View) -> View {
    view.text_size(14.).text_color(MUTED).text_wrap(true)
}

pub(crate) fn primary_action(label: &str) -> View {
    action_button(text(label))
        .w_full()
        .bg(Color(255, 255, 255, 255))
        .text_color(INK)
        .hover(|s| s.bg(Color(232, 232, 232, 255)))
        .active(|s| s.bg(Color(217, 217, 217, 255)))
        .disabled_style(|s| s.bg(CONTROL).text_color(MUTED))
}

pub(crate) fn receive_primary(label: &str) -> View {
    primary_action(label).w(144.)
}

pub(crate) fn receive_actions(primary: View, choose: View) -> View {
    row().gap(8.).items_center().child(primary).child(choose)
}

pub(crate) fn quiet_action(label: View) -> View {
    action_button(label.text_size(14.))
        .h(36.)
        .px(12.)
        .bg(Color(0, 0, 0, 0))
        .text_color(MUTED)
        .hover(|s| s.bg(CONTROL))
        .active(|s| s.bg(Color(53, 53, 53, 255)))
}

fn action_button(label: View) -> View {
    button()
        .h(44.)
        .shrink_0()
        .px(16.)
        .rounded(10.)
        .border(0.)
        .items_center()
        .justify_center()
        .focus(|s| s.outline(2., PAPER).outline_offset(3.))
        .text_size(16.)
        .font_weight(500)
        .child(label)
}
