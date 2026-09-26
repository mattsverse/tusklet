use super::*;
use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext};
use std::sync::mpsc;
use tusklet::model::Database;

pub(super) fn created_database() -> Tusklet {
    Tusklet {
        snapshot: Snapshot {
            projects: vec![Project {
                id: "project".into(),
                name: "Example project".into(),
            }],
            databases: vec![DatabaseView {
                database: Database {
                    id: "database".into(),
                    project_id: "project".into(),
                    config: DatabaseConfig {
                        name: "Dev".into(),
                        ..DatabaseConfig::default()
                    },
                    password: "test-password".into(),
                    port: 58114,
                },
                state: ContainerState::NotCreated,
            }],
            ..Snapshot::default()
        },
        selected: Some("database".into()),
        project: Some("project".into()),
        worker: None,
        events: None,
        form: None,
        notice: Some((
            "Database created. Start it when you're ready.".into(),
            false,
        )),
        busy: None,
        pending_action: None,
        ready: true,
        follow: true,
        window_visible: true,
        logs: String::new(),
        log_scroll: ScrollHandle::new(),
        project_scroll: ScrollHandle::new(),
        form_scroll: ScrollHandle::new(),
        keyboard: None,
        restore_focus: None,
        menu: None,
        palette: None,
    }
}

pub(super) fn workspace(cx: &mut TestAppContext) -> (Entity<Tusklet>, Receiver<Request>) {
    let (worker, requests) = mpsc::channel();
    let view = cx.new(|_| {
        let mut view = created_database();
        view.worker = Some(worker);
        view.notice = None;
        let mut other = view.snapshot.databases[0].clone();
        other.database.id = "other-database".into();
        other.database.config.name = "Other database".into();
        view.snapshot.databases.push(other);
        view.snapshot.projects.push(Project {
            id: "empty-project".into(),
            name: "Empty project".into(),
        });
        view
    });
    (view, requests)
}

pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
}

pub(super) fn click(cx: &mut VisualTestContext, selector: &'static str) {
    draw(cx);
    let bounds = cx.debug_bounds(selector).expect(selector);
    cx.simulate_click(bounds.center(), Modifiers::none());
    draw(cx);
}

pub(super) fn keys(cx: &mut VisualTestContext, sequence: &str) {
    for key in sequence.split(' ') {
        cx.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            is_held: false,
        });
        draw(cx);
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).unwrap(),
        });
        draw(cx);
    }
}
