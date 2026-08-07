mod panes;
mod session;
mod settings_dialog;
mod sidebar;
mod theme;
mod workspace;

use gpui::{AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_component::{Root, TitleBar};
use gpui_component_assets::Assets;
use kairn_core::settings::Settings;

use crate::theme::Mode;
use crate::workspace::Workspace;

fn main() {
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        workspace::init(cx);

        let settings = Settings::load();
        theme::apply(Mode::from_str(&settings.theme), None, cx);

        let mut titlebar = TitleBar::title_bar_options();
        titlebar.title = Some("Kairn".into());

        let bounds = Bounds::centered(None, size(px(1440.), px(880.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(titlebar),
            app_id: Some("kairn".into()),
            ..Default::default()
        };

        let window = cx.open_window(options, |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(settings, window, cx));
            cx.new(|cx| Root::new(workspace, window, cx))
        });

        match window {
            Ok(_) => cx.activate(true),
            Err(e) => {
                eprintln!("kairn: failed to open window: {e}");
                cx.quit();
            }
        }
    });
}
