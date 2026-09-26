use gpui::{
    AnyElement, ClipboardItem, Context, Div, Entity, FontWeight, PathPromptOptions, ScrollHandle,
    SharedString, Window, div, prelude::*, px, rgb,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
};
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
                    "Rename project"
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
            FormKind::Confirm { description, .. } => (
                "Confirm operation",
                description.clone(),
                vec!["Type the name to confirm"],
                "Confirm",
            ),
        }
    }
}

struct Form {
    kind: FormKind,
    fields: Vec<Entity<InputState>>,
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
    ready: bool,
    offline: bool,
    follow: bool,
    logs: String,
    log_scroll: ScrollHandle,
}

impl Tusklet {
    pub fn new(
        data_path: anyhow::Result<PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut app = Self {
            snapshot: Snapshot::default(),
            selected: None,
            project: None,
            worker: None,
            events: None,
            form: None,
            notice: None,
            busy: None,
            ready: false,
            offline: false,
            follow: true,
            logs: String::new(),
            log_scroll: ScrollHandle::new(),
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
                    if self.follow && log_id == self.selected && self.logs != snapshot.logs {
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
                    match result {
                        Ok(message) => {
                            self.form = None;
                            self.notice = Some((message, false));
                            if let Some(id) = selection {
                                self.select(Some(id), cx);
                            }
                        }
                        Err(error) => self.notice = Some((error, true)),
                    }
                }
                Event::Fatal(error) => {
                    self.notice = Some((error, true));
                    self.busy = None;
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
        }
    }

    fn execute(&mut self, action: Action, description: &str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(description.into());
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
        self.request(Request::Select(id, self.follow));
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
                        .placeholder(placeholders[i])
                        .default_value(value)
                })
            })
            .collect();
        fields[0].update(cx, |input, cx| input.focus(window, cx));
        self.form = Some(Form { kind, fields });
        self.notice = None;
        cx.notify();
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
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
        let rename_id = project.id.clone();
        let delete_id = project.id.clone();
        let delete_name = project.name.clone();
        let databases: Vec<_> = self
            .snapshot
            .databases
            .iter()
            .filter(|db| db.database.project_id == project.id)
            .collect();
        let mut group = div().flex().flex_col().gap_1();
        for view in &databases {
            let db_id = view.database.id.clone();
            let selected = self.selected.as_ref() == Some(&db_id);
            group = group.child(
                div()
                    .ml_3()
                    .border_l_1()
                    .border_color(rgb(LINE))
                    .pl_2()
                    .child(
                        Button::new(SharedString::from(format!("db-{db_id}")))
                            .ghost()
                            .w_full()
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
                    ),
            );
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
        group.child(
            div()
                .flex()
                .gap_1()
                .pl_3()
                .child(
                    Button::new(SharedString::from(format!("rename-{rename_id}")))
                        .ghost()
                        .xsmall()
                        .label("Rename")
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_form(FormKind::Project(Some(rename_id.clone())), window, cx);
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("remove-{delete_id}")))
                        .ghost()
                        .xsmall()
                        .label("Remove")
                        .disabled(!databases.is_empty() || self.busy.is_some())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_form(
                                FormKind::Confirm {
                                    action: Action::DeleteProject(delete_id.clone()),
                                    name: delete_name.clone(),
                                    description:
                                        "Remove this empty project. Type its name to confirm."
                                            .into(),
                                },
                                window,
                                cx,
                            );
                        })),
                ),
        )
    }

    fn project_group(&self, project: &Project, cx: &mut Context<Self>) -> Div {
        let id = project.id.clone();
        let new_id = id.clone();
        let expanded = self.project.as_ref() == Some(&id);
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("project-{id}")))
                            .ghost()
                            .label(format!(
                                "{}  {}",
                                if expanded { "▾" } else { "▸" },
                                project.name
                            ))
                            .flex_1()
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
                        Button::new(SharedString::from(format!("add-{new_id}")))
                            .ghost()
                            .small()
                            .label("+")
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(
                                    FormKind::Database {
                                        project: new_id.clone(),
                                        editing: None,
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
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
            format!(
                "● Docker connected · {} local images",
                self.snapshot.images.len()
            )
        }
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
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
                            .disabled(!self.ready || self.busy.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form(FormKind::Project(None), window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("projects-scroll")
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
                        Button::new("offline")
                            .ghost()
                            .small()
                            .label(if self.offline {
                                "● Offline mode on"
                            } else {
                                "○ Offline mode off"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.offline = !this.offline;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(self.docker_status()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("Local data. Your machine."),
                    ),
            )
    }

    fn form_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = self.form.as_ref().unwrap();
        let (title, subtitle, labels, save) = form.kind.presentation();
        let editing = matches!(
            &form.kind,
            FormKind::Database {
                editing: Some(_),
                ..
            }
        );
        let mut fields = div().flex().flex_col().gap_4();
        for (i, label) in labels.into_iter().enumerate() {
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(rgb(MUTED)).child(label))
                    .child(
                        Input::new(&form.fields[i])
                            .disabled(self.busy.is_some() || (editing && (1..=3).contains(&i))),
                    ),
            );
        }
        if matches!(&form.kind, FormKind::Database { .. }) {
            fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                "Available offline: {}",
                if self.snapshot.images.is_empty() {
                    "No postgres images downloaded yet".into()
                } else {
                    self.snapshot.images.join(", ")
                }
            )));
            fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child("Leave the port blank to choose an available one. Assigned ports remain reserved in Tusklet, even while stopped. Use -c wal_level=logical to enable logical replication."));
            if editing {
                fields = fields.child(div().text_xs().text_color(rgb(MUTED)).child("To change PostgreSQL versions, create a database and migrate with dump/restore."));
            }
        }
        div()
            .id("form-scroll")
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
                    .child(fields)
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                Button::new("save-form")
                                    .primary()
                                    .label(save)
                                    .disabled(self.busy.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.save_form(cx))),
                            )
                            .child(
                                Button::new("cancel-form")
                                    .ghost()
                                    .label("Cancel")
                                    .disabled(self.busy.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.form = None;
                                        this.notice = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn database_view(&self, view: &DatabaseView, cx: &mut Context<Self>) -> AnyElement {
        let db = &view.database;
        let start_id = db.id.clone();
        let stop_id = db.id.clone();
        let edit_id = db.id.clone();
        let project_id = db.project_id.clone();
        let delete_id = db.id.clone();
        let delete_name = db.config.name.clone();
        let volume = db.volume_name();
        let uri = db.connection_url();
        let active = view.state.is_active();
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
                .child(div().flex().gap_2()
                    .child(Button::new("settings").label("Settings").disabled(unavailable || active).on_click(cx.listener(move |this, _, window, cx| this.open_form(FormKind::Database { project: project_id.clone(), editing: Some(edit_id.clone()) }, window, cx))))
                    .child(if active {
                        Button::new("stop").label("Stop database").disabled(unavailable).on_click(cx.listener(move |this, _, _, cx| this.execute(Action::Stop(stop_id.clone()), "Stopping PostgreSQL…", cx)))
                    } else {
                        Button::new("start").primary().label("Start database").disabled(unavailable).on_click(cx.listener(move |this, _, _, cx| this.execute(Action::Start(start_id.clone(), !this.offline), "Starting PostgreSQL · downloading image if needed…", cx)))
                    })))
            .child(div().mx_8().mb_5().p_4().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).rounded_lg().flex().items_center().justify_between()
                .child(div().flex().gap_8().child(metric("HOST", "127.0.0.1".into())).child(metric("PORT", db.port.to_string())).child(metric("DATABASE", db.config.database.clone())).child(metric("USER", db.config.user.clone())))
                .child(Button::new("copy-url").ghost().small().label("Copy connection URL").on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(uri.clone())); this.notice = Some(("Connection URL copied, including the database password.".into(), false)); cx.notify();
                }))))
            .child(div().mx_8().mb_4().flex().items_center().gap_2()
                .child(Button::new("dump").small().label("Export backup").disabled(unavailable || view.state != ContainerState::Running).on_click(cx.listener(|this, _, _, cx| this.backup(cx))))
                .child(Button::new("restore").small().label("Restore backup").disabled(unavailable || view.state != ContainerState::Running).on_click(cx.listener(|this, _, window, cx| this.restore(window, cx))))
                .child(div().flex_1())
                .child(Button::new("remove-database").ghost().small().label("Remove database").disabled(unavailable || active).on_click(cx.listener(move |this, _, window, cx| this.open_form(FormKind::Confirm {
                    action: Action::DeleteDatabase(delete_id.clone()), name: delete_name.clone(), description: format!("Remove the stopped container and its Tusklet entry. The data volume {volume} will be kept in Docker. Type the database name to confirm.")
                }, window, cx)))))
            .child(div().debug_selector(|| "database-logs".into()).mx_8().mb_6().flex_1().min_h_0().flex().flex_col().border_1().border_color(rgb(LINE)).rounded_lg().overflow_hidden()
                .child(div().px_4().py_3().bg(rgb(PANEL)).border_b_1().border_color(rgb(LINE)).flex().items_center().justify_between()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("PostgreSQL logs"))
                    .child(div().flex().gap_2()
                        .child(Button::new("copy-logs").ghost().xsmall().label("Copy logs").on_click(cx.listener(|this, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(this.logs.clone())))))
                        .child(Button::new("follow").ghost().xsmall().label(if self.follow { "● Following" } else { "Resume follow" }).on_click(cx.listener(|this, _, _, cx| {
                            this.follow = !this.follow; this.request(Request::Select(this.selected.clone(), this.follow)); cx.notify();
                        })))))
                .child(div().id("logs").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.log_scroll).p_4().font_family(if cfg!(target_os = "windows") { "Consolas" } else { "monospace" }).text_xs().children(log_lines))
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
            .child(div().mt_4().text_xs().text_color(rgb(MUTED)).child("ANY POSTGRES VERSION    /    OFFLINE READY    /    BUILT FOR LOCAL WORK"))
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.form.is_some() {
            self.form_view(cx)
        } else if let Some(db) = self.selected_db() {
            self.database_view(&db, cx)
        } else {
            self.empty_view(cx)
        };
        div().size_full().flex().bg(rgb(BG)).text_color(rgb(TEXT)).text_sm()
            .child(self.sidebar(cx))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                .when(self.snapshot.docker_error.is_some(), |el| el.child(div().px_6().py_3().bg(rgb(0x3a_2d_23)).text_color(rgb(0xf0_c6_9b)).flex().items_center().gap_3()
                    .child(div().flex_1().child(format!("Docker unavailable. Start Docker Desktop or Docker Engine. {}", self.snapshot.docker_error.as_deref().unwrap_or("").chars().take(240).collect::<String>())))
                    .child(Button::new("retry").small().label("Retry").disabled(self.busy.is_some()).on_click(cx.listener(|this, _, _, _| this.request(Request::Refresh))))))
                .when_some(self.notice.clone(), |el, (message, error)| el.child(div().px_6().py_3().bg(rgb(if error { 0x3a_27_28 } else { 0x24_35_2c })).flex().items_center().gap_3()
                    .child(div().flex_1().text_color(rgb(if error { 0xf2_b6_af } else { GREEN })).child(message.chars().take(1200).collect::<String>()))
                    .child(Button::new("dismiss").ghost().xsmall().label("Dismiss").on_click(cx.listener(|this, _, _, cx| { this.notice = None; cx.notify(); })))))
                .when_some(self.busy.clone(), |el, message| el.child(div().px_6().py_3().bg(rgb(0x23_30_3b)).child(message)))
                .child(body))
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::*;
    use gpui::{TestAppContext, size};
    use gpui_component::Root;
    use tusklet::model::Database;

    fn created_database() -> Tusklet {
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
            ready: true,
            offline: false,
            follow: true,
            logs: String::new(),
            log_scroll: ScrollHandle::new(),
        }
    }

    #[gpui::test]
    fn created_database_keeps_heading_readable_and_logs_visible(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
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
