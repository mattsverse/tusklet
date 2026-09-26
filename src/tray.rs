use tusklet::{
    model::{ContainerState, Snapshot},
    service::Action,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Quit,
    Start(String),
    Stop(String),
}

#[derive(Clone, Copy)]
pub enum Activity<'a> {
    Loading,
    Ready,
    Busy(&'a Action),
    Editing,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Item(String, Option<Command>),
    Separator,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MenuModel {
    entries: Vec<Entry>,
}

impl MenuModel {
    pub fn new(snapshot: &Snapshot, activity: Activity<'_>, error: Option<&str>) -> Self {
        let active = snapshot
            .databases
            .iter()
            .filter(|db| db.state.is_active())
            .count();
        let heading = if snapshot.docker_error.is_some()
            || matches!(activity, Activity::Loading | Activity::Unavailable)
        {
            "Tusklet".into()
        } else {
            format!("Tusklet · {active} running")
        };
        let mut entries = vec![Entry::Item(heading, None)];
        let status = match activity {
            Activity::Loading => Some("Loading databases…"),
            Activity::Unavailable => Some("Workspace unavailable · open Tusklet for details"),
            _ if snapshot.docker_error.is_some() => Some("Docker unavailable"),
            Activity::Editing => Some("Finish editing in Tusklet to start or stop databases"),
            Activity::Busy(_) => Some("Operation in progress…"),
            Activity::Ready => None,
        };
        if let Some(status) = status {
            entries.push(Entry::Item(status.into(), None));
        }
        if let Some(error) = error {
            let message: String = error
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(100)
                .collect();
            entries.push(Entry::Item(
                format!("Error: {message} · Open Tusklet…"),
                Some(Command::Open),
            ));
        }
        let enabled = matches!(activity, Activity::Ready) && snapshot.docker_error.is_none();
        for project in &snapshot.projects {
            entries.push(Entry::Separator);
            entries.push(Entry::Item(project.name.clone(), None));
            let mut empty = true;
            for db in snapshot
                .databases
                .iter()
                .filter(|db| db.database.project_id == project.id)
            {
                empty = false;
                let id = &db.database.id;
                let pending = match &activity {
                    Activity::Busy(Action::Start(pending)) if pending == id => Some("Starting…"),
                    Activity::Busy(Action::Stop(pending)) if pending == id => Some("Stopping…"),
                    _ => None,
                };
                let active = db.state.is_active();
                let symbol = if active { "●" } else { "○" };
                let action = if active { "Stop" } else { "Start" };
                let detail = pending.map_or_else(
                    || {
                        if db.state == ContainerState::Unknown {
                            db.state.label().to_owned()
                        } else {
                            format!("{} · {action}", db.state.label())
                        }
                    },
                    str::to_owned,
                );
                let command = (enabled && db.state != ContainerState::Unknown).then(|| {
                    if active {
                        Command::Stop(id.clone())
                    } else {
                        Command::Start(id.clone())
                    }
                });
                entries.push(Entry::Item(
                    format!("{symbol} {} — {detail}", db.database.config.name),
                    command,
                ));
            }
            if empty {
                entries.push(Entry::Item("No databases".into(), None));
            }
        }
        if snapshot.projects.is_empty() && matches!(activity, Activity::Ready) {
            entries.push(Entry::Item(
                "No databases yet · create one in Tusklet".into(),
                None,
            ));
        }
        entries.extend([
            Entry::Separator,
            Entry::Item("Open Tusklet".into(), Some(Command::Open)),
            Entry::Item("Quit Tusklet".into(), Some(Command::Quit)),
        ]);
        Self { entries }
    }

    /// Revalidate against current state, since native menu events may be queued.
    pub fn allows(&self, command: &Command) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry, Entry::Item(_, Some(candidate)) if candidate == command))
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{Command, Entry, MenuModel};
    use anyhow::Result;
    use std::collections::HashMap;
    use tray_icon::{
        Icon, TrayIcon, TrayIconBuilder,
        menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    };

    pub struct Tray {
        icon: TrayIcon,
        model: MenuModel,
        commands: HashMap<MenuId, Command>,
    }

    impl Tray {
        /// Called on GPUI's main thread after the native event loop has started.
        pub fn new(model: MenuModel) -> Result<Self> {
            let image = image::load_from_memory(include_bytes!("../assets/tray.png"))?.into_rgba8();
            let (width, height) = image.dimensions();
            let icon = Icon::from_rgba(image.into_raw(), width, height)?;
            let (menu, commands) = build_menu(&model)?;
            let icon = TrayIconBuilder::new()
                .with_icon(icon)
                .with_icon_as_template(true)
                .with_tooltip("Tusklet — local PostgreSQL databases")
                .with_menu(Box::new(menu))
                .build()?;
            Ok(Self {
                icon,
                model,
                commands,
            })
        }

        pub fn update(&mut self, model: MenuModel) -> Result<()> {
            // Log refreshes and unrelated UI changes must not replace an open menu.
            if self.model != model {
                let (menu, commands) = build_menu(&model)?;
                self.icon.set_menu(Some(Box::new(menu)));
                self.commands = commands;
                self.model = model;
            }
            Ok(())
        }

        pub fn take_commands(&self) -> Vec<Command> {
            MenuEvent::receiver()
                .try_iter()
                .filter_map(|event| self.commands.get(&event.id).cloned())
                .collect()
        }
    }

    fn build_menu(model: &MenuModel) -> Result<(Menu, HashMap<MenuId, Command>)> {
        let menu = Menu::new();
        let mut commands = HashMap::new();
        for entry in &model.entries {
            match entry {
                Entry::Separator => menu.append(&PredefinedMenuItem::separator())?,
                Entry::Item(label, command) => {
                    // Keep IDs stable across status refreshes so an already queued
                    // click is not lost merely because the menu was refreshed.
                    let item = match command {
                        Some(command) => {
                            let id = match command {
                                Command::Open => "tusklet.open".into(),
                                Command::Quit => "tusklet.quit".into(),
                                Command::Start(id) => format!("tusklet.start.{id}"),
                                Command::Stop(id) => format!("tusklet.stop.{id}"),
                            };
                            MenuItem::with_id(id, label, true, None)
                        }
                        None => MenuItem::new(label, false, None),
                    };
                    if let Some(command) = command {
                        commands.insert(item.id().clone(), command.clone());
                    }
                    menu.append(&item)?;
                }
            }
        }
        Ok((menu, commands))
    }
}

#[cfg(target_os = "macos")]
pub use native::Tray;

#[cfg(test)]
mod tests {
    use super::*;
    use tusklet::model::{Database, DatabaseConfig, DatabaseView, Project};

    #[cfg(target_os = "macos")]
    #[test]
    fn embedded_tray_icon_contains_visible_artwork_and_transparent_background() {
        let icon = image::load_from_memory(include_bytes!("../assets/tray.png"))
            .unwrap()
            .into_rgba8();
        let visible_pixels = icon.pixels().filter(|pixel| pixel[3] > 128).count();
        let transparent_pixels = icon.pixels().filter(|pixel| pixel[3] == 0).count();
        assert!(
            visible_pixels > 100,
            "the menu bar icon contains no visible artwork"
        );
        assert!(
            transparent_pixels > 100,
            "the template icon needs a transparent background"
        );
    }

    fn workspace() -> Snapshot {
        Snapshot {
            projects: vec![
                Project {
                    id: "first".into(),
                    name: "Website".into(),
                },
                Project {
                    id: "second".into(),
                    name: "Side project".into(),
                },
            ],
            databases: [
                ("dev", "first", ContainerState::Running),
                ("test", "first", ContainerState::Stopped),
                ("new", "second", ContainerState::NotCreated),
                ("unhealthy", "second", ContainerState::Unhealthy),
                ("starting", "second", ContainerState::Starting),
                ("unknown", "second", ContainerState::Unknown),
            ]
            .into_iter()
            .map(|(id, project, state)| DatabaseView {
                database: Database {
                    id: id.into(),
                    project_id: project.into(),
                    config: DatabaseConfig {
                        name: id.into(),
                        ..Default::default()
                    },
                    password: "must-not-appear-in-menu".into(),
                    port: 15432,
                },
                state,
            })
            .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn lists_projects_and_all_databases_with_correct_actions() {
        let model = MenuModel::new(&workspace(), Activity::Ready, None);
        for id in ["dev", "unhealthy", "starting"] {
            assert!(model.allows(&Command::Stop(id.into())));
            assert!(!model.allows(&Command::Start(id.into())));
        }
        for id in ["test", "new"] {
            assert!(model.allows(&Command::Start(id.into())));
            assert!(!model.allows(&Command::Stop(id.into())));
        }
        assert!(!model.allows(&Command::Start("unknown".into())));
        assert!(!model.allows(&Command::Stop("unknown".into())));
        let labels: Vec<_> = model
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Item(label, _) => Some(label.as_str()),
                Entry::Separator => None,
            })
            .collect();
        assert_eq!(labels[0], "Tusklet · 3 running");
        assert_eq!(labels[1], "Website");
        assert!(labels[2].contains("dev"));
        assert!(labels[3].contains("test"));
        assert_eq!(labels[4], "Side project");
        assert!(!format!("{model:?}").contains("must-not-appear-in-menu"));
    }

    #[test]
    fn unavailable_and_busy_workspaces_keep_navigation_but_disable_database_actions() {
        let snapshot = workspace();
        let starting = Action::Start("test".into());
        let stopping = Action::Stop("dev".into());
        for activity in [
            Activity::Loading,
            Activity::Unavailable,
            Activity::Editing,
            Activity::Busy(&starting),
            Activity::Busy(&stopping),
        ] {
            let model = MenuModel::new(&snapshot, activity, None);
            assert!(model.allows(&Command::Open));
            assert!(model.allows(&Command::Quit));
            assert!(!model.allows(&Command::Start("test".into())));
            assert!(!model.allows(&Command::Stop("dev".into())));
        }
        let model = MenuModel::new(&snapshot, Activity::Busy(&starting), None);
        assert!(
            model
                .entries
                .contains(&Entry::Item("○ test — Starting…".into(), None))
        );
        let model = MenuModel::new(&snapshot, Activity::Busy(&stopping), None);
        assert!(
            model
                .entries
                .contains(&Entry::Item("● dev — Stopping…".into(), None))
        );
    }

    #[test]
    fn docker_failure_disables_actions_and_operation_errors_link_to_the_window() {
        let mut snapshot = workspace();
        snapshot.docker_error = Some("Docker is offline".into());
        let model = MenuModel::new(&snapshot, Activity::Ready, Some("Port 15432 is in use"));
        assert!(!model.allows(&Command::Start("test".into())));
        assert!(!model.allows(&Command::Stop("dev".into())));
        assert!(model.entries.iter().any(|entry| matches!(entry,
            Entry::Item(label, Some(Command::Open)) if label.contains("Port 15432 is in use"))));
        snapshot.docker_error = None;
        assert!(
            MenuModel::new(&snapshot, Activity::Ready, None).allows(&Command::Start("test".into()))
        );
    }

    #[test]
    fn log_updates_do_not_replace_the_native_menu() {
        let mut snapshot = workspace();
        let model = MenuModel::new(&snapshot, Activity::Ready, None);
        snapshot.logs = "new log lines".into();
        assert_eq!(model, MenuModel::new(&snapshot, Activity::Ready, None));
    }

    #[test]
    fn empty_workspace_explains_how_to_get_started() {
        let model = MenuModel::new(&Snapshot::default(), Activity::Ready, None);
        assert!(model.entries.iter().any(|entry| matches!(entry,
            Entry::Item(label, None) if label.contains("create one in Tusklet"))));
        assert!(model.allows(&Command::Open));
    }
}
