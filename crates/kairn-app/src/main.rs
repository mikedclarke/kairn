mod chrome;
mod cli_install;
mod history;
mod keymap;
mod link_title;
mod name_dialog;
mod note_editor;
mod overlays;
mod panes;
mod session;
mod settings_dialog;
mod sidebar;
mod theme;
mod ui;
mod vault_state;
mod workspace;

use std::borrow::Cow;

use gpui::{
    AppContext, Application, AssetSource, Bounds, SharedString, WindowBounds, WindowOptions, px,
    size,
};
use gpui_component::{Root, TitleBar};
use gpui_component_assets::Assets;
use kairn_core::settings::Settings;

use crate::workspace::Workspace;

/// The component library's embedded assets plus Kairn's own icons: gpui
/// takes a single asset source, so the two sets merge here.
struct KairnAssets;

impl AssetSource for KairnAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let own: Option<&'static [u8]> = match path {
            "kairn-icons/file.svg" => Some(include_bytes!("../assets/icons/file.svg")),
            "kairn-icons/file-text.svg" => Some(include_bytes!("../assets/icons/file-text.svg")),
            "kairn-icons/file-code.svg" => Some(include_bytes!("../assets/icons/file-code.svg")),
            "kairn-icons/file-image.svg" => {
                Some(include_bytes!("../assets/icons/file-image.svg"))
            }
            "kairn-icons/folder.svg" => Some(include_bytes!("../assets/icons/folder.svg")),
            "kairn-icons/folder-open.svg" => {
                Some(include_bytes!("../assets/icons/folder-open.svg"))
            }
            "kairn-icons/folder-symlink.svg" => {
                Some(include_bytes!("../assets/icons/folder-symlink.svg"))
            }
            "kairn-icons/clock.svg" => Some(include_bytes!("../assets/icons/clock.svg")),
            _ => None,
        };
        match own {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Assets.list(path)
    }
}

fn main() {
    let app = Application::new().with_assets(KairnAssets);

    // Dock-icon click when no window is visible: bring the app forward.
    app.on_reopen(|cx| cx.activate(true));

    app.run(move |cx| {
        gpui_component::init(cx);
        workspace::init(cx);
        theme::resolve_fonts(cx);

        let settings = Settings::load();
        theme::apply(&settings, &settings.notes_root(), None, cx);

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
