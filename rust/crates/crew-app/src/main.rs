// Crew, rebuilt in Rust and GPUI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod i18n;
mod panes;
mod terminal_view;
mod theme;
mod tray;
mod usage_probe;

use gpui::{App, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};
use gpui_component::{Root, TitleBar};
use gpui_platform::application;

fn main() {
    // One copy at a time: launching Crew again brings the running window back.
    if tray::hand_over_to_running(&crew_core::Paths::system()) {
        return;
    }
    application().with_assets(assets::CrewAssets).run(|cx: &mut App| {
        gpui_component::init(cx);
        // Softer edges than ClearType, closer to how the Electron app's text looks.
        cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| app::CrewApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx).bg(gpui::transparent_black()))
            },
        )
        .expect("couldn't open the window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
