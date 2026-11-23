#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use zgx::compose::prelude::*;
use zgx_desktop::{Application, WindowOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Application::new()
        .window(WindowOptions {
            title: "SEAM".into(),
            width: 480.,
            height: 280.,
            min_size: Some((300., 160.)),
            ..Default::default()
        })
        .run(|cx| {
            cx.render(
                column()
                    .p(24.)
                    .child(text("Window preview").heading(1).text_size(24.)),
            );
        })
}
