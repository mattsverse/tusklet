use gpui::{
    App, Bounds, Div, ElementId, EntityInputHandler, FocusHandle, Focusable, IntoElement,
    KeyBinding, Pixels, RenderOnce, ScrollHandle, Size, WeakFocusHandle, Window, actions,
    prelude::*, px,
};

actions!(
    tusklet_keyboard,
    [
        NextControl,
        PreviousControl,
        LogUp,
        LogDown,
        LogPageUp,
        LogPageDown,
        LogHome,
        LogEnd
    ]
);

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", NextControl, Some("Tusklet")),
        KeyBinding::new("shift-tab", PreviousControl, Some("Tusklet")),
        KeyBinding::new("escape", gpui_component::input::Escape, Some("TuskletForm")),
        KeyBinding::new(
            "secondary-enter",
            gpui_component::input::Enter { secondary: true },
            Some("TuskletForm"),
        ),
        KeyBinding::new("up", LogUp, Some("TuskletLogs")),
        KeyBinding::new("down", LogDown, Some("TuskletLogs")),
        KeyBinding::new("pageup", LogPageUp, Some("TuskletLogs")),
        KeyBinding::new("pagedown", LogPageDown, Some("TuskletLogs")),
        KeyBinding::new("home", LogHome, Some("TuskletLogs")),
        KeyBinding::new("end", LogEnd, Some("TuskletLogs")),
        KeyBinding::new("secondary-up", LogHome, Some("TuskletLogs")),
        KeyBinding::new("secondary-down", LogEnd, Some("TuskletLogs")),
    ]);
}

pub(super) struct Keyboard {
    pub root: FocusHandle,
    pub body: FocusHandle,
    pub logs: FocusHandle,
    pressed_activation: Option<(String, Option<WeakFocusHandle>)>,
}

impl Keyboard {
    pub fn new(cx: &App) -> Self {
        Self {
            root: cx.focus_handle(),
            body: cx.focus_handle(),
            logs: cx.focus_handle().tab_stop(true),
            pressed_activation: None,
        }
    }
}

/// A non-tabbable focus scope that reveals its controls without altering their layout.
/// Only focus changes and viewport resizes reveal content, so mouse scrolling is not
/// continuously pulled back to whichever control happens to retain focus.
#[derive(IntoElement)]
pub(super) struct RevealOnFocus {
    id: ElementId,
    content: Div,
    scroll: ScrollHandle,
}

pub(super) fn reveal_on_focus(
    id: impl Into<ElementId>,
    scroll: &ScrollHandle,
    content: Div,
) -> RevealOnFocus {
    RevealOnFocus {
        id: id.into(),
        content,
        scroll: scroll.clone(),
    }
}

struct RevealState {
    scope: FocusHandle,
    last_focus: Option<WeakFocusHandle>,
    viewport: Size<Pixels>,
}

impl RenderOnce for RevealOnFocus {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| RevealState {
            scope: cx.focus_handle(),
            last_focus: None,
            viewport: Size::default(),
        });
        let scope = state.read(cx).scope.clone();
        let scroll = self.scroll;
        self.content
            .on_children_prepainted(move |bounds, window, cx| {
                let Some(bounds) = bounds.into_iter().reduce(|a, b| a.union(&b)) else {
                    return;
                };
                let state = state.clone();
                let scroll = scroll.clone();
                // The new dispatch tree is installed after painting. Check it then,
                // including for controls that have just entered the form.
                window.defer(cx, move |window, cx| {
                    state.update(cx, |state, cx| {
                        let focus = window.focused(cx);
                        let within = state.scope.contains_focused(window, cx);
                        let viewport = scroll.bounds();
                        let changed = focus.as_ref().map(FocusHandle::downgrade)
                            != state.last_focus
                            || viewport.size != state.viewport;
                        state.last_focus = focus.as_ref().map(FocusHandle::downgrade);
                        state.viewport = viewport.size;
                        if within && changed {
                            reveal(bounds, &scroll, window);
                        }
                    });
                });
            })
            .id(self.id)
            .track_focus(&scope)
    }
}

fn reveal(bounds: Bounds<Pixels>, scroll: &ScrollHandle, window: &mut Window) {
    let viewport = scroll.bounds();
    let margin = px(4.);
    let delta =
        if bounds.top() < viewport.top() + margin || bounds.size.height > viewport.size.height {
            viewport.top() + margin - bounds.top()
        } else if bounds.bottom() > viewport.bottom() - margin {
            viewport.bottom() - margin - bounds.bottom()
        } else {
            return;
        };
    let mut offset = scroll.offset();
    offset.y = (offset.y + delta).clamp(-scroll.max_offset().height, px(0.));
    if offset != scroll.offset() {
        scroll.set_offset(offset);
        window.refresh();
    }
}

impl super::Tusklet {
    pub(super) fn record_activation(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &Window,
        cx: &gpui::Context<Self>,
    ) {
        if !event.is_held
            && !event.keystroke.modifiers.modified()
            && matches!(event.keystroke.key.as_str(), "enter" | "space")
            && let Some(keyboard) = &mut self.keyboard
        {
            keyboard.pressed_activation = Some((
                event.keystroke.key.clone(),
                window.focused(cx).as_ref().map(FocusHandle::downgrade),
            ));
        }
    }

    pub(super) fn guard_activation_release(
        &mut self,
        event: &gpui::KeyUpEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if !matches!(event.keystroke.key.as_str(), "enter" | "space") {
            return;
        }
        let origin = self
            .keyboard
            .as_mut()
            .and_then(|keyboard| keyboard.pressed_activation.take());
        let same_control = origin.is_some_and(|(key, focus)| {
            key == event.keystroke.key
                && focus == window.focused(cx).as_ref().map(FocusHandle::downgrade)
        });
        if !same_control {
            // Handled key-down actions do not reach the raw key listener. Their
            // release must not click a new button (e.g. Cancel in read-only settings).
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    pub(super) fn finish_composition(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let field = self.form.as_ref().and_then(|form| {
            form.fields
                .iter()
                .find(|field| field.focus_handle(cx).is_focused(window))
                .cloned()
        });
        if let Some(field) = field {
            let composing = field.update(cx, |input, cx| {
                if input.marked_text_range(window, cx).is_some() {
                    input.unmark_text(window, cx);
                    cx.notify();
                    true
                } else {
                    false
                }
            });
            if composing {
                window.prevent_default();
                cx.stop_propagation();
            }
        }
    }

    pub(super) fn form_read_only(&self, kind: &super::FormKind) -> bool {
        matches!(kind, super::FormKind::Database { editing: Some(id), .. }
            if !self.item_command_enabled(&super::ItemTarget::Database(id.clone()), super::ItemCommand::Remove))
    }

    pub(super) fn cancel_form(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        if let Some(form) = self.form.take() {
            self.restore_focus = form
                .return_focus
                .or_else(|| self.keyboard.as_ref().map(|keyboard| keyboard.body.clone()));
            self.notice = None;
            cx.notify();
        }
    }

    pub(super) fn submit_from_input(
        &mut self,
        _: &gpui_component::input::Enter,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        // Destructive confirmations require activating the labeled confirmation
        // button. An Enter meant for a text field must never delete or restore data.
        if self
            .form
            .as_ref()
            .is_some_and(|form| !matches!(form.kind, super::FormKind::Confirm { .. }))
        {
            self.save_form(cx);
        }
        window.prevent_default();
    }

    pub(super) fn move_focus(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.palette.is_some() {
            self.move_palette_selection(backwards, window, cx);
            return;
        }
        self.close_item_menu(window, cx);
        if backwards {
            window.focus_prev();
        } else {
            window.focus_next();
        }
    }

    pub(super) fn scroll_logs(
        &mut self,
        distance: Option<Pixels>,
        end: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.follow {
            self.follow = false;
            self.request(super::Request::Select(self.selected.clone(), false));
        }
        let mut offset = self.log_scroll.offset();
        offset.y = distance
            .map_or_else(
                || {
                    if end {
                        -self.log_scroll.max_offset().height
                    } else {
                        px(0.)
                    }
                },
                |distance| offset.y + distance,
            )
            .clamp(-self.log_scroll.max_offset().height, px(0.));
        self.log_scroll.set_offset(offset);
        cx.notify();
    }
}
