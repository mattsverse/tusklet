use super::test_support::{click, draw, keys, workspace};
use super::*;
use gpui::{
    Focusable, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext, size,
};

fn tab_to_visible_project(
    cx: &mut VisualTestContext,
    view: &Entity<Tusklet>,
    selector: &'static str,
    key: &str,
) {
    // The limit catches broken focus/scroll behavior without fixing the tab order.
    for _ in 0..128 {
        keys(cx, key);
        let bounds = cx.debug_bounds(selector).expect(selector);
        let viewport = view.read_with(cx, |view, _| view.project_scroll.bounds());
        if bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom() {
            return;
        }
    }
    panic!("{selector} was not revealed by {key} navigation");
}

#[gpui::test]
fn keyboard_preserves_global_shortcuts_in_workspace_menus_and_forms(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};

    cx.update(init);
    let calls = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let calls = calls.clone();
        cx.bind_keys([gpui::KeyBinding::new("secondary-q", crate::Quit, None)]);
        cx.on_action(move |_: &crate::Quit, _| calls.set(calls.get() + 1));
    });
    let (view, _requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(
        cx,
        "secondary-q tab tab tab space secondary-q enter secondary-q",
    );
    assert_eq!(calls.get(), 3);
}

#[gpui::test]
fn keyboard_enters_fresh_window_and_restores_focus_after_cancel(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab");
    let origin = cx.update(|window, cx| window.focused(cx)).unwrap();
    keys(cx, "enter");
    view.update(cx, |view, _| assert!(view.form.is_some()));
    keys(cx, "escape");
    view.update(cx, |view, _| assert!(view.form.is_none()));
    assert!(cx.update(|window, _| origin.is_focused(window)));
    keys(cx, "enter tab tab enter");
    view.update(cx, |view, _| assert!(view.form.is_none()));
    assert!(cx.update(|window, _| origin.is_focused(window)));
}

#[gpui::test]
fn keyboard_opens_project_menu_and_creates_database(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab tab tab space enter");
    view.update(cx, |view, _| {
        assert!(matches!(
            view.form.as_ref().unwrap().kind,
            FormKind::Database { .. }
        ));
    });
    cx.simulate_input("Keyboard database");
    keys(cx, "enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::CreateDatabase(_, config)) if config.name == "Keyboard database")
    );
}

#[gpui::test]
fn keyboard_form_reveals_save_at_minimum_window_size(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    cx.simulate_resize(size(px(900.), px(640.)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_form(
                FormKind::Database {
                    project: "project".into(),
                    editing: None,
                },
                window,
                cx,
            );
        });
    });
    draw(cx);
    cx.simulate_input("Keyboard database");
    keys(cx, "tab tab tab tab tab tab");
    let bounds = cx.debug_bounds("save-form").unwrap();
    assert!(
        bounds.top() >= px(0.) && bounds.bottom() <= px(640.),
        "{bounds:?}"
    );
    keys(cx, "enter");
    assert!(matches!(
        requests.try_recv().unwrap(),
        Request::Execute(Action::CreateDatabase(_, _))
    ));
}

#[gpui::test]
fn keyboard_skips_immutable_database_fields(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_form(
                FormKind::Database {
                    project: "project".into(),
                    editing: Some("database".into()),
                },
                window,
                cx,
            );
        });
    });
    draw(cx);
    keys(cx, "tab");
    cx.update(|window, cx| {
        assert!(
            view.read(cx).form.as_ref().unwrap().fields[4]
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    keys(cx, "shift-tab");
    cx.update(|window, cx| {
        assert!(
            view.read(cx).form.as_ref().unwrap().fields[0]
                .focus_handle(cx)
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn keyboard_checkbox_toggles_once_per_complete_press(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    click(cx, "database-actions-other-database");
    cx.simulate_keystrokes("down down down enter");
    draw(cx);
    keys(cx, "tab space");
    view.update(cx, |view, _| {
        assert_eq!(
            view.form.as_ref().unwrap().kind.presentation().3,
            "Remove database and data"
        );
    });
    keys(cx, "space");
    view.update(cx, |view, _| {
        assert_eq!(
            view.form.as_ref().unwrap().kind.presentation().3,
            "Remove database"
        );
    });
    assert!(requests.try_recv().is_err());
}

#[gpui::test]
fn keyboard_sidebar_reveals_projects_in_both_directions(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    view.update(cx, |view, _| {
        for n in 0..20 {
            view.snapshot.projects.push(Project {
                id: format!("audit-{n}"),
                name: format!("Audit project {n}"),
            });
        }
    });
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    cx.simulate_resize(size(px(900.), px(640.)));
    draw(cx);
    tab_to_visible_project(cx, &view, "project-audit-19", "tab");
    tab_to_visible_project(cx, &view, "project-project", "shift-tab");
    tab_to_visible_project(cx, &view, "project-audit-19", "tab");
    keys(cx, "enter");
    view.update(cx, |view, _| {
        assert_eq!(view.project.as_deref(), Some("audit-19"));
    });
}

#[gpui::test]
fn keyboard_menu_dismissal_and_context_menu_restore_the_trigger(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab tab tab");
    let trigger = cx.update(|window, cx| window.focused(cx)).unwrap();
    keys(cx, "enter");
    view.update(cx, |view, _| assert!(view.menu.is_some()));
    keys(cx, "escape");
    view.update(cx, |view, _| assert!(view.menu.is_none()));
    assert!(cx.update(|window, _| trigger.is_focused(window)));
    keys(cx, "enter tab");
    view.update(cx, |view, _| assert!(view.menu.is_none()));
    keys(cx, "shift-tab");
    assert!(cx.update(|window, _| trigger.is_focused(window)));
    keys(cx, "shift-tab shift-f10");
    view.update(cx, |view, _| assert!(view.menu.is_some()));
    keys(cx, "down enter");
    view.update(cx, |view, _| assert!(matches!(&view.form.as_ref().unwrap().kind, FormKind::Project(Some(id)) if id == "project")));
    keys(cx, "escape tab");
    assert!(cx.update(|window, _| trigger.is_focused(window)));
}

#[gpui::test]
fn keyboard_logs_scroll_pause_follow_and_allow_exit(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    view.update(cx, |view, _| {
        view.logs = "Log line\n".repeat(400);
    });
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    view.update(cx, |view, cx| {
        view.log_scroll.scroll_to_bottom();
        cx.notify();
    });
    draw(cx);
    // The log viewport is the last tab stop, so Shift+Tab from the root reaches it.
    keys(cx, "shift-tab");
    assert!(cx.update(|window, cx| {
        view.read(cx)
            .keyboard
            .as_ref()
            .unwrap()
            .logs
            .is_focused(window)
    }));
    let bottom = view.read_with(cx, |view, _| view.log_scroll.offset().y);
    keys(cx, "up");
    assert!(view.read_with(cx, |view, _| view.log_scroll.offset().y > bottom));
    assert!(matches!(
        requests.try_recv().unwrap(),
        Request::Select(_, false)
    ));
    keys(cx, "home");
    assert_eq!(
        view.read_with(cx, |view, _| view.log_scroll.offset().y),
        px(0.)
    );
    keys(cx, "pagedown");
    assert!(view.read_with(cx, |view, _| view.log_scroll.offset().y < px(0.)));
    keys(cx, "end");
    let bottom_again = view.read_with(cx, |view, _| view.log_scroll.offset().y);
    assert!((bottom_again - bottom).abs() <= px(2.));
    keys(cx, "pageup");
    assert!(view.read_with(cx, |view, _| view.log_scroll.offset().y > bottom_again));
    keys(cx, "shift-tab enter");
    assert!(matches!(
        requests.try_recv().unwrap(),
        Request::Select(_, true)
    ));
    assert!(view.read_with(cx, |view, _| view.follow));
    keys(cx, "tab tab enter");
    view.update(cx, |view, _| {
        assert!(matches!(
            view.form.as_ref().unwrap().kind,
            FormKind::Project(None)
        ));
    });
}

#[gpui::test]
fn keyboard_read_only_settings_focus_cancel_and_cannot_submit(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    view.update(cx, |view, _| {
        view.snapshot.databases[0].state = ContainerState::Running;
    });
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_form(
                FormKind::Database {
                    project: "project".into(),
                    editing: Some("database".into()),
                },
                window,
                cx,
            );
        });
    });
    draw(cx);
    keys(cx, "secondary-enter");
    assert!(requests.try_recv().is_err());
    keys(cx, "enter");
    view.update(cx, |view, _| assert!(view.form.is_none()));
    keys(cx, "tab");
    assert!(cx.update(|window, cx| window.focused(cx)).is_some());
}

#[gpui::test]
fn keyboard_confirmation_requires_explicit_activation_and_saved_name(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab tab tab tab tab enter down down enter");
    view.update(cx, |view, _| {
        assert!(matches!(
            view.form.as_ref().unwrap().kind,
            FormKind::Confirm { .. }
        ));
    });
    cx.simulate_input("Dev");
    keys(cx, "enter secondary-enter");
    assert!(requests.try_recv().is_err());
    keys(cx, "tab space tab enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::DeleteDatabase { id, remove_volume: true }) if id == "database")
    );
    keys(cx, "space enter secondary-enter escape");
    assert!(requests.try_recv().is_err());
    view.update(cx, |view, _| assert!(view.form.is_some()));
}

#[gpui::test]
fn keyboard_completed_form_restores_focus_and_errors_remain_editable(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (events, receiver) = std::sync::mpsc::channel();
    view.update(cx, |view, _| view.events = Some(receiver));
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab");
    let origin = cx.update(|window, cx| window.focused(cx)).unwrap();
    keys(cx, "enter");
    cx.simulate_input("Keyboard project");
    keys(cx, "enter");
    assert!(matches!(
        requests.try_recv().unwrap(),
        Request::Execute(Action::CreateProject(_))
    ));
    events
        .send(Event::Completed(Err("Name already exists".into()), None))
        .unwrap();
    view.update(cx, Tusklet::receive);
    draw(cx);
    cx.update(|window, cx| {
        assert!(
            view.read(cx).form.as_ref().unwrap().fields[0]
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    keys(cx, "secondary-a");
    cx.simulate_input("Unique project");
    keys(cx, "enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::CreateProject(name)) if name == "Unique project")
    );
    events
        .send(Event::Completed(Ok("Project created".into()), None))
        .unwrap();
    view.update(cx, Tusklet::receive);
    draw(cx);
    assert!(cx.update(|window, _| origin.is_focused(window)));
    keys(cx, "enter");
    view.update(cx, |view, _| assert!(view.form.is_some()));
}

#[gpui::test]
fn keyboard_menu_starts_on_an_enabled_command_when_docker_is_unavailable(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    view.update(cx, |view, _| {
        view.snapshot.runtime_error = Some("Offline".into());
    });
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab tab tab tab tab enter");
    view.update(cx, |view, _| assert!(view.menu.is_some(), "menu opened"));
    cx.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("enter").unwrap(),
        is_held: false,
    });
    draw(cx);
    view.update(cx, |view, _| {
        assert!(view.form.is_some(), "form opened on key-down");
    });
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").unwrap(),
    });
    draw(cx);
    view.update(cx, |view, _| {
        assert!(matches!(&view.form.as_ref().unwrap().kind, FormKind::Database { editing: Some(id), .. } if id == "database"));
    });
    assert!(requests.try_recv().is_err());
    keys(cx, "enter");
    view.update(cx, |view, _| assert!(view.form.is_none()));
}

#[gpui::test]
fn keyboard_enter_and_escape_finish_composition_before_acting_on_form(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(cx);
    keys(cx, "tab enter");
    cx.update(|window, cx| {
        let field = view.read(cx).form.as_ref().unwrap().fields[0].clone();
        field.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "入力", None, window, cx);
        });
    });
    draw(cx);
    keys(cx, "enter");
    assert!(requests.try_recv().is_err());
    view.update(cx, |view, _| assert!(view.form.is_some()));
    keys(cx, "escape enter");
    cx.update(|window, cx| {
        let field = view.read(cx).form.as_ref().unwrap().fields[0].clone();
        field.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "入力", None, window, cx);
        });
    });
    draw(cx);
    keys(cx, "escape");
    view.update(cx, |view, _| assert!(view.form.is_some()));
    keys(cx, "escape");
    view.update(cx, |view, _| assert!(view.form.is_none()));
    assert!(requests.try_recv().is_err());
}

#[gpui::test]
fn keyboard_reopening_workspace_creates_fresh_focus_and_remains_operable(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, _requests) = workspace(cx);
    let (_, old_window) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    draw(old_window);
    keys(old_window, "tab enter");
    let previous = view.read_with(old_window, |view, _| {
        view.keyboard.as_ref().unwrap().root.clone()
    });
    view.update(old_window, Tusklet::window_closed);
    old_window.update(|window, _| window.remove_window());
    let (_, cx) =
        cx.add_window_view(|window, cx| gpui_component::Root::new(view.clone(), window, cx));
    view.update(cx, Tusklet::window_opened);
    draw(cx);
    assert_ne!(
        previous,
        view.read_with(cx, |view, _| view.keyboard.as_ref().unwrap().root.clone())
    );
    keys(cx, "tab enter");
    view.update(cx, |view, _| {
        assert!(matches!(
            view.form.as_ref().unwrap().kind,
            FormKind::Project(None)
        ));
    });
}
