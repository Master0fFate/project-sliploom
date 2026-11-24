use std::rc::Rc;

use zgx::compose::prelude::*;
use zgx::scene::Align;
use zgx::theme::Themes;

use crate::material::pane;

const FRAME_WIDTH: f32 = 960.;

pub(crate) fn view(
    themes: Rc<Themes>,
    heading: View,
    receive: View,
    history: View,
    mode_switch: View,
) -> View {
    let receiver = column()
        .gap(12.)
        .child(
            row()
                .gap(8.)
                .items_center()
                .justify_between()
                .child(heading)
                .child(mode_switch),
        )
        .child(receive);
    column()
        .w_full()
        .max_w(FRAME_WIDTH)
        .min_w(0.)
        .align_self(Align::Center)
        .p(16.)
        .child(
            row()
                .w_full()
                .min_w(0.)
                .gap(20.)
                .items_start()
                .child(pane(receiver, themes).min_w(0.).flex_basis(480.).grow())
                .child(column().min_w(0.).flex_basis(272.).pt(20.).child(history)),
        )
}
