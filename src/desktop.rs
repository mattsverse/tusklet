use gpui::{
    App, Bounds, Entity, Global, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use gpui_component::Root;
use std::path::PathBuf;

use crate::ui::Tusklet;

/// The application owns the workspace, so closing a window cannot drop its worker,
/// pending operation, or latest snapshot. Reopened windows use this same entity.
pub struct Desktop {
    workspace: Entity<Tusklet>,
    #[cfg(target_os = "macos")]
    tray: Option<crate::tray::Tray>,
}

impl Global for Desktop {}

pub fn initialize(data_path: anyhow::Result<PathBuf>, cx: &mut App) {
    let workspace = cx.new(|cx| Tusklet::new(data_path, cx));
    cx.set_global(Desktop {
        workspace,
        #[cfg(target_os = "macos")]
        tray: None,
    });
    open_window(cx);
    #[cfg(target_os = "macos")]
    initialize_tray(cx);
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            if keeps_running(cx) {
                let workspace = cx.global::<Desktop>().workspace.clone();
                workspace.update(cx, Tusklet::window_closed);
            } else {
                cx.quit();
            }
        }
    })
    .detach();
}

fn keeps_running(cx: &App) -> bool {
    #[cfg(target_os = "macos")]
    return cx.global::<Desktop>().tray.is_some();
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cx;
        false
    }
}

pub fn open_window(cx: &mut App) {
    let Some(desktop) = cx.try_global::<Desktop>() else {
        return;
    };
    let workspace = desktop.workspace.clone();
    if let Some(window) = cx.windows().first().copied() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    } else {
        let bounds = Bounds::centered(None, size(px(1180.), px(790.)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(640.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Tusklet".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Root::new(workspace.clone(), window, cx)),
        ) {
            eprintln!("Could not open Tusklet: {error}");
            if !keeps_running(cx) {
                cx.quit();
            }
            return;
        }
        workspace.update(cx, Tusklet::window_opened);
    }
    cx.activate(true);
}

#[cfg(target_os = "macos")]
fn initialize_tray(cx: &mut App) {
    use crate::tray::{Command, Tray};
    use std::time::Duration;

    let workspace = cx.global::<Desktop>().workspace.clone();
    match Tray::new(workspace.read(cx).tray_model()) {
        Ok(tray) => cx.global_mut::<Desktop>().tray = Some(tray),
        Err(error) => {
            workspace.update(cx, |workspace, cx| workspace.tray_error(&error, cx));
            return;
        }
    }
    cx.observe(&workspace, |workspace, cx| {
        let model = workspace.read(cx).tray_model();
        if let Some(tray) = &mut cx.global_mut::<Desktop>().tray
            && let Err(error) = tray.update(model)
        {
            eprintln!("Could not update Tusklet menu bar: {error:#}");
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            if cx
                .update(|cx| {
                    let desktop = cx.global::<Desktop>();
                    let commands = desktop
                        .tray
                        .as_ref()
                        .map(Tray::take_commands)
                        .unwrap_or_default();
                    let workspace = desktop.workspace.clone();
                    for command in commands {
                        match command {
                            Command::Open => open_window(cx),
                            Command::Quit => cx.quit(),
                            command => workspace.update(cx, |workspace, cx| {
                                workspace.execute_tray(&command, cx);
                            }),
                        }
                    }
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}
