#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod ui;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, prelude::*, px, size,
};
use gpui_component::{Root, Theme, ThemeMode};

actions!(handypos, [Quit]);

fn main() {
    let mut args = std::env::args_os().skip(1);
    let data_path = match args.next() {
        Some(arg) if arg == "--data-dir" => match (args.next(), args.next()) {
            (Some(path), None) => Ok(std::path::PathBuf::from(path).join("handypos.sqlite3")),
            _ => Err(anyhow::anyhow!("Usage: handypos [--data-dir DIRECTORY]")),
        },
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!(
                "HandyPOS — a local PostgreSQL workspace\nUsage: handypos [--data-dir DIRECTORY]"
            );
            return;
        }
        Some(_) => Err(anyhow::anyhow!("Usage: handypos [--data-dir DIRECTORY]")),
        None => handypos::store::default_path(),
    };
    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "HandyPOS".into(),
            items: vec![MenuItem::action("Quit HandyPOS", Quit)],
        }]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1180.), px(790.)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(640.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("HandyPOS".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| ui::HandyPos::new(data_path, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        ) {
            eprintln!("Could not open HandyPOS: {error}");
            cx.quit();
        }
        cx.activate(true);
    });
}
