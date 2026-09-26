use gpui::{
    AnyElement, App, ClipboardItem, Context, Div, Entity, FocusHandle, Focusable, FontWeight,
    ImageSource, PathPromptOptions, Resource, ScrollHandle, SharedString, Window, div, img,
    prelude::*, px, rgb,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputState},
};
mod item_actions;
mod keyboard;
mod palette;
use keyboard::{Keyboard, reveal_on_focus};
#[cfg(all(test, feature = "ui-tests"))]
mod item_actions_tests;
#[cfg(all(test, feature = "ui-tests"))]
mod keyboard_tests;
#[cfg(all(test, feature = "ui-tests"))]
mod test_support;
use item_actions::{ItemCommand, ItemTarget};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
    time::Duration,
};
use tusklet::{
    model::{ContainerState, DatabaseConfig, DatabaseView, Project, Snapshot},
    service::{Action, Event, Request, spawn_worker},
};

const BG: u32 = 0x11_16_18;
const PANEL: u32 = 0x18_1e_21;
const LINE: u32 = 0x2b_34_37;
const MUTED: u32 = 0x93_a3_a6;
const TEXT: u32 = 0xe7_ee_eb;
const GREEN: u32 = 0xa9_d6_ad;

#[derive(Clone)]
enum FormKind {
    Project(Option<String>),
    Database {
        project: String,
        editing: Option<String>,
    },
    Confirm {
        action: Action,
        name: String,
        description: String,
    },
}

impl FormKind {
    fn presentation(&self) -> (&'static str, String, Vec<&'static str>, &'static str) {
        match self {
            FormKind::Project(id) => (
                if id.is_some() {
                    "Project settings"
                } else {
                    "New project"
                },
                "A place to keep related databases together.".into(),
                vec!["Project name"],
                "Save project",
            ),
            FormKind::Database { editing, .. } => (
                if editing.is_some() {
                    "Database settings"
                } else {
                    "New database"
                },
                "One PostgreSQL container, with its own persistent data volume.".into(),
                vec![
                    "Display name",
                    "PostgreSQL image tag",
                    "Initial database",
                    "PostgreSQL user",
                    "Reserved host port (optional)",
                    "PostgreSQL arguments (optional)",
                ],
                "Save database",
            ),
            FormKind::Confirm {
                action,
                description,
                ..
            } => (
                match action {
                    Action::DeleteProject(_) => "Remove project",
                    Action::DeleteDatabase { .. } => "Remove database",
                    _ => "Confirm operation",
                },
                description.clone(),
                vec!["Type the name to confirm"],
                match action {
                    Action::DeleteProject(_) => "Remove project",
                    Action::DeleteDatabase {
                        remove_volume: true,
                        ..
                    } => "Remove database and data",
                    Action::DeleteDatabase { .. } => "Remove database",
                    _ => "Confirm",
                },
            ),
        }
    }
}

struct Form {
    kind: FormKind,
    fields: Vec<Entity<InputState>>,
    return_focus: Option<FocusHandle>,
}

pub struct Tusklet {
    snapshot: Snapshot,
    selected: Option<String>,
    project: Option<String>,
    worker: Option<Sender<Request>>,
    events: Option<Receiver<Event>>,
    form: Option<Form>,
    notice: Option<(String, bool)>,
    busy: Option<String>,
    pending_action: Option<Action>,
    ready: bool,
    follow: bool,
    window_visible: bool,
    logs: String,
    log_scroll: ScrollHandle,
    project_scroll: ScrollHandle,
    form_scroll: ScrollHandle,
    keyboard: Option<Keyboard>,
    restore_focus: Option<FocusHandle>,
    menu: Option<item_actions::OpenMenu>,
    palette: Option<palette::CommandPalette>,
}

pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    keyboard::init(cx);
    palette::init(cx);
}

impl Tusklet {
    pub fn new(data_path: anyhow::Result<PathBuf>, cx: &mut Context<Self>) -> Self {
        let mut app = Self {
            snapshot: Snapshot::default(),
            selected: None,
            project: None,
            worker: None,
            events: None,
            form: None,
            notice: None,
            busy: None,
            pending_action: None,
            ready: false,
            follow: true,
            window_visible: false,
            logs: String::new(),
            log_scroll: ScrollHandle::new(),
            project_scroll: ScrollHandle::new(),
            form_scroll: ScrollHandle::new(),
            keyboard: None,
            restore_focus: None,
            menu: None,
            palette: None,
        };
        match data_path {
            Ok(path) => {
                let (worker, events) = spawn_worker(path);
                app.worker = Some(worker);
                app.events = Some(events);
            }
            Err(error) => app.notice = Some((format!("{error:#}"), true)),
        }
        cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if entity.update(cx, Self::receive).is_err() {
                    break;
                }
            }
        })
        .detach();
        app
    }

    fn receive(&mut self, cx: &mut Context<Self>) {
        let events: Vec<_> = self
            .events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                Event::Snapshot(snapshot, log_id) => {
                    if self.follow
                        && self.window_visible
                        && log_id == self.selected
                        && self.logs != snapshot.logs
                    {
                        self.logs.clone_from(&snapshot.logs);
                        self.log_scroll.scroll_to_bottom();
                    }
                    self.snapshot = *snapshot;
                    self.ready = true;
                    if !self
                        .snapshot
                        .projects
                        .iter()
                        .any(|p| Some(&p.id) == self.project.as_ref())
                    {
                        self.project = self.snapshot.projects.first().map(|p| p.id.clone());
                    }
                    if !self
                        .snapshot
                        .databases
                        .iter()
                        .any(|db| Some(&db.database.id) == self.selected.as_ref())
                    {
                        let next = self
                            .snapshot
                            .databases
                            .iter()
                            .find(|db| Some(&db.database.project_id) == self.project.as_ref())
                            .map(|db| db.database.id.clone());
                        if next != self.selected {
                            self.select(next, cx);
                        }
                    }
                }
                Event::Completed(result, selection) => {
                    self.busy = None;
                    self.pending_action = None;
                    match result {
                        Ok(message) => {
                            if let Some(form) = self.form.take() {
                                self.restore_focus = form.return_focus;
                            }
                            self.notice = Some((message, false));
                            if let Some(id) = selection {
                                self.select(Some(id), cx);
                            }
                        }
                        Err(error) => self.notice = Some((error, true)),
                    }
                }
                Event::RefreshFailed(error) => {
                    self.notice = Some((error, true));
                }
                Event::WorkerStopped(error) => {
                    self.notice = Some((error, true));
                    self.worker = None;
                    self.busy = None;
                    self.pending_action = None;
                }
            }
        }
        cx.notify();
    }

    fn request(&mut self, request: Request) {
        if self
            .worker
            .as_ref()
            .is_none_or(|worker| worker.send(request).is_err())
        {
            self.notice = Some((
                "The background service is unavailable. Restart Tusklet.".into(),
                true,
            ));
            self.busy = None;
            self.pending_action = None;
            self.worker = None;
        }
    }

    fn execute(&mut self, action: Action, description: &str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(description.into());
        self.pending_action = Some(action.clone());
        self.notice = None;
        self.request(Request::Execute(action));
        cx.notify();
    }

    fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if id != self.selected {
            self.logs.clear();
        }
        self.selected.clone_from(&id);
        if let Some(db) = self
            .snapshot
            .databases
            .iter()
            .find(|db| Some(&db.database.id) == self.selected.as_ref())
        {
            self.project = Some(db.database.project_id.clone());
        }
        self.request(Request::Select(id, self.follow && self.window_visible));
        cx.notify();
    }

    pub fn window_closed(&mut self, cx: &mut Context<Self>) {
        self.window_visible = false;
        // InputState contains window-specific focus handles. A reopened window
        // starts with no editor, while the workspace and pending operation survive.
        self.form = None;
        self.menu = None;
        self.palette = None;
        self.keyboard = None;
        self.restore_focus = None;
        self.project_scroll = ScrollHandle::new();
        self.form_scroll = ScrollHandle::new();
        if self.worker.is_some() {
            self.request(Request::Select(self.selected.clone(), false));
        }
        cx.notify();
    }

    pub fn window_opened(&mut self, cx: &mut Context<Self>) {
        self.window_visible = true;
        self.log_scroll = ScrollHandle::new();
        if self.worker.is_some() {
            self.request(Request::Select(self.selected.clone(), self.follow));
        }
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    pub fn tray_model(&self) -> crate::tray::MenuModel {
        use crate::tray::{Activity, MenuModel};

        let activity = if self.worker.is_none() {
            Activity::Unavailable
        } else if !self.ready {
            Activity::Loading
        } else if let Some(action) = &self.pending_action {
            Activity::Busy(action)
        } else if self.form.is_some() {
            Activity::Editing
        } else {
            Activity::Ready
        };
        let error = self
            .notice
            .as_ref()
            .filter(|(_, error)| *error)
            .map(|(text, _)| text.as_str());
        MenuModel::new(&self.snapshot, activity, error)
    }

    #[cfg(target_os = "macos")]
    pub fn execute_tray(&mut self, command: &crate::tray::Command, cx: &mut Context<Self>) {
        use crate::tray::Command;

        if !self.tray_model().allows(command) {
            return;
        }
        match command {
            Command::Start(id) => self.execute(
                Action::Start(id.clone()),
                "Starting PostgreSQL · downloading image if needed…",
                cx,
            ),
            Command::Stop(id) => self.execute(Action::Stop(id.clone()), "Stopping PostgreSQL…", cx),
            Command::Open | Command::Quit => {}
        }
    }

    #[cfg(target_os = "macos")]
    pub fn tray_error(&mut self, error: &anyhow::Error, cx: &mut Context<Self>) {
        self.notice = Some((
            format!(
                "Could not create the menu bar icon. Closing the window will quit Tusklet. {error:#}"
            ),
            true,
        ));
        cx.notify();
    }

    fn selected_db(&self) -> Option<DatabaseView> {
        self.snapshot
            .databases
            .iter()
            .find(|db| Some(&db.database.id) == self.selected.as_ref())
            .cloned()
    }

    fn open_form(&mut self, kind: FormKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let values = match &kind {
            FormKind::Project(id) => vec![
                id.as_ref()
                    .and_then(|id| self.snapshot.projects.iter().find(|p| &p.id == id))
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
            ],
            FormKind::Database { editing, .. } => {
                let config = editing
                    .as_ref()
                    .and_then(|id| {
                        self.snapshot
                            .databases
                            .iter()
                            .find(|db| &db.database.id == id)
                    })
                    .map(|db| db.database.config.clone())
                    .unwrap_or_default();
                vec![
                    config.name,
                    config.tag,
                    config.database,
                    config.user,
                    config
                        .reserved_port
                        .map(|p| p.to_string())
                        .unwrap_or_default(),
                    config.command,
                ]
            }
            FormKind::Confirm { .. } => vec![String::new()],
        };
        let placeholders = [
            "e.g. Storefront",
            "18, 17-alpine, or any official tag",
            "postgres",
            "postgres",
            "Automatic",
            "-c wal_level=logical -c max_replication_slots=10",
        ];
        let fields: Vec<_> = values
            .into_iter()
            .enumerate()
            .map(|(i, value)| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(match &kind {
                            FormKind::Confirm { name, .. } => name.clone(),
                            _ => placeholders[i].into(),
                        })
                        .default_value(value)
                })
            })
            .collect();
        self.keyboard.get_or_insert_with(|| Keyboard::new(cx));
        let return_focus = self
            .form
            .as_ref()
            .and_then(|form| form.return_focus.clone())
            .or_else(|| {
                self.menu
                    .as_ref()
                    .and_then(|menu| menu.return_focus.clone())
            })
            .or_else(|| window.focused(cx));
        self.form_scroll = ScrollHandle::new();
        let read_only = self.form_read_only(&kind);
        self.form = Some(Form {
            kind,
            fields,
            return_focus,
        });
        if read_only {
            // Focus an enabled button after the new form is laid out.
            self.restore_focus = self.keyboard.as_ref().map(|keyboard| keyboard.body.clone());
        } else {
            self.form.as_ref().unwrap().fields[0].update(cx, |input, cx| input.focus(window, cx));
        }
        self.notice = None;
        cx.notify();
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        if self.busy.is_some() || self.form_read_only(&form.kind) {
            return;
        }
        let values: Vec<_> = form
            .fields
            .iter()
            .map(|field| field.read(cx).value().to_string())
            .collect();
        let action = match &form.kind {
            FormKind::Project(Some(id)) => {
                Action::RenameProject(id.clone(), values[0].trim().into())
            }
            FormKind::Project(None) => Action::CreateProject(values[0].trim().into()),
            FormKind::Database { project, editing } => {
                let port = if values[4].trim().is_empty() {
                    None
                } else {
                    let Ok(port) = values[4].trim().parse::<u16>() else {
                        self.notice = Some((
                            "Port must be a number between 1024 and 65535, or blank for automatic."
                                .into(),
                            true,
                        ));
                        cx.notify();
                        return;
                    };
                    Some(port)
                };
                let config = DatabaseConfig {
                    name: values[0].trim().into(),
                    tag: values[1].trim().into(),
                    database: values[2].trim().into(),
                    user: values[3].trim().into(),
                    reserved_port: port,
                    command: values[5].trim().into(),
                };
                if let Err(error) = config.validate() {
                    self.notice = Some((error.to_string(), true));
                    cx.notify();
                    return;
                }
                match editing {
                    Some(id) => Action::UpdateDatabase(id.clone(), config),
                    None => Action::CreateDatabase(project.clone(), config),
                }
            }
            FormKind::Confirm { action, name, .. } => {
                if values[0] != *name {
                    self.notice = Some((format!("Type {name} exactly to confirm."), true));
                    cx.notify();
                    return;
                }
                let target = match action {
                    Action::DeleteDatabase { id, .. } => Some(ItemTarget::Database(id.clone())),
                    Action::DeleteProject(id) => Some(ItemTarget::Project(id.clone())),
                    _ => None,
                };
                if let Some(target) = target
                    && !self.item_command_enabled(&target, ItemCommand::Remove)
                {
                    self.notice = Some((
                        self.removal_block_reason(&target)
                            .unwrap_or(
                                "This item cannot be removed right now. Cancel and try again.",
                            )
                            .into(),
                        true,
                    ));
                    cx.notify();
                    return;
                }
                action.clone()
            }
        };
        self.execute(action, "Applying changes…", cx);
    }

    fn backup(&mut self, cx: &mut Context<Self>) {
        let Some(db) = self.selected_db() else {
            return;
        };
        let name = format!("{}.dump", db.database.config.database);
        let directory = directories::UserDirs::new()
            .map_or_else(|| PathBuf::from("."), |d| d.home_dir().to_owned());
        let prompt = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |entity, cx| {
            let result = prompt.await;
            let _ = entity.update(cx, |this, cx| match result {
                Ok(Ok(Some(path))) => {
                    this.execute(Action::Dump(db.database.id, path), "Saving backup…", cx);
                }
                Ok(Ok(None)) => {}
                _ => {
                    this.notice = Some(("Could not open the save dialog.".into(), true));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn restore(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(db) = self.selected_db() else {
            return;
        };
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose PostgreSQL backup".into()),
        });
        cx.spawn_in(window, async move |entity, cx| {
            let result = prompt.await;
            let _ = entity.update_in(cx, |this, window, cx| match result {
                Ok(Ok(Some(paths))) if !paths.is_empty() => this.open_form(FormKind::Confirm {
                    action: Action::Restore(db.database.id, paths[0].clone()),
                    name: db.database.config.name.clone(),
                    description: format!("Restore {} into {}. Use an empty database. Existing objects are not dropped; conflicts roll back the restore. Only restore backups you trust. Type the database name to continue.", paths[0].display(), db.database.config.name),
                }, window, cx),
                Ok(Ok(_)) => {},
                _ => { this.notice = Some(("Could not open the file picker.".into(), true)); cx.notify(); }
            });
        }).detach();
    }

    fn project_children(&self, project: &Project, cx: &mut Context<Self>) -> Div {
        let databases: Vec<_> = self
            .snapshot
            .databases
            .iter()
            .filter(|db| db.database.project_id == project.id)
            .collect();
        let mut group = div().flex().flex_col().gap_1();
        for view in &databases {
            let db_id = view.database.id.clone();
            let target = ItemTarget::Database(db_id.clone());
            let selected = self.selected.as_ref() == Some(&db_id);
            group = group.child(reveal_on_focus(
                SharedString::from(format!("database-row-{db_id}")),
                &self.project_scroll,
                div()
                    .ml_3()
                    .border_l_1()
                    .border_color(rgb(LINE))
                    .pl_2()
                    .child(Self::item_row(
                        target.clone(),
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!("db-{db_id}")))
                                    .debug_selector(|| format!("db-{db_id}"))
                                    .ghost()
                                    .flex_1()
                                    .min_w_0()
                                    .justify_start()
                                    .when(selected, |button| {
                                        button.bg(rgb(0x29_3b_33)).text_color(rgb(GREEN))
                                    })
                                    .label(format!(
                                        "{}  {}",
                                        if view.state.is_active() { "●" } else { "○" },
                                        view.database.config.name
                                    ))
                                    .disabled(self.form.is_some())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select(Some(db_id.clone()), cx);
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "database-actions-{}",
                                    view.database.id
                                )))
                                .debug_selector(|| format!("database-actions-{}", view.database.id))
                                .ghost()
                                .small()
                                .label("…")
                                .tooltip("Database actions")
                                .flex_shrink_0()
                                .disabled(self.busy.is_some() || self.form.is_some())
                                .on_click(cx.listener({
                                    let target = target.clone();
                                    move |this, event: &gpui::ClickEvent, window, cx| {
                                        this.open_item_menu(
                                            target.clone(),
                                            event.position(),
                                            matches!(event, gpui::ClickEvent::Keyboard(_)),
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                            ),
                        cx,
                    )),
            ));
        }
        if databases.is_empty() {
            group = group.child(
                div()
                    .pl_6()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("No databases yet"),
            );
        }
        group
    }

    fn project_group(&self, project: &Project, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let id = project.id.clone();
        let target = ItemTarget::Project(id.clone());
        let expanded = self.project.as_ref() == Some(&id);
        div()
            .id(SharedString::from(format!("project-group-{id}")))
            .flex()
            .flex_col()
            .gap_1()
            .child(reveal_on_focus(
                SharedString::from(format!("project-focus-{id}")),
                &self.project_scroll,
                div().child(Self::item_row(
                    target.clone(),
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("project-{id}")))
                                .debug_selector(|| format!("project-{id}"))
                                .ghost()
                                .label(format!(
                                    "{}  {}",
                                    if expanded { "▾" } else { "▸" },
                                    project.name
                                ))
                                .flex_1()
                                .min_w_0()
                                .justify_start()
                                .disabled(self.busy.is_some() || self.form.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.project = Some(id.clone());
                                    let next = this
                                        .snapshot
                                        .databases
                                        .iter()
                                        .find(|db| db.database.project_id == id)
                                        .map(|db| db.database.id.clone());
                                    this.select(next, cx);
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "project-actions-{}",
                                project.id
                            )))
                            .debug_selector(|| format!("project-actions-{}", project.id))
                            .ghost()
                            .small()
                            .label("…")
                            .tooltip("Project actions")
                            .flex_shrink_0()
                            .disabled(self.busy.is_some() || self.form.is_some())
                            .on_click(cx.listener({
                                let target = target.clone();
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_item_menu(
                                        target.clone(),
                                        event.position(),
                                        matches!(event, gpui::ClickEvent::Keyboard(_)),
                                        window,
                                        cx,
                                    );
                                }
                            })),
                        ),
                    cx,
                )),
            ))
            .when(expanded, |group| {
                group.child(self.project_children(project, cx))
            })
    }

    fn docker_status(&self) -> String {
        if !self.ready {
            "Connecting to workspace…".into()
        } else if self.snapshot.docker_error.is_some() {
            "● Docker unavailable".into()
        } else {
            "● Docker connected".into()
        }
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let logo = ImageSource::Resource(Resource::Embedded("tusklet.png".into()));
        let projects = div().flex().flex_col().gap_3().children(
            self.snapshot
                .projects
                .iter()
                .map(|project| self.project_group(project, cx)),
        );
        div()
            .w(px(254.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .p_6()
                    .pb_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child(img(logo).size(px(36.)).flex_shrink_0())
                            .child("Tusklet"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("POSTGRES, WITH A HOME."),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().text_color(rgb(MUTED)).child("WORKSPACE"))
                    .child(
                        Button::new("new-project")
                            .ghost()
                            .small()
                            .label("+ Project")
                            .disabled(!self.ready || self.busy.is_some() || self.form.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form(FormKind::Project(None), window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("projects-scroll")
                    .track_scroll(&self.project_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .child(projects),
            )
            .child(
                div()
                    .p_4()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Button::new("command-palette")
                            .debug_selector(|| "command-palette-trigger".into())
                            .ghost()
                            .small()
                            .label(if cfg!(target_os = "macos") {
                                "Commands  ⌘K"
                            } else {
                                "Commands  Ctrl+K"
                            })
                            .disabled(self.form.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_palette(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(self.docker_status()),
                    ),
            )
    }

    fn form_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = self.form.as_ref().unwrap();
        let (title, subtitle, _, save) = form.kind.presentation();
        let editing = matches!(
            &form.kind,
            FormKind::Database {
                editing: Some(_),
                ..
            }
        );
        let removal_target = match &form.kind {
            FormKind::Project(Some(id)) => Some(ItemTarget::Project(id.clone())),
            FormKind::Database {
                editing: Some(id), ..
            } => Some(ItemTarget::Database(id.clone())),
            _ => None,
        };
        let read_only = self.form_read_only(&form.kind);
        div()
            .id("form-scroll")
            .key_context("TuskletForm")
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Enter, window, cx| {
                    this.finish_composition(window, cx);
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    this.finish_composition(window, cx);
                }),
            )
            .on_action(cx.listener(Self::submit_from_input))
            .on_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    this.cancel_form(window, cx);
                }),
            )
            .track_scroll(&self.form_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_8()
            .child(
                div()
                    .max_w(px(680.))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(
                        div()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(div().mt_2().text_color(rgb(MUTED)).child(subtitle)),
                    )
                    .child(self.form_fields(form, editing, read_only, cx))
                    .children(self.volume_removal_option(form, cx).map(|option| {
                        reveal_on_focus("form-volume-option", &self.form_scroll, option)
                    }))
                    .child(self.form_actions(form, save, read_only, cx))
                    .when_some(removal_target, |view, target| {
                        view.child(reveal_on_focus(
                            "form-removal",
                            &self.form_scroll,
                            self.removal_settings(target, cx),
                        ))
                    }),
            )
            .into_any_element()
    }

    fn form_actions(
        &self,
        form: &Form,
        save: &'static str,
        read_only: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        reveal_on_focus(
            "form-actions",
            &self.form_scroll,
            div()
                .flex()
                .gap_3()
                .child(
                    Button::new("save-form")
                        .debug_selector(|| "save-form".into())
                        .primary()
                        .when(
                            matches!(
                                &form.kind,
                                FormKind::Confirm {
                                    action: Action::DeleteDatabase { .. }
                                        | Action::DeleteProject(_),
                                    ..
                                }
                            ),
                            Button::danger,
                        )
                        .label(save)
                        .disabled(self.busy.is_some() || read_only)
                        .on_click(cx.listener(|this, _, _, cx| this.save_form(cx))),
                )
                .child(
                    Button::new("cancel-form")
                        .debug_selector(|| "cancel-form".into())
                        .ghost()
                        .label("Cancel")
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_form(window, cx))),
                ),
        )
    }

    fn volume_removal_option(&self, form: &Form, cx: &mut Context<Self>) -> Option<Div> {
        let FormKind::Confirm {
            action: Action::DeleteDatabase { id, remove_volume },
            ..
        } = &form.kind
        else {
            return None;
        };
        let database = &self
            .snapshot
            .databases
            .iter()
            .find(|view| &view.database.id == id)?
            .database;
        let description = if *remove_volume {
            format!(
                "Permanently delete {} and all its PostgreSQL data. This cannot be undone.",
                database.volume_name()
            )
        } else {
            format!(
                "Data volume {} will be kept in Docker.",
                database.volume_name()
            )
        };
        let checked = *remove_volume;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    Checkbox::new("remove-data-volume")
                        .debug_selector(|| "remove-data-volume".into())
                        .label("Also delete the data volume")
                        .checked(checked)
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(|this, checked, _, cx| {
                            this.set_remove_volume(*checked, cx);
                        })),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(if *remove_volume { 0xf2_b6_af } else { MUTED }))
                        .child(description),
                ),
        )
    }

    fn set_remove_volume(&mut self, checked: bool, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if let Some(Form {
            kind:
                FormKind::Confirm {
                    action: Action::DeleteDatabase { remove_volume, .. },
                    ..
                },
            ..
        }) = &mut self.form
        {
            *remove_volume = checked;
            cx.notify();
        }
    }

    fn form_fields(
        &self,
        form: &Form,
        editing: bool,
        read_only: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (_, _, labels, _) = form.kind.presentation();
        let name_to_copy = match &form.kind {
            FormKind::Confirm {
                action: Action::DeleteProject(_) | Action::DeleteDatabase { .. },
                name,
                ..
            } => Some(name),
            _ => None,
        };
        let mut fields = div().flex().flex_col().gap_4();
        for (i, label) in labels.into_iter().enumerate() {
            let disabled = self.busy.is_some() || read_only || (editing && (1..=3).contains(&i));
            form.fields[i].focus_handle(cx).tab_stop(!disabled);
            fields = fields.child(reveal_on_focus(
                ("form-field", i),
                &self.form_scroll,
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(div().text_sm().text_color(rgb(MUTED)).child(
                                if name_to_copy.is_some() {
                                    "Type or paste the name to confirm"
                                } else {
                                    label
                                },
                            ))
                            .when_some(name_to_copy, |row, name| {
                                let name = name.clone();
                                let field = form.fields[i].clone();
                                row.child(
                                    Button::new("copy-confirmation-name")
                                        .debug_selector(|| "copy-confirmation-name".into())
                                        .ghost()
                                        .small()
                                        .label("Copy name")
                                        .disabled(disabled)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                name.clone(),
                                            ));
                                            field.update(cx, |input, cx| input.focus(window, cx));
                                            this.notice = Some((
                                                "Name copied. Paste it below to confirm.".into(),
                                                false,
                                            ));
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(Input::new(&form.fields[i]).disabled(disabled)),
            ));
        }
        if matches!(&form.kind, FormKind::Database { .. }) {
            fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                "Cached PostgreSQL tags: {}",
                if self.snapshot.images.is_empty() {
                    "No postgres images downloaded yet".into()
                } else {
                    self.snapshot.images.join(", ")
                }
            )));
            fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child("Cached images work without internet access. Missing images are downloaded when you start."));
            fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child("Leave the port blank to choose an available one. Assigned ports remain reserved in Tusklet, even while stopped. Use -c wal_level=logical to enable logical replication."));
            if editing {
                fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child("To change PostgreSQL versions, create a database and migrate with dump/restore."));
                if read_only {
                    fields =
                        fields.child(div().text_sm().text_color(rgb(MUTED)).child(
                            "Stop the database and connect to Docker to edit these settings.",
                        ));
                }
            }
        }
        fields
    }

    fn removal_settings(&self, target: ItemTarget, cx: &mut Context<Self>) -> Div {
        let (label, description) = match &target {
            ItemTarget::Project(_) => ("Remove project", "Remove this empty project from Tusklet.".into()),
            ItemTarget::Database(id) => ("Remove database", self.snapshot.databases.iter()
                .find(|view| &view.database.id == id)
                .map(|view| format!("Remove the container and its Tusklet entry. Data volume {} is kept by default. You can choose to delete it in the confirmation.", view.database.volume_name()))
                .unwrap_or_default()),
        };
        div()
            .mt_4()
            .pt_6()
            .border_t_1()
            .border_color(rgb(LINE))
            .flex()
            .flex_col()
            .gap_3()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
            .child(div().text_sm().text_color(rgb(MUTED)).child(description))
            .when_some(self.removal_block_reason(&target), |view, reason| {
                view.child(div().text_sm().text_color(rgb(MUTED)).child(reason))
            })
            .child(
                div().child(
                    Button::new("settings-remove")
                        .debug_selector(|| "settings-remove".into())
                        .danger()
                        .label(label)
                        .disabled(!self.item_command_enabled(&target, ItemCommand::Remove))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_item_command(&target, ItemCommand::Remove, window, cx);
                        })),
                ),
            )
    }

    fn database_header_actions(&self, view: &DatabaseView, cx: &mut Context<Self>) -> Div {
        let settings_target = ItemTarget::Database(view.database.id.clone());
        let toggle_target = settings_target.clone();
        let (toggle_command, toggle_button) = if view.state.is_active() {
            (
                ItemCommand::Stop,
                Button::new("stop").label("Stop database"),
            )
        } else {
            (
                ItemCommand::Start,
                Button::new("start").primary().label("Start database"),
            )
        };
        div()
            .flex()
            .gap_2()
            .child(
                Button::new("settings")
                    .debug_selector(|| "database-settings".into())
                    .label("Settings")
                    .disabled(!self.item_command_enabled(&settings_target, ItemCommand::Settings))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_item_command(&settings_target, ItemCommand::Settings, window, cx);
                    })),
            )
            .child(
                toggle_button
                    .debug_selector(|| "database-toggle".into())
                    .disabled(!self.item_command_enabled(&toggle_target, toggle_command))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_item_command(&toggle_target, toggle_command, window, cx);
                    })),
            )
    }

    fn database_view(&self, view: &DatabaseView, cx: &mut Context<Self>) -> AnyElement {
        let db = &view.database;
        let removal_target = ItemTarget::Database(db.id.clone());
        let uri = db.connection_url();
        let unavailable = self.busy.is_some() || self.snapshot.docker_error.is_some();
        let status_color = if view.state == ContainerState::Running {
            GREEN
        } else {
            MUTED
        };
        let logs = if self.logs.is_empty() {
            if view.state == ContainerState::NotCreated {
                "Your database is ready to start.\n\nStart it to see PostgreSQL activity here.\nImages already on this machine are always used first.".into()
            } else {
                "Waiting for PostgreSQL logs…".into()
            }
        } else {
            self.logs.clone()
        };
        let log_lines: Vec<_> = logs
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let color = if line.contains("ERROR:") || line.contains("FATAL:") {
                    0xe9_a3_98
                } else if line.contains("ready to accept connections") {
                    GREEN
                } else {
                    0xb0_c0_c1
                };
                div()
                    .flex()
                    .gap_4()
                    .min_h(px(20.))
                    .child(
                        div()
                            .w(px(30.))
                            .flex_shrink_0()
                            .text_right()
                            .text_color(rgb(0x53_65_69))
                            .child(format!("{}", index + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(color))
                            .child(line.chars().take(2000).collect::<String>()),
                    )
            })
            .collect();
        div().flex_1().min_w_0().min_h_0().flex().flex_col()
            .child(div().debug_selector(|| "database-header".into()).px_8().pt_7().pb_5().flex().items_center().justify_between().gap_4()
                .child(div().debug_selector(|| "database-heading".into()).flex_1().min_w_0().child(div().text_xs().text_color(rgb(MUTED)).child("DATABASE"))
                    .child(div().mt_2().text_2xl().font_weight(FontWeight::SEMIBOLD).child(db.config.name.clone()))
                    .child(div().mt_2().text_sm().text_color(rgb(status_color)).child(format!("● {}   /   {}", view.state.label(), db.image()))))
                .child(self.database_header_actions(view, cx)))
            .child(div().mx_8().mb_5().p_4().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).rounded_lg().flex().items_center().justify_between()
                .child(div().flex().gap_8().child(metric("HOST", "127.0.0.1".into())).child(metric("PORT", db.port.to_string())).child(metric("DATABASE", db.config.database.clone())).child(metric("USER", db.config.user.clone())))
                .child(Button::new("copy-url").ghost().small().label("Copy connection URL").on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(uri.clone())); this.notice = Some(("Connection URL copied, including the database password.".into(), false)); cx.notify();
                }))))
            .child(div().mx_8().mb_4().flex().items_center().gap_2()
                .child(Button::new("dump").small().label("Export backup").disabled(unavailable || view.state != ContainerState::Running).on_click(cx.listener(|this, _, _, cx| this.backup(cx))))
                .child(Button::new("restore").small().label("Restore backup").disabled(unavailable || view.state != ContainerState::Running).on_click(cx.listener(|this, _, window, cx| this.restore(window, cx))))
                .child(div().flex_1())
                .child(Button::new("remove-database").ghost().small().label("Remove database").disabled(!self.item_command_enabled(&removal_target, ItemCommand::Remove)).on_click(cx.listener(move |this, _, window, cx| this.run_item_command(&removal_target, ItemCommand::Remove, window, cx)))))
            .child(div().debug_selector(|| "database-logs".into()).mx_8().mb_6().flex_1().min_h_0().flex().flex_col().border_1().border_color(rgb(LINE)).rounded_lg().overflow_hidden()
                .child(div().px_4().py_3().bg(rgb(PANEL)).border_b_1().border_color(rgb(LINE)).flex().items_center().justify_between()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("PostgreSQL logs"))
                    .child(div().flex().gap_2()
                        .child(Button::new("copy-logs").ghost().xsmall().label("Copy logs").on_click(cx.listener(|this, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(this.logs.clone())))))
                        .child(Button::new("follow").ghost().xsmall().label(if self.follow { "● Following" } else { "Resume follow" }).on_click(cx.listener(|this, _, _, cx| {
                            this.follow = !this.follow; if this.follow { this.log_scroll.scroll_to_bottom(); } this.request(Request::Select(this.selected.clone(), this.follow && this.window_visible)); cx.notify();
                        })))))
                .child(div().id("logs").debug_selector(|| "logs-viewport".into())
                    .track_focus(&self.keyboard.as_ref().unwrap().logs).key_context("TuskletLogs")
                    .border_2().border_color(gpui::transparent_black())
                    .focus(|style| style.border_color(rgb(GREEN)))
                    .on_action(cx.listener(|this, _: &keyboard::LogUp, _, cx| this.scroll_logs(Some(px(20.)), false, cx)))
                    .on_action(cx.listener(|this, _: &keyboard::LogDown, _, cx| this.scroll_logs(Some(px(-20.)), false, cx)))
                    .on_action(cx.listener(|this, _: &keyboard::LogPageUp, _, cx| this.scroll_logs(Some(this.log_scroll.bounds().size.height * 0.9), false, cx)))
                    .on_action(cx.listener(|this, _: &keyboard::LogPageDown, _, cx| this.scroll_logs(Some(this.log_scroll.bounds().size.height * -0.9), false, cx)))
                    .on_action(cx.listener(|this, _: &keyboard::LogHome, _, cx| this.scroll_logs(None, false, cx)))
                    .on_action(cx.listener(|this, _: &keyboard::LogEnd, _, cx| this.scroll_logs(None, true, cx)))
                    .flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.log_scroll).p_4().font_family(if cfg!(target_os = "windows") { "Consolas" } else { "monospace" }).text_xs().children(log_lines))
                .child(div().px_4().py_2().border_t_1().border_color(rgb(LINE)).text_xs().text_color(rgb(MUTED)).child("Latest 400 lines · refreshes every 2 seconds · timestamps from Docker")))
            .into_any_element()
    }

    fn empty_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.project.clone();
        div().flex_1().flex().flex_col().justify_center().items_center().p_10().gap_5()
            .child(div().size(px(70.)).rounded_2xl().bg(rgb(0x29_3b_33)).text_color(rgb(GREEN)).flex().items_center().justify_center().text_3xl().child("PG"))
            .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child("A home for your databases."))
            .child(div().text_color(rgb(MUTED)).max_w(px(440.)).text_center().child("Spin up PostgreSQL, organize it by project, and keep everything close. Your data stays on your machine."))
            .child(Button::new("get-started").primary().label(if project.is_some() { "Create a database" } else { "Create your first project" }).disabled(!self.ready || self.busy.is_some())
                .on_click(cx.listener(move |this, _, window, cx| this.open_form(match &project {
                    Some(id) => FormKind::Database { project: id.clone(), editing: None }, None => FormKind::Project(None)
                }, window, cx))))
            .into_any_element()
    }
}

fn metric(label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(MUTED)).child(label))
        .child(div().text_sm().child(value))
}

impl Render for Tusklet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let keyboard = self.keyboard.get_or_insert_with(|| Keyboard::new(cx));
        let root_focus = keyboard.root.clone();
        let body_focus = keyboard.body.clone();
        if window.focused(cx).is_none() {
            root_focus.focus(window);
        }
        let restore = self.restore_focus.take();
        let fallback = root_focus.clone();
        let body_fallback = body_focus.clone();
        window.defer(cx, move |window, cx| {
            if let Some(target) = restore {
                target.focus(window);
                if target == body_fallback {
                    window.focus_next();
                } else if !fallback.contains_focused(window, cx) {
                    body_fallback.focus(window);
                    window.focus_next();
                }
            } else if !fallback.contains_focused(window, cx) {
                fallback.focus(window);
            }
        });
        let body = if self.form.is_some() {
            self.form_view(cx)
        } else if let Some(db) = self.selected_db() {
            self.database_view(&db, cx)
        } else {
            self.empty_view(cx)
        };
        let menu = self.menu_view(window, cx);
        let palette = self.palette_view(window, cx);
        div().id("workspace").track_focus(&root_focus).key_context("Tusklet")
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| this.record_activation(event, window, cx)))
            .capture_key_up(cx.listener(Self::guard_activation_release))
            .on_action(cx.listener(|this, _: &keyboard::NextControl, window, cx| this.move_focus(false, window, cx)))
            .on_action(cx.listener(|this, _: &keyboard::PreviousControl, window, cx| this.move_focus(true, window, cx)))
            .on_action(cx.listener(|this, _: &palette::TogglePalette, window, cx| this.toggle_palette(window, cx)))
            .relative().size_full().flex().bg(rgb(BG)).text_color(rgb(TEXT)).text_sm()
            .child(self.sidebar(cx))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                .when(self.snapshot.docker_error.is_some(), |el| el.child(div().px_6().py_3().bg(rgb(0x3a_2d_23)).text_color(rgb(0xf0_c6_9b)).flex().items_center().gap_3()
                    .child(div().flex_1().child(format!("Docker unavailable. Start Docker Desktop or Docker Engine. {}", self.snapshot.docker_error.as_deref().unwrap_or("").chars().take(240).collect::<String>())))
                    .child(Button::new("retry").small().label("Retry").disabled(self.busy.is_some()).on_click(cx.listener(|this, _, _, _| this.request(Request::Refresh))))))
                .when_some(self.notice.clone(), |el, (message, error)| el.child(div().px_6().py_3().bg(rgb(if error { 0x3a_27_28 } else { 0x24_35_2c })).flex().items_center().gap_3()
                    .child(div().flex_1().text_color(rgb(if error { 0xf2_b6_af } else { GREEN })).child(message.chars().take(1200).collect::<String>()))
                    .child(Button::new("dismiss").ghost().xsmall().label("Dismiss").on_click(cx.listener(|this, _, _, cx| { this.notice = None; cx.notify(); })))))
                .when_some(self.busy.clone(), |el, message| el.child(div().px_6().py_3().bg(rgb(0x23_30_3b)).child(message)))
                .child(div().id("workspace-body").track_focus(&body_focus).flex_1().min_h_0().flex().flex_col().child(body)))
            .children(menu)
            .children(palette)
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::test_support::created_database;
    use super::*;
    use gpui::{TestAppContext, size};
    use gpui_component::Root;

    #[gpui::test]
    fn refresh_failure_does_not_complete_a_pending_operation(cx: &mut TestAppContext) {
        let (worker, requests) = std::sync::mpsc::channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let view = cx.new(|_| {
            let mut view = created_database();
            view.worker = Some(worker);
            view.events = Some(receiver);
            view
        });

        // An earlier refresh can fail just before the UI queues an operation.
        events
            .send(Event::RefreshFailed("Could not read workspace".into()))
            .unwrap();
        view.update(cx, |view, cx| {
            view.execute(Action::Start("database".into()), "Starting…", cx);
            view.receive(cx);
            assert_eq!(view.busy.as_deref(), Some("Starting…"));
            assert!(matches!(&view.pending_action, Some(Action::Start(id)) if id == "database"));
            assert!(view.worker.is_some());
            assert_eq!(view.notice, Some(("Could not read workspace".into(), true)));
            view.execute(Action::Stop("database".into()), "Stopping…", cx);
        });
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Execute(Action::Start(_))
        ));
        assert!(
            requests.try_recv().is_err(),
            "refresh failure allowed duplicate work"
        );

        events
            .send(Event::Snapshot(
                Box::new(created_database().snapshot),
                Some("database".into()),
            ))
            .unwrap();
        view.update(cx, |view, cx| {
            view.receive(cx);
            assert!(
                view.busy.is_some(),
                "snapshot recovery completed the operation"
            );
        });
        events
            .send(Event::Completed(Err("Start failed".into()), None))
            .unwrap();
        view.update(cx, |view, cx| {
            view.receive(cx);
            assert!(view.busy.is_none());
            assert!(view.pending_action.is_none());
            view.execute(Action::Start("database".into()), "Retrying…", cx);
        });
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Execute(Action::Start(_))
        ));
    }

    #[gpui::test]
    fn stopped_worker_clears_pending_work_and_preserves_the_error_on_reopen(
        cx: &mut TestAppContext,
    ) {
        let (worker, _requests) = std::sync::mpsc::channel();
        let (events, receiver) = std::sync::mpsc::channel();
        let view = cx.new(|_| {
            let mut view = created_database();
            view.worker = Some(worker);
            view.events = Some(receiver);
            view
        });
        view.update(cx, |view, cx| {
            view.execute(Action::Start("database".into()), "Starting…", cx);
        });
        events
            .send(Event::WorkerStopped("Could not open workspace".into()))
            .unwrap();
        view.update(cx, |view, cx| {
            view.receive(cx);
            assert!(view.worker.is_none());
            assert!(view.busy.is_none());
            assert!(view.pending_action.is_none());
            #[cfg(target_os = "macos")]
            assert!(format!("{:?}", view.tray_model()).contains("Workspace unavailable"));
            view.window_closed(cx);
            view.window_opened(cx);
            assert_eq!(view.notice, Some(("Could not open workspace".into(), true)));
        });
    }

    #[gpui::test]
    fn failed_delivery_clears_pending_work(cx: &mut TestAppContext) {
        let (worker, requests) = std::sync::mpsc::channel();
        drop(requests);
        let view = cx.new(|_| {
            let mut view = created_database();
            view.worker = Some(worker);
            view
        });
        view.update(cx, |view, cx| {
            view.execute(Action::Start("database".into()), "Starting…", cx);
            assert!(view.worker.is_none());
            assert!(view.busy.is_none());
            assert!(view.pending_action.is_none());
            assert!(
                view.notice
                    .as_ref()
                    .unwrap()
                    .0
                    .contains("service is unavailable")
            );
        });
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn tray_operations_survive_window_close_and_reject_duplicate_or_stale_actions(
        cx: &mut TestAppContext,
    ) {
        use crate::tray::Command;
        use std::sync::mpsc;

        let (worker, requests) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let view = cx.new(|_| {
            let mut view = created_database();
            view.worker = Some(worker);
            view.events = Some(receiver);
            view
        });
        let start = Command::Start("database".into());
        let stop = Command::Stop("database".into());
        view.update(cx, |view, cx| {
            view.execute_tray(&start, cx);
            view.execute_tray(&start, cx);
            view.window_closed(cx);
            assert!(view.busy.is_some());
            assert!(!view.window_visible);
        });
        assert!(
            matches!(requests.try_recv().unwrap(), Request::Execute(Action::Start(id)) if id == "database")
        );
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Select(_, false)
        ));
        assert!(requests.try_recv().is_err(), "duplicate start was queued");

        let mut snapshot = created_database().snapshot;
        snapshot.databases[0].state = ContainerState::Running;
        events
            .send(Event::Snapshot(Box::new(snapshot), Some("database".into())))
            .unwrap();
        events
            .send(Event::Completed(
                Ok("Started".into()),
                Some("database".into()),
            ))
            .unwrap();
        view.update(cx, |view, cx| {
            view.receive(cx);
            assert!(view.busy.is_none());
            assert!(view.tray_model().allows(&stop));
            view.execute_tray(&start, cx);
        });
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Select(_, false)
        ));
        assert!(requests.try_recv().is_err(), "stale start was queued");
        view.update(cx, |view, cx| {
            view.window_opened(cx);
            assert!(view.window_visible);
            assert_eq!(view.selected.as_deref(), Some("database"));
            assert_eq!(view.selected_db().unwrap().state, ContainerState::Running);
            view.execute_tray(&stop, cx);
        });
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Select(_, true)
        ));
        assert!(
            matches!(requests.try_recv().unwrap(), Request::Execute(Action::Stop(id)) if id == "database")
        );

        events
            .send(Event::Completed(
                Err("Docker stopped responding".into()),
                None,
            ))
            .unwrap();
        view.update(cx, |view, cx| {
            view.receive(cx);
            assert!(view.busy.is_none());
            assert!(view.pending_action.is_none());
            assert_eq!(
                view.notice,
                Some(("Docker stopped responding".into(), true))
            );
            assert!(format!("{:?}", view.tray_model()).contains("Docker stopped responding"));
        });
    }

    #[gpui::test]
    fn hidden_window_pauses_logs_and_reopening_preserves_follow_preference(
        cx: &mut TestAppContext,
    ) {
        use std::sync::mpsc;

        let (worker, requests) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let view = cx.new(|_| {
            let mut view = created_database();
            view.worker = Some(worker);
            view.events = Some(receiver);
            view.logs = "last visible logs".into();
            view
        });
        view.update(cx, Tusklet::window_closed);
        events
            .send(Event::Snapshot(
                Box::new(created_database().snapshot),
                Some("database".into()),
            ))
            .unwrap();
        view.update(cx, |view, cx| {
            // Hidden snapshots contain no logs and must not clear the visible tail.
            view.receive(cx);
            assert_eq!(view.logs, "last visible logs");
            view.window_opened(cx);
            assert!(view.follow);
        });
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Select(_, false)
        ));
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::Select(_, true)
        ));
        view.update(cx, |view, cx| {
            view.follow = false;
            view.window_closed(cx);
            view.window_opened(cx);
            assert!(!view.follow);
        });
        assert!(
            requests
                .try_iter()
                .all(|request| matches!(request, Request::Select(_, false)))
        );
    }

    #[gpui::test]
    fn created_database_keeps_heading_readable_and_logs_visible(cx: &mut TestAppContext) {
        cx.update(init);
        let view = cx.new(|_| created_database());
        let (_, cx) = cx.add_window_view(|window, cx| Root::new(view.clone(), window, cx));
        for show_notice in [true, false] {
            if !show_notice {
                view.update(cx, |view, cx| {
                    view.notice = None;
                    cx.notify();
                });
            }
            for (width, height) in [(1163., 780.), (900., 640.), (1180., 790.), (1600., 1000.)] {
                cx.simulate_resize(size(px(width), px(height)));
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear());

                let heading = cx
                    .debug_bounds("database-heading")
                    .expect("database heading");
                let header = cx.debug_bounds("database-header").expect("database header");
                let logs = cx.debug_bounds("database-logs").expect("database logs");
                assert!(
                    heading.size.width >= px(200.),
                    "heading collapsed: {heading:?}"
                );
                assert!(header.size.height < px(160.), "header expanded: {header:?}");
                assert!(
                    heading.top() >= header.top(),
                    "heading above header: {heading:?}"
                );
                assert!(
                    heading.bottom() <= header.bottom(),
                    "heading below header: {heading:?}"
                );
                assert!(logs.size.height >= px(150.), "logs collapsed: {logs:?}");
                assert!(logs.bottom() <= px(height), "logs outside window: {logs:?}");
            }
        }
    }
}
