use super::{FormKind, GREEN, ItemCommand, ItemTarget, LINE, MUTED, PANEL, Tusklet};
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EntityInputHandler, FocusHandle, Focusable,
    KeyBinding, MouseButton, ScrollHandle, Subscription, Window, actions, div, prelude::*, px, rgb,
    rgba,
};
use gpui_component::input::{self, Input, InputEvent, InputState};
use tusklet::{model::ContainerState, service::Request};

actions!(tusklet_palette, [TogglePalette]);

#[cfg(all(test, feature = "ui-tests"))]
#[path = "palette_tests.rs"]
mod tests;

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "secondary-k",
        TogglePalette,
        Some("Tusklet"),
    )]);
}

#[derive(Clone)]
enum Command {
    NewProject,
    Item(ItemTarget, ItemCommand),
    OpenProject(String),
    OpenDatabase(String),
    CopyUrl(String),
    Export(String),
    Restore(String),
    CopyLogs(String),
    SetFollow(String, bool),
    Refresh,
}

#[derive(Clone)]
struct Entry {
    label: String,
    detail: String,
    keywords: &'static str,
    command: Command,
}

impl Entry {
    fn new(label: &str, detail: &str, keywords: &'static str, command: Command) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            keywords,
            command,
        }
    }

    fn matches(&self, query: &str) -> bool {
        let text = format!("{} {} {}", self.label, self.detail, self.keywords).to_lowercase();
        query.split_whitespace().all(|word| text.contains(word))
    }

    fn database_actions(id: &str, detail: &str) -> [Self; 8] {
        let target = ItemTarget::Database(id.into());
        [
            Self::new(
                "Open database",
                detail,
                "switch show navigate",
                Command::OpenDatabase(id.into()),
            ),
            Self::new(
                "Start database",
                detail,
                "run launch",
                Command::Item(target.clone(), ItemCommand::Start),
            ),
            Self::new(
                "Stop database",
                detail,
                "shutdown",
                Command::Item(target.clone(), ItemCommand::Stop),
            ),
            Self::new(
                "Database settings",
                detail,
                "edit configure rename",
                Command::Item(target.clone(), ItemCommand::Settings),
            ),
            Self::new(
                "Copy connection URL",
                detail,
                "database postgres connect clipboard",
                Command::CopyUrl(id.into()),
            ),
            Self::new(
                "Export backup…",
                detail,
                "database dump save",
                Command::Export(id.into()),
            ),
            Self::new(
                "Restore backup…",
                detail,
                "database import load",
                Command::Restore(id.into()),
            ),
            Self::new(
                "Remove database…",
                detail,
                "delete",
                Command::Item(target, ItemCommand::Remove),
            ),
        ]
    }
}

pub(super) struct CommandPalette {
    input: Entity<InputState>,
    entries: Vec<Entry>,
    matches: Vec<usize>,
    selected: usize,
    scroll: ScrollHandle,
    return_focus: Option<FocusHandle>,
    _subscription: Subscription,
}

impl Tusklet {
    fn palette_command_enabled(&self, command: &Command) -> bool {
        if self.form.is_some() {
            return false;
        }
        match command {
            Command::NewProject => self.ready && self.worker.is_some() && self.busy.is_none(),
            Command::Item(target, command) => self.item_command_enabled(target, *command),
            Command::OpenProject(id) => {
                self.ready
                    && self.worker.is_some()
                    && self.busy.is_none()
                    && self
                        .snapshot
                        .projects
                        .iter()
                        .any(|project| &project.id == id)
            }
            Command::OpenDatabase(id) => {
                self.ready
                    && self.worker.is_some()
                    && self
                        .snapshot
                        .databases
                        .iter()
                        .any(|view| &view.database.id == id)
            }
            Command::CopyUrl(id) => self
                .snapshot
                .databases
                .iter()
                .any(|view| &view.database.id == id),
            Command::Export(id) | Command::Restore(id) => {
                self.ready
                    && self.worker.is_some()
                    && self.busy.is_none()
                    && self.snapshot.runtime_error.is_none()
                    && self.snapshot.databases.iter().any(|view| {
                        &view.database.id == id && view.state == ContainerState::Running
                    })
            }
            Command::CopyLogs(id) => self.selected.as_ref() == Some(id) && !self.logs.is_empty(),
            Command::SetFollow(id, follow) => {
                self.selected.as_ref() == Some(id)
                    && self.worker.is_some()
                    && self.follow != *follow
            }
            Command::Refresh => self.worker.is_some() && self.busy.is_none(),
        }
    }

    fn palette_entries(&self) -> Vec<Entry> {
        let mut entries = vec![Entry::new(
            "New project",
            "Workspace",
            "create add",
            Command::NewProject,
        )];
        // Keep the current project/database first without changing workspace order.
        let mut projects: Vec<_> = self.snapshot.projects.iter().collect();
        projects.sort_by_key(|project| self.project.as_ref() != Some(&project.id));
        for project in projects {
            let target = ItemTarget::Project(project.id.clone());
            entries.push(Entry::new(
                "New database",
                &project.name,
                "create add postgres",
                Command::Item(target, ItemCommand::NewDatabase),
            ));
        }
        let mut databases: Vec<_> = self.snapshot.databases.iter().collect();
        databases.sort_by_key(|view| self.selected.as_ref() != Some(&view.database.id));
        for view in databases {
            let db = &view.database;
            let project = self
                .snapshot
                .projects
                .iter()
                .find(|project| project.id == db.project_id);
            let detail = format!(
                "{} / {}",
                project.map_or("Project", |project| project.name.as_str()),
                db.config.name
            );
            entries.extend(Entry::database_actions(&db.id, &detail));
        }
        for project in &self.snapshot.projects {
            let target = ItemTarget::Project(project.id.clone());
            entries.extend([
                Entry::new(
                    "Open project",
                    &project.name,
                    "switch show navigate",
                    Command::OpenProject(project.id.clone()),
                ),
                Entry::new(
                    "Project settings",
                    &project.name,
                    "edit configure rename",
                    Command::Item(target.clone(), ItemCommand::Settings),
                ),
                Entry::new(
                    "Remove project…",
                    &project.name,
                    "delete",
                    Command::Item(target, ItemCommand::Remove),
                ),
            ]);
        }
        if let Some(view) = self.selected_db() {
            entries.extend([
                Entry::new(
                    "Copy logs",
                    &view.database.config.name,
                    "clipboard",
                    Command::CopyLogs(view.database.id.clone()),
                ),
                Entry::new(
                    if self.follow {
                        "Pause log following"
                    } else {
                        "Resume log following"
                    },
                    &view.database.config.name,
                    "logs tail",
                    Command::SetFollow(view.database.id.clone(), !self.follow),
                ),
            ]);
        }
        entries.push(Entry::new(
            "Refresh workspace",
            "Docker status and databases",
            "retry reload",
            Command::Refresh,
        ));
        entries.retain(|entry| self.palette_command_enabled(&entry.command));
        entries
    }

    pub(super) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        // Keep unsaved forms intact; their Escape/Enter shortcuts retain ownership.
        if self.form.is_some() {
            return;
        }
        self.close_item_menu(window, cx);
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search actions, projects, databases…")
        });
        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().to_lowercase();
                // Refresh on edits, but keep the displayed commands stable between
                // keystrokes so a background update cannot change what Enter runs.
                let entries = this.palette_entries();
                if let Some(palette) = &mut this.palette {
                    palette.entries = entries;
                    palette.matches = palette
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.matches(&query))
                        .map(|(index, _)| index)
                        .collect();
                    palette.selected = 0;
                    palette.scroll.set_offset(gpui::point(px(0.), px(0.)));
                }
                cx.notify();
            }
        });
        let entries = self.palette_entries();
        self.palette = Some(CommandPalette {
            input: input.clone(),
            matches: (0..entries.len()).collect(),
            entries,
            selected: 0,
            scroll: ScrollHandle::new(),
            return_focus: window.focused(cx),
            _subscription: subscription,
        });
        self.restore_focus = None;
        input.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.take() {
            if let Some(focus) = palette.return_focus {
                focus.focus(window);
            } else if let Some(keyboard) = &self.keyboard {
                keyboard.root.focus(window);
            }
            cx.notify();
        }
    }

    pub(super) fn move_palette_selection(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = &mut self.palette else {
            return;
        };
        let len = palette.matches.len();
        if len > 0 {
            palette.selected = if backwards {
                (palette.selected + len - 1) % len
            } else {
                (palette.selected + 1) % len
            };
            palette.scroll.scroll_to_item(palette.selected);
        }
        palette.input.focus_handle(cx).focus(window);
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn palette_composing(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.palette.as_ref().is_some_and(|palette| {
            palette.input.clone().update(cx, |input, cx| {
                if input.marked_text_range(window, cx).is_some() {
                    input.unmark_text(window, cx);
                    cx.notify();
                    true
                } else {
                    false
                }
            })
        })
    }

    fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.palette.as_ref().and_then(|palette| {
            palette
                .matches
                .get(palette.selected)
                .map(|index| palette.entries[*index].command.clone())
        });
        if let Some(command) = command {
            self.run_palette_command(&command, window, cx);
        }
    }

    fn run_palette_command(
        &mut self,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A snapshot or tray action may have changed availability since opening.
        if !self.palette_command_enabled(command) {
            return;
        }
        self.close_palette(window, cx);
        match command {
            Command::NewProject => self.open_form(FormKind::Project(None), window, cx),
            Command::Item(target, command) => self.run_item_command(target, *command, window, cx),
            Command::OpenProject(id) => {
                self.project = Some(id.clone());
                let next = self
                    .snapshot
                    .databases
                    .iter()
                    .find(|view| &view.database.project_id == id)
                    .map(|view| view.database.id.clone());
                self.select(next, cx);
            }
            Command::OpenDatabase(id) => self.select(Some(id.clone()), cx),
            Command::CopyUrl(id) => {
                let view = self
                    .snapshot
                    .databases
                    .iter()
                    .find(|view| &view.database.id == id)
                    .unwrap();
                cx.write_to_clipboard(ClipboardItem::new_string(view.database.connection_url()));
                self.notice = Some((
                    "Connection URL copied, including the database password.".into(),
                    false,
                ));
            }
            Command::Export(id) | Command::Restore(id) => {
                self.select(Some(id.clone()), cx);
                if matches!(command, Command::Export(_)) {
                    self.backup(cx);
                } else {
                    self.restore(window, cx);
                }
            }
            Command::CopyLogs(_) => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.logs.clone()));
            }
            Command::SetFollow(_, follow) => {
                self.follow = *follow;
                if self.follow {
                    self.log_scroll.scroll_to_bottom();
                }
                self.request(Request::Select(
                    self.selected.clone(),
                    self.follow && self.window_visible,
                ));
            }
            Command::Refresh => self.request(Request::Refresh),
        }
        cx.notify();
    }

    pub(super) fn palette_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let rows = palette.matches.iter().enumerate().map(|(row, index)| {
            let entry = &palette.entries[*index];
            let command = entry.command.clone();
            let enabled = self.palette_command_enabled(&command);
            div()
                .id(("palette-result", *index))
                .debug_selector(move || format!("palette-result-{row}"))
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .when(row == palette.selected, |row| row.bg(rgb(0x29_3b_33)))
                .when(enabled, |row| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(rgb(0x29_3b_33)))
                })
                .when(!enabled, |row| row.opacity(0.45))
                .child(div().text_color(rgb(GREEN)).child(entry.label.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(if enabled {
                            entry.detail.clone()
                        } else {
                            format!("{} · Unavailable now", entry.detail)
                        }),
                )
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_palette_command(&command, window, cx);
                }))
        });
        Some(div().id("palette-overlay").absolute().inset_0().occlude()
            .bg(rgba(0x00_00_00_88)).flex().justify_center().items_start().pt_12().px_6()
            .on_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                if event.button == MouseButton::Left { this.close_palette(window, cx); }
                cx.stop_propagation();
            }))
            .child(div().id("command-palette").debug_selector(|| "command-palette".into())
                .occlude().w(px(600.)).max_w_full().flex().flex_col().bg(rgb(PANEL))
                .border_1().border_color(rgb(LINE)).rounded_xl().shadow_lg().overflow_hidden()
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .capture_action(cx.listener(|this, _: &input::MoveUp, window, cx| this.move_palette_selection(true, window, cx)))
                .capture_action(cx.listener(|this, _: &input::MoveDown, window, cx| this.move_palette_selection(false, window, cx)))
                .capture_action(cx.listener(|this, _: &input::Enter, window, cx| {
                    if !this.palette_composing(window, cx) { this.confirm_palette(window, cx); }
                    window.prevent_default(); cx.stop_propagation();
                }))
                .capture_action(cx.listener(|this, _: &input::Escape, window, cx| {
                    if !this.palette_composing(window, cx) { this.close_palette(window, cx); }
                    window.prevent_default(); cx.stop_propagation();
                }))
                .child(div().p_4().border_b_1().border_color(rgb(LINE))
                    .child(div().mb_3().text_xs().text_color(rgb(MUTED)).child("COMMANDS"))
                    .child(Input::new(&palette.input)))
                .child(div().id("palette-results").debug_selector(|| "palette-results".into())
                    .overflow_y_scroll().track_scroll(&palette.scroll)
                    .max_h((window.viewport_size().height - px(240.)).min(px(390.)).max(px(80.)))
                    .p_2().children(rows)
                    .when(palette.matches.is_empty(), |list| list.child(div().p_6().text_color(rgb(MUTED)).child("No matching actions available. Try a project or database name."))))
                .child(div().px_4().py_3().border_t_1().border_color(rgb(LINE)).text_xs().text_color(rgb(MUTED))
                    .child("↑ ↓  Navigate     Enter  Run     Esc  Close")))
            .into_any_element())
    }
}
