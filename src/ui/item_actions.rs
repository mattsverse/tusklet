use super::{FormKind, Tusklet};
use gpui::{
    AnyElement, Context, DismissEvent, Div, Entity, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, Pixels, Point, Subscription, Window, anchored, deferred, div, prelude::*, px,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use std::{cell::Cell, rc::Rc};
use tusklet::{model::ContainerState, service::Action};

pub(super) struct OpenMenu {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    pub return_focus: Option<FocusHandle>,
    select_first: usize,
    _subscription: Subscription,
}

#[derive(Clone)]
pub(super) enum ItemTarget {
    Project(String),
    Database(String),
}

#[derive(Clone, Copy)]
pub(super) enum ItemCommand {
    NewDatabase,
    Settings,
    Start,
    Stop,
    Remove,
}

impl Tusklet {
    pub(super) fn open_item_menu(
        &mut self,
        target: ItemTarget,
        position: Point<Pixels>,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.form.is_some() || !self.item_command_enabled(&target, ItemCommand::Settings) {
            return;
        }
        let first_enabled = match &target {
            ItemTarget::Database(_)
                if !self.item_command_enabled(&target, ItemCommand::Start)
                    && !self.item_command_enabled(&target, ItemCommand::Stop) =>
            {
                2
            }
            _ => 1,
        };
        self.close_item_menu(window, cx);
        let return_focus = window.focused(cx);
        let builder = self.item_menu(target, cx);
        let menu = PopupMenu::build(window, cx, builder);
        let subscription =
            cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
                this.close_item_menu(window, cx);
            });
        menu.focus_handle(cx).focus(window);
        self.menu = Some(OpenMenu {
            menu,
            position,
            return_focus,
            select_first: if keyboard { first_enabled } else { 0 },
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn close_item_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            // A menu action may already have focused a new form. Preserve that focus.
            if menu.menu.focus_handle(cx).contains_focused(window, cx) {
                if let Some(focus) = menu.return_focus {
                    focus.focus(window);
                } else if let Some(keyboard) = &self.keyboard {
                    keyboard.root.focus(window);
                }
            }
            cx.notify();
        }
    }

    pub(super) fn menu_view(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_mut()?;
        let select_first = std::mem::take(&mut menu.select_first);
        let focus = menu.menu.focus_handle(cx);
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .on_children_prepainted(move |_, window, cx| {
                                if select_first > 0 {
                                    let focus = focus.clone();
                                    window.defer(cx, move |window, cx| {
                                        if focus.is_focused(window) {
                                            // PopupMenu exposes selection through its registered actions.
                                            // Dispatch after its focus node has entered the rendered tree.
                                            if let Ok(action) =
                                                cx.build_action("ui::SelectDown", None)
                                            {
                                                for _ in 0..select_first {
                                                    focus.dispatch_action(
                                                        action.as_ref(),
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            }
                                        }
                                    });
                                }
                            })
                            .child(menu.menu.clone()),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    pub(super) fn item_row(target: ItemTarget, content: Div, cx: &mut Context<Self>) -> Div {
        let position = Rc::new(Cell::new(Point::default()));
        let key_position = position.clone();
        let key_target = target.clone();
        content
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    position.set(bounds.bottom_left());
                }
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if (key.key == "f10"
                    && key.modifiers.shift
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.platform)
                    || (key.key == "menu" && !key.modifiers.modified())
                {
                    this.open_item_menu(key_target.clone(), key_position.get(), true, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.open_item_menu(target.clone(), event.position, false, window, cx);
                    cx.stop_propagation();
                }),
            )
    }

    pub(super) fn item_command_enabled(&self, target: &ItemTarget, command: ItemCommand) -> bool {
        if !self.ready || self.worker.is_none() || self.busy.is_some() {
            return false;
        }
        match target {
            ItemTarget::Project(id) => {
                self.snapshot
                    .projects
                    .iter()
                    .any(|project| &project.id == id)
                    && match command {
                        ItemCommand::NewDatabase | ItemCommand::Settings => true,
                        ItemCommand::Remove => !self
                            .snapshot
                            .databases
                            .iter()
                            .any(|view| &view.database.project_id == id),
                        ItemCommand::Start | ItemCommand::Stop => false,
                    }
            }
            ItemTarget::Database(id) => {
                let Some(view) = self
                    .snapshot
                    .databases
                    .iter()
                    .find(|view| &view.database.id == id)
                else {
                    return false;
                };
                match command {
                    ItemCommand::Settings => true,
                    ItemCommand::NewDatabase => false,
                    ItemCommand::Start | ItemCommand::Stop | ItemCommand::Remove => {
                        self.snapshot.runtime_error.is_none()
                            && view.state != ContainerState::Unknown
                            && match command {
                                ItemCommand::Stop => view.state.is_active(),
                                _ => !view.state.is_active(),
                            }
                    }
                }
            }
        }
    }

    fn item_menu(
        &self,
        target: ItemTarget,
        cx: &Context<Self>,
    ) -> impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + use<> {
        let commands = match &target {
            ItemTarget::Project(_) => [
                ("New database", ItemCommand::NewDatabase),
                ("Project settings", ItemCommand::Settings),
                ("Remove project…", ItemCommand::Remove),
            ],
            ItemTarget::Database(id) => [
                if self
                    .snapshot
                    .databases
                    .iter()
                    .any(|view| &view.database.id == id && view.state.is_active())
                {
                    ("Stop database", ItemCommand::Stop)
                } else {
                    ("Start database", ItemCommand::Start)
                },
                ("Database settings", ItemCommand::Settings),
                ("Remove database…", ItemCommand::Remove),
            ],
        };
        let commands = commands.map(|(label, command)| {
            (
                label,
                command,
                self.form.is_none() && self.item_command_enabled(&target, command),
            )
        });
        let reason = self.removal_block_reason(&target);
        let entity = cx.weak_entity();
        move |mut menu, _, _| {
            menu = menu.min_w(px(220.)).max_w(px(320.));
            for (label, command, enabled) in commands {
                if matches!(command, ItemCommand::Remove) {
                    menu = menu.separator();
                }
                let target = target.clone();
                let entity = entity.clone();
                menu = menu.item(PopupMenuItem::new(label).disabled(!enabled).on_click(
                    move |_, window, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                // Recheck the current state, even if the open menu is stale.
                                if this.form.is_none() {
                                    this.run_item_command(&target, command, window, cx);
                                }
                            });
                        }
                    },
                ));
            }
            if let Some(reason) = reason {
                menu = menu.label(reason);
            }
            menu
        }
    }

    pub(super) fn removal_block_reason(&self, target: &ItemTarget) -> Option<&'static str> {
        match target {
            ItemTarget::Project(id) => self
                .snapshot
                .databases
                .iter()
                .any(|view| &view.database.project_id == id)
                .then_some("Remove this project's databases first."),
            ItemTarget::Database(id) => {
                let view = self
                    .snapshot
                    .databases
                    .iter()
                    .find(|view| &view.database.id == id)?;
                if self.snapshot.runtime_error.is_some() || view.state == ContainerState::Unknown {
                    Some("Connect to the container runtime to remove this database.")
                } else if view.state.is_active() {
                    Some("Stop this database before removing it.")
                } else {
                    None
                }
            }
        }
    }

    pub(super) fn run_item_command(
        &mut self,
        target: &ItemTarget,
        command: ItemCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.item_command_enabled(target, command) {
            return;
        }
        match (target, command) {
            (ItemTarget::Project(id), ItemCommand::NewDatabase) => self.open_form(
                FormKind::Database {
                    project: id.clone(),
                    editing: None,
                },
                window,
                cx,
            ),
            (ItemTarget::Project(id), ItemCommand::Settings) => {
                self.open_form(FormKind::Project(Some(id.clone())), window, cx);
            }
            (ItemTarget::Project(id), ItemCommand::Remove) => {
                let project = self
                    .snapshot
                    .projects
                    .iter()
                    .find(|project| &project.id == id)
                    .unwrap();
                self.open_form(
                    FormKind::Confirm {
                        action: Action::DeleteProject(id.clone()),
                        name: project.name.clone(),
                        description: format!(
                            "Remove the empty project “{}” from Tusklet. Type its name to confirm.",
                            project.name
                        ),
                    },
                    window,
                    cx,
                );
            }
            (ItemTarget::Database(id), ItemCommand::Settings) => {
                let view = self
                    .snapshot
                    .databases
                    .iter()
                    .find(|view| &view.database.id == id)
                    .unwrap();
                self.open_form(
                    FormKind::Database {
                        project: view.database.project_id.clone(),
                        editing: Some(id.clone()),
                    },
                    window,
                    cx,
                );
            }
            (ItemTarget::Database(id), ItemCommand::Start) => self.execute(
                Action::Start(id.clone()),
                "Starting PostgreSQL · downloading image if needed…",
                cx,
            ),
            (ItemTarget::Database(id), ItemCommand::Stop) => {
                self.execute(Action::Stop(id.clone()), "Stopping PostgreSQL…", cx);
            }
            (ItemTarget::Database(id), ItemCommand::Remove) => {
                let view = self
                    .snapshot
                    .databases
                    .iter()
                    .find(|view| &view.database.id == id)
                    .unwrap();
                self.open_form(FormKind::Confirm {
                    action: Action::DeleteDatabase { id: id.clone(), remove_volume: false },
                    name: view.database.config.name.clone(),
                    description: format!("Remove “{}”, its stopped container, and its Tusklet entry. Type the database name to confirm.", view.database.config.name),
                }, window, cx);
            }
            _ => {}
        }
    }
}
