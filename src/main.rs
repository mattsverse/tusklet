#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod desktop;
#[cfg(any(target_os = "macos", test))]
mod tray;
mod ui;

use gpui::{App, Application, KeyBinding, Menu, MenuItem, actions};
use gpui_component::{Theme, ThemeMode};

actions!(tusklet, [Open, Quit]);

fn main() {
    let mut args = std::env::args_os().skip(1);
    let data_path = match args.next() {
        Some(arg) if arg == "--data-dir" => match (args.next(), args.next()) {
            (Some(path), None) => Ok(std::path::PathBuf::from(path).join("tusklet.sqlite3")),
            _ => Err(anyhow::anyhow!("Usage: tusklet [--data-dir DIRECTORY]")),
        },
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!(
                "Tusklet — a local PostgreSQL workspace\nUsage: tusklet [--data-dir DIRECTORY]"
            );
            return;
        }
        Some(_) => Err(anyhow::anyhow!("Usage: tusklet [--data-dir DIRECTORY]")),
        None => tusklet::store::default_path(),
    };
    let app = Application::new().with_assets(assets::Assets);
    app.on_reopen(desktop::open_window);
    app.run(move |cx: &mut App| {
        ui::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Open, cx| desktop::open_window(cx));
        cx.set_menus(vec![Menu {
            name: "Tusklet".into(),
            items: vec![
                MenuItem::action("Open Tusklet", Open),
                MenuItem::separator(),
                MenuItem::action("Quit Tusklet", Quit),
            ],
        }]);
        desktop::initialize(data_path, cx);
    });
}
