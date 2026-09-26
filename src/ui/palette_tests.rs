use super::*;
use crate::ui::{
    init,
    test_support::{click, draw, keys, workspace},
};
use gpui::{Modifiers, TestAppContext, size};
use gpui_component::Root;
use tusklet::service::Action;

#[gpui::test]
fn palette_creates_projects_and_preserves_form_shortcuts(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "secondary-k enter");
    view.update(cx, |view, _| {
        assert!(view.palette.is_none());
        assert!(matches!(
            view.form.as_ref().unwrap().kind,
            FormKind::Project(None)
        ));
    });
    cx.simulate_input("Palette project");
    keys(cx, "secondary-k");
    view.update(cx, |view, cx| {
        assert!(view.palette.is_none(), "palette must not replace a draft");
        assert_eq!(
            view.form.as_ref().unwrap().fields[0].read(cx).value(),
            "Palette project"
        );
    });
    keys(cx, "enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::CreateProject(name)) if name == "Palette project")
    );
    assert!(requests.try_recv().is_err());
}

#[gpui::test]
fn palette_search_creates_database_in_named_project(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    click(cx, "command-palette-trigger");
    cx.simulate_input("  CREATE  database  EMPTY ");
    keys(cx, "enter");
    view.update(cx, |view, _| {
        assert!(matches!(&view.form.as_ref().unwrap().kind,
            FormKind::Database { project, editing: None } if project == "empty-project"));
    });
    cx.simulate_input("Palette database");
    keys(cx, "enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::CreateDatabase(project, config))
        if project == "empty-project" && config.name == "Palette database")
    );
}

#[gpui::test]
fn palette_start_stop_targets_search_result_and_rejects_stale_actions(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "secondary-k");
    cx.simulate_input("start other");
    draw(cx);
    // Background/tray operations can change state before the command is chosen.
    view.update(cx, |view, cx| {
        view.snapshot.databases[1].state = ContainerState::Running;
        cx.notify();
    });
    keys(cx, "enter");
    assert!(requests.try_recv().is_err());
    view.update(cx, |view, _| assert!(view.palette.is_some()));
    cx.simulate_input(" ");
    keys(cx, "enter");
    view.update(cx, |view, _| {
        assert!(view.palette.as_ref().unwrap().matches.is_empty());
    });
    assert!(requests.try_recv().is_err());
    keys(cx, "escape secondary-k");
    cx.simulate_input("stop other");
    click(cx, "palette-result-0");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::Stop(id)) if id == "other-database")
    );
    view.update(cx, |view, cx| {
        assert_eq!(view.selected.as_deref(), Some("database"));
        view.busy = None;
        view.pending_action = None;
        view.snapshot.databases[1].state = ContainerState::Stopped;
        cx.notify();
    });
    keys(cx, "secondary-k");
    cx.simulate_input("start other");
    keys(cx, "enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::Start(id)) if id == "other-database")
    );
    assert!(requests.try_recv().is_err());
}

#[gpui::test]
fn palette_restores_focus_traps_tab_and_scrolls_selection(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    cx.simulate_resize(size(px(900.), px(640.)));
    draw(cx);
    keys(cx, "tab");
    let origin = cx.update(|window, cx| window.focused(cx)).unwrap();
    keys(cx, "secondary-k up");
    cx.update(|window, cx| {
        let palette = view.read(cx).palette.as_ref().unwrap();
        assert_eq!(palette.selected, palette.matches.len() - 1);
        assert!(palette.input.focus_handle(cx).is_focused(window));
        assert!(palette.scroll.offset().y < px(0.));
    });
    keys(cx, "tab");
    cx.update(|window, cx| {
        let palette = view.read(cx).palette.as_ref().unwrap();
        assert_eq!(palette.selected, 0);
        assert!(palette.input.focus_handle(cx).is_focused(window));
    });
    keys(cx, "shift-tab down escape");
    assert!(cx.update(|window, _| origin.is_focused(window)));
    keys(cx, "secondary-k");
    let bounds = cx.debug_bounds("command-palette").unwrap();
    assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(640.));
    cx.simulate_click(gpui::point(px(10.), px(10.)), Modifiers::none());
    draw(cx);
    view.update(cx, |view, _| assert!(view.palette.is_none()));
    assert!(cx.update(|window, _| origin.is_focused(window)));
}

#[gpui::test]
fn palette_empty_search_does_nothing_and_escape_keeps_workspace(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "secondary-k");
    cx.simulate_input("no matching action anywhere");
    keys(cx, "up down tab enter");
    view.update(cx, |view, _| {
        assert!(view.palette.as_ref().unwrap().matches.is_empty());
        assert!(view.form.is_none());
    });
    assert!(requests.try_recv().is_err());
    keys(cx, "escape");
    view.update(cx, |view, _| {
        assert_eq!(view.selected.as_deref(), Some("database"));
    });
}

#[gpui::test]
fn palette_opens_from_item_menu_and_returns_focus_to_its_trigger(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab tab tab");
    let origin = cx.update(|window, cx| window.focused(cx)).unwrap();
    keys(cx, "space secondary-k");
    view.update(cx, |view, _| {
        assert!(view.menu.is_none());
        assert!(view.palette.is_some());
    });
    keys(cx, "escape");
    assert!(cx.update(|window, _| origin.is_focused(window)));
}

#[gpui::test]
fn palette_filters_busy_offline_and_container_specific_commands(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    view.update(cx, |view, _| {
        let labels = |view: &Tusklet| {
            view.palette_entries()
                .iter()
                .map(|entry| entry.label.clone())
                .collect::<Vec<_>>()
        };
        let available = labels(view);
        assert!(available.iter().any(|label| label == "Start database"));
        assert!(
            !available
                .iter()
                .any(|label| label == "Stop database" || label == "Export backup…")
        );
        view.snapshot.databases[0].state = ContainerState::Running;
        let available = labels(view);
        assert!(available.iter().any(|label| label == "Stop database"));
        assert!(available.iter().any(|label| label == "Export backup…"));
        assert!(available.iter().any(|label| label == "Restore backup…"));
        view.snapshot.runtime_error = Some("Offline".into());
        let available = labels(view);
        assert!(available.iter().any(|label| label == "New project"));
        assert!(available.iter().any(|label| label == "New database"));
        assert!(!available.iter().any(|label| label == "Start database"
            || label == "Stop database"
            || label == "Remove database…"
            || label == "Export backup…"));
        view.busy = Some("Working".into());
        let available = labels(view);
        assert!(!available.iter().any(|label| label == "New project"
            || label == "New database"
            || label == "Refresh workspace"));
        view.worker = None;
        assert!(
            !labels(view)
                .iter()
                .any(|label| label == "Refresh workspace")
        );
    });
}

#[gpui::test]
fn palette_switches_database_and_copies_target_connection(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    view.update(cx, |view, _| {
        view.snapshot.databases[1].database.port = 58115;
    });
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "secondary-k");
    cx.simulate_input("copy connection other");
    keys(cx, "enter");
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            view.read(cx).snapshot.databases[1]
                .database
                .connection_url()
        );
    });
    assert!(requests.try_recv().is_err());
    keys(cx, "secondary-k");
    cx.simulate_input("switch database other");
    keys(cx, "enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Select(Some(id), true) if id == "other-database")
    );
    view.update(cx, |view, _| {
        assert_eq!(view.selected.as_deref(), Some("other-database"));
    });
}

#[gpui::test]
fn palette_removal_opens_existing_confirmation_without_deleting(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "secondary-k");
    cx.simulate_input("delete database other");
    keys(cx, "enter");
    view.update(cx, |view, _| {
        assert!(view.palette.is_none());
        assert!(matches!(&view.form.as_ref().unwrap().kind,
            FormKind::Confirm { action: Action::DeleteDatabase { id, remove_volume: false }, .. } if id == "other-database"));
    });
    cx.simulate_input("Other database");
    keys(cx, "enter");
    assert!(requests.try_recv().is_err());
    keys(cx, "escape");
    view.update(cx, |view, _| assert!(view.form.is_none()));
}

#[gpui::test]
fn palette_composition_precedes_commands_and_window_close_clears_state(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "secondary-k");
    cx.update(|window, cx| {
        let input = view.read(cx).palette.as_ref().unwrap().input.clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "new project", None, window, cx);
        });
    });
    keys(cx, "enter");
    view.update(cx, |view, _| {
        assert!(view.palette.is_some());
        assert!(view.form.is_none());
    });
    assert!(requests.try_recv().is_err());
    view.update(cx, Tusklet::window_closed);
    view.update(cx, |view, _| assert!(view.palette.is_none()));
}
