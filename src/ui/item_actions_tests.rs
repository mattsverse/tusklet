use super::test_support::{click, draw, keys, workspace};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, size};
use gpui_component::Root;
use std::sync::mpsc;

#[gpui::test]
fn database_header_does_not_start_an_unknown_container(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    view.update(cx, |view, _| {
        view.snapshot.databases[0].state = ContainerState::Unknown;
    });
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    click(cx, "database-toggle");
    assert!(
        requests.try_recv().is_err(),
        "header started an unknown container"
    );
    click(cx, "database-settings");
    view.update(cx, |view, _| {
        assert!(matches!(&view.form.as_ref().unwrap().kind,
            FormKind::Database { editing: Some(id), .. } if id == "database"));
    });
}

#[gpui::test]
fn database_header_starts_stopped_and_stops_active_containers(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (events, receiver) = mpsc::channel();
    view.update(cx, |view, _| view.events = Some(receiver));
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    for (state, stops) in [
        (ContainerState::NotCreated, false),
        (ContainerState::Stopped, false),
        (ContainerState::Running, true),
        (ContainerState::Starting, true),
        (ContainerState::Unhealthy, true),
    ] {
        view.update(cx, |view, cx| {
            view.snapshot.databases[0].state = state;
            cx.notify();
        });
        click(cx, "database-toggle");
        match requests.try_recv().unwrap() {
            Request::Execute(Action::Start(id)) if !stops => assert_eq!(id, "database"),
            Request::Execute(Action::Stop(id)) if stops => assert_eq!(id, "database"),
            _ => panic!("wrong header action"),
        }
        assert!(requests.try_recv().is_err());
        events
            .send(Event::Completed(Ok("Done".into()), None))
            .unwrap();
        view.update(cx, Tusklet::receive);
    }
}

#[gpui::test]
fn database_header_blocks_unready_unavailable_and_busy_workspaces(cx: &mut TestAppContext) {
    cx.update(init);
    for (ready, available, busy) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let (view, requests) = workspace(cx);
        view.update(cx, |view, _| {
            view.ready = ready;
            if !available {
                view.worker = None;
            }
            if busy {
                view.busy = Some("Working…".into());
                view.pending_action = Some(Action::Start("other-database".into()));
            }
        });
        let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
        click(cx, "database-toggle");
        click(cx, "database-settings");
        assert!(requests.try_recv().is_err());
        view.update(cx, |view, _| assert!(view.form.is_none()));
    }
}

#[gpui::test]
fn database_header_settings_remain_viewable_when_active_or_offline(cx: &mut TestAppContext) {
    cx.update(init);
    for (state, error) in [
        (ContainerState::Running, None),
        (ContainerState::Stopped, Some("Docker offline".into())),
    ] {
        let (view, requests) = workspace(cx);
        view.update(cx, |view, _| {
            view.snapshot.databases[0].state = state;
            view.snapshot.docker_error = error;
        });
        let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
        click(cx, "database-settings");
        click(cx, "save-form");
        assert!(requests.try_recv().is_err());
        view.update(cx, |view, _| {
            assert!(matches!(&view.form.as_ref().unwrap().kind,
                FormKind::Database { editing: Some(id), .. } if id == "database"));
        });
    }
}

#[gpui::test]
fn database_header_rechecks_state_before_dispatching_a_stale_click(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    let button = cx.debug_bounds("database-toggle").unwrap();
    // Leave the rendered Start callback in place while the underlying state changes.
    view.update(cx, |view, _| {
        view.snapshot.databases[0].state = ContainerState::Running;
    });
    cx.simulate_click(button.center(), Modifiers::none());
    assert!(
        requests.try_recv().is_err(),
        "stale header action was dispatched"
    );
}

#[gpui::test]
fn database_menus_start_and_stop_the_clicked_database(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    draw(cx);
    let row = cx.debug_bounds("db-other-database").unwrap();
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::none());
    draw(cx);
    cx.simulate_keystrokes("down enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::Start(id)) if id == "other-database")
    );
    view.update(cx, |view, cx| {
        assert_eq!(view.selected.as_deref(), Some("database"));
        view.busy = None;
        view.pending_action = None;
        view.snapshot.databases[1].state = ContainerState::Running;
        cx.notify();
    });
    click(cx, "database-actions-other-database");
    cx.simulate_keystrokes("down enter");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::Stop(id)) if id == "other-database")
    );
    assert!(requests.try_recv().is_err());
}

#[gpui::test]
fn collapsed_project_settings_remove_requires_its_saved_name(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    click(cx, "project-actions-empty-project");
    cx.simulate_keystrokes("down down enter");
    view.update(cx, |view, _| {
        assert!(matches!(&view.form.as_ref().unwrap().kind, FormKind::Project(Some(id)) if id == "empty-project"));
    });
    click(cx, "settings-remove");
    click(cx, "save-form");
    assert!(requests.try_recv().is_err());
    cx.update(|window, cx| view.update(cx, |view, cx| {
        let form = view.form.as_ref().unwrap();
        assert!(matches!(&form.kind, FormKind::Confirm { action: Action::DeleteProject(id), .. } if id == "empty-project"));
        form.fields[0].update(cx, |field, cx| field.set_value("Empty project", window, cx));
    }));
    click(cx, "save-form");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::DeleteProject(id)) if id == "empty-project")
    );
}

#[gpui::test]
fn database_settings_remove_preserves_volume_and_rechecks_state(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    cx.simulate_resize(size(px(1180.), px(1300.)));
    click(cx, "database-actions-other-database");
    cx.simulate_keystrokes("down down enter");
    click(cx, "settings-remove");
    cx.update(|window, cx| view.update(cx, |view, cx| {
        let form = view.form.as_ref().unwrap();
        assert!(matches!(&form.kind, FormKind::Confirm { action: Action::DeleteDatabase { id, remove_volume: false }, .. }
            if id == "other-database"));
        form.fields[0].update(cx, |field, cx| field.set_value("Other database", window, cx));
        view.snapshot.databases[1].state = ContainerState::Running;
        cx.notify();
    }));
    click(cx, "save-form");
    assert!(requests.try_recv().is_err());
    view.update(cx, |view, cx| {
        assert!(
            view.notice
                .as_ref()
                .unwrap()
                .0
                .contains("Stop this database")
        );
        view.snapshot.databases[1].state = ContainerState::Stopped;
        cx.notify();
    });
    click(cx, "save-form");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::DeleteDatabase { id, remove_volume: false }) if id == "other-database")
    );
}

#[gpui::test]
fn volume_deletion_is_explicit_resets_on_cancel_and_requires_confirmation(cx: &mut TestAppContext) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    cx.simulate_resize(size(px(900.), px(640.)));
    click(cx, "database-actions-other-database");
    cx.simulate_keystrokes("down down down enter");
    draw(cx);
    keys(cx, "tab space");
    draw(cx);
    view.update(cx, |view, _| {
        assert_eq!(
            view.form.as_ref().unwrap().kind.presentation().3,
            "Remove database and data"
        );
    });
    click(cx, "save-form");
    assert!(requests.try_recv().is_err());
    click(cx, "cancel-form");

    click(cx, "database-actions-other-database");
    cx.simulate_keystrokes("down down down enter");
    view.update(cx, |view, _| {
        assert!(matches!(
            &view.form.as_ref().unwrap().kind,
            FormKind::Confirm {
                action: Action::DeleteDatabase {
                    remove_volume: false,
                    ..
                },
                ..
            }
        ));
    });
    click(cx, "remove-data-volume");
    click(cx, "remove-data-volume");
    view.update(cx, |view, _| {
        assert_eq!(
            view.form.as_ref().unwrap().kind.presentation().3,
            "Remove database"
        );
    });
    click(cx, "remove-data-volume");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.form.as_ref().unwrap().fields[0].update(cx, |field, cx| {
                field.set_value("Other database", window, cx);
            });
        });
    });
    click(cx, "save-form");
    assert!(
        matches!(requests.try_recv().unwrap(), Request::Execute(Action::DeleteDatabase {
        id, remove_volume: true
    }) if id == "other-database")
    );
}

#[gpui::test]
fn stale_menu_actions_cannot_remove_running_databases_or_nonempty_projects(
    cx: &mut TestAppContext,
) {
    cx.update(init);
    let (view, requests) = workspace(cx);
    let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
    click(cx, "database-actions-other-database");
    view.update(cx, |view, cx| {
        view.snapshot.databases[1].state = ContainerState::Running;
        cx.notify();
    });
    // The open menu still offers Remove, but its callback must use the latest state.
    cx.simulate_keystrokes("down down down enter");
    view.update(cx, |view, _| assert!(view.form.is_none()));
    assert!(requests.try_recv().is_err());
    view.update(cx, |view, _| {
        let db = ItemTarget::Database("other-database".into());
        let project = ItemTarget::Project("project".into());
        assert!(!view.item_command_enabled(&project, ItemCommand::Remove));
        for state in [
            ContainerState::Starting,
            ContainerState::Unhealthy,
            ContainerState::Unknown,
        ] {
            view.snapshot.databases[1].state = state;
            assert!(!view.item_command_enabled(&db, ItemCommand::Remove));
        }
        view.snapshot.databases[1].state = ContainerState::Stopped;
        view.snapshot.docker_error = Some("Docker unavailable".into());
        assert!(!view.item_command_enabled(&db, ItemCommand::Start));
        assert!(!view.item_command_enabled(&db, ItemCommand::Remove));
        assert!(view.item_command_enabled(&db, ItemCommand::Settings));
        assert!(view.item_command_enabled(
            &ItemTarget::Project("empty-project".into()),
            ItemCommand::Remove
        ));
        view.busy = Some("Working".into());
        assert!(!view.item_command_enabled(&db, ItemCommand::Settings));
    });
}
