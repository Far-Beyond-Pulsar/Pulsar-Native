//! Hold Tab for the radial quick-action menu (Pulsar-Native#387).
//!
//! The flow, all on the UI thread:
//!
//! 1. **Tab down.** A keystroke interceptor (runs before key bindings) takes a
//!    plain Tab only when Tab would do nothing but move focus -- in a text
//!    input or code editor Tab keeps its own meaning. Focus moves to an
//!    invisible catcher so the Tab *release* reaches us wherever focus was,
//!    and the editor that had focus is remembered.
//! 2. **Held past the delay.** The menu opens at the pointer with the
//!    configured actions that editor handles (so each editor gets its own
//!    set: "the local editor").
//! 3. **While held.** Every key goes to the menu: ←/→ (and ↑/↓) step, Esc
//!    dismisses without running anything; scroll and pointer direction also
//!    select.
//! 4. **Tab up.** Focus goes back to the editor, then the selected action is
//!    dispatched there. A release before the delay is a normal Tab: focus
//!    moves to the next element, as it always did.

use std::time::Duration;

use gpui::{
    div, prelude::FluentBuilder as _, Action, AnyElement, App, Bounds, Context, FocusHandle,
    InteractiveElement as _, IntoElement, KeyUpEvent, KeystrokeEvent, ParentElement, Pixels, Point,
    Styled, Subscription, Window,
};
use ui::IconName;
use ui_common::radial_menu::{RadialMenu, RadialMenuItem, MAX_ITEMS};

use super::PulsarApp;

const OWNER: &str = "radial_menu";

pub struct RadialHost {
    phase: Phase,
    /// Holds focus while Tab is down, so the release reaches the host.
    catcher: FocusHandle,
    /// Bumped on every press and release; a hold timer from an earlier press
    /// sees a different value and does nothing.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

enum Phase {
    Idle,
    /// Tab is down. `menu` is `None` until the hold delay passes (or when the
    /// focused editor has no configured actions).
    Held {
        prev_focus: Option<FocusHandle>,
        menu: Option<RadialMenu>,
        /// The hold delay passed, so the release is never a plain Tab.
        held_long: bool,
        dismissed: bool,
    },
}

impl RadialHost {
    pub fn new(cx: &mut Context<PulsarApp>) -> Self {
        Self {
            phase: Phase::Idle,
            catcher: cx.focus_handle(),
            generation: 0,
            _subscriptions: Vec::new(),
        }
    }
}

// ── Settings ─────────────────────────────────────────────────────────────────

fn setting(key: &str) -> Option<engine_state::ConfigValue> {
    engine_state::global_config()
        .get(engine_state::NS_EDITOR, OWNER, key)
        .ok()
}

fn enabled() -> bool {
    !matches!(
        setting("enabled"),
        Some(engine_state::ConfigValue::Bool(false))
    )
}

fn hold_delay() -> Duration {
    let ms = match setting("hold_delay_ms") {
        Some(engine_state::ConfigValue::Int(ms)) => ms.clamp(50, 1000) as u64,
        _ => 180,
    };
    Duration::from_millis(ms)
}

fn configured_items() -> String {
    match setting("items") {
        Some(engine_state::ConfigValue::String(items)) => items,
        _ => pulsar_settings::editor::radial_menu::DEFAULT_ITEMS.to_string(),
    }
}

/// `Label | namespace::Action` or `namespace::Action`; blank lines and `#`
/// comments are skipped.
fn parse_line(line: &str) -> Option<(Option<&str>, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    match line.split_once('|') {
        Some((label, name)) => {
            let label = label.trim();
            Some(((!label.is_empty()).then_some(label), name.trim()))
        }
        None => Some((None, line)),
    }
}

/// `level_editor::DuplicateObject` → "Duplicate Object".
fn humanize(action_name: &str) -> String {
    let short = action_name.rsplit("::").next().unwrap_or(action_name);
    let mut out = String::with_capacity(short.len() + 4);
    for (i, ch) in short.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// An icon for well-known actions; anything else gets a generic one.
fn icon_for(action_name: &str) -> IconName {
    let short = action_name.rsplit("::").next().unwrap_or(action_name);
    match short {
        "SaveScene" | "SaveFile" | "SaveAll" => IconName::FloppyDisk,
        "PlayScene" => IconName::Play,
        "StopScene" => IconName::Square,
        "Undo" => IconName::Undo,
        "Redo" => IconName::Redo,
        "DuplicateObject" | "Copy" => IconName::Copy,
        "DeleteObject" | "Delete" => IconName::Trash,
        "ToggleGrid" => IconName::GridPlus,
        "FocusSelected" => IconName::FrameSelect,
        "ToggleCommandPalette" => IconName::Search,
        "ToggleFileManager" => IconName::FolderOpen,
        _ => IconName::Sparks,
    }
}

// ── Host ─────────────────────────────────────────────────────────────────────

impl PulsarApp {
    /// Hook the menu up to this window: the Tab interceptor, and cancelling
    /// when the window loses focus mid-hold (the release would never arrive).
    pub(super) fn install_radial_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let window_handle = window.window_handle();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != window_handle {
                return;
            }
            if let Some(app) = this.upgrade() {
                app.update(cx, |app, cx| app.radial_intercept(event, window, cx));
            }
        });
        let activation = cx.observe_window_activation(window, |app, window, cx| {
            if !window.is_window_active() {
                app.radial_finish(false, window, cx);
            }
        });
        self.state.radial._subscriptions = vec![intercept, activation];
    }

    fn radial_intercept(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if matches!(self.state.radial.phase, Phase::Idle) {
            if keystroke.key == "tab"
                && !keystroke.modifiers.modified()
                && enabled()
                && Self::tab_only_moves_focus(event, cx)
            {
                cx.stop_propagation();
                self.radial_press(window, cx);
            }
            return;
        }
        let Phase::Held {
            menu, dismissed, ..
        } = &mut self.state.radial.phase
        else {
            return;
        };
        // Tab is held: every key belongs to the menu (Tab itself is repeat).
        cx.stop_propagation();
        match keystroke.key.as_str() {
            "escape" => {
                *menu = None;
                *dismissed = true;
            }
            "right" | "down" => {
                if let Some(menu) = menu {
                    menu.select_next();
                }
            }
            "left" | "up" => {
                if let Some(menu) = menu {
                    menu.select_prev();
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// Whether Tab here would only do focus traversal (the root `Tab`
    /// binding) or nothing -- as opposed to indenting, completing, or any
    /// other binding a text input or editor gives Tab.
    fn tab_only_moves_focus(event: &KeystrokeEvent, cx: &App) -> bool {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let (bindings, _) =
            keymap.bindings_for_input(&[event.keystroke.clone()], &event.context_stack);
        bindings
            .first()
            .is_none_or(|binding| binding.action().name() == "root::Tab")
    }

    fn radial_press(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = &mut self.state.radial;
        host.generation += 1;
        let generation = host.generation;
        host.phase = Phase::Held {
            prev_focus: window.focused(cx),
            menu: None,
            held_long: false,
            dismissed: false,
        };
        host.catcher.focus(window, cx);

        let delay = hold_delay();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |app, window, cx| {
                app.radial_open(generation, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Hold delay passed: open the menu if Tab is still down.
    fn radial_open(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.radial.generation != generation {
            return;
        }
        let Phase::Held {
            prev_focus,
            menu,
            held_long,
            dismissed: false,
        } = &mut self.state.radial.phase
        else {
            return;
        };
        *held_long = true;
        if menu.is_some() {
            return;
        }
        let items = Self::radial_items(prev_focus.as_ref(), window, cx);
        if items.is_empty() {
            return;
        }
        let viewport = Bounds::new(Point::default(), window.viewport_size());
        *menu = Some(RadialMenu::new(items, window.mouse_position(), viewport));
        cx.notify();
    }

    /// The configured actions the editor that had focus can run.
    fn radial_items(
        prev_focus: Option<&FocusHandle>,
        window: &Window,
        cx: &mut App,
    ) -> Vec<RadialMenuItem> {
        let mut items: Vec<RadialMenuItem> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for line in configured_items().lines() {
            let Some((label, name)) = parse_line(line) else {
                continue;
            };
            if !seen.insert(name.to_string()) {
                continue;
            }
            let Ok(action) = cx.build_action(name, None) else {
                tracing::debug!("radial menu: unknown action '{name}'");
                continue;
            };
            let available = match prev_focus {
                Some(focus) => window.is_action_available_in(action.as_ref(), focus),
                None => window.is_action_available(action.as_ref(), cx),
            };
            if !available {
                continue;
            }
            items.push(RadialMenuItem {
                label: label
                    .map(str::to_string)
                    .unwrap_or_else(|| humanize(name))
                    .into(),
                icon: icon_for(name),
                action,
            });
            if items.len() == MAX_ITEMS {
                break;
            }
        }
        items
    }

    /// Tab released: back to the editor, then run the selection -- or, if the
    /// menu never opened, do the normal Tab.
    fn radial_release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.radial_finish(true, window, cx);
    }

    /// End a hold. `run` = the release happened (vs. the window losing focus,
    /// which only restores focus).
    fn radial_finish(&mut self, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let host = &mut self.state.radial;
        let Phase::Held {
            prev_focus,
            menu,
            held_long,
            dismissed,
        } = std::mem::replace(&mut host.phase, Phase::Idle)
        else {
            return;
        };
        host.generation += 1;
        if let Some(focus) = &prev_focus {
            focus.focus(window, cx);
        }
        if run && !dismissed {
            match menu {
                // Dispatched from the restored focus, so it reaches that editor.
                Some(menu) => {
                    if let Some(action) = menu.selected_action() {
                        window.dispatch_action(action, cx);
                    }
                }
                // Released before the delay: an ordinary Tab.
                None if !held_long => window.focus_next(cx),
                // Held, but this editor has no configured actions.
                None => {}
            }
        }
        cx.notify();
    }

    /// The catcher (always present, invisible) and, while open, the menu.
    pub(super) fn render_radial_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let catcher = div()
            .id("radial-menu-catcher")
            .absolute()
            .size_0()
            .track_focus(&self.state.radial.catcher)
            .on_key_up(cx.listener(|app, event: &KeyUpEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    app.radial_release(window, cx);
                }
            }));

        let menu = match &self.state.radial.phase {
            Phase::Held {
                menu: Some(menu), ..
            } => {
                let hover = cx.listener(|app, position: &Point<Pixels>, _, cx| {
                    if let Phase::Held {
                        menu: Some(menu), ..
                    } = &mut app.state.radial.phase
                    {
                        if menu.select_at(*position) {
                            cx.notify();
                        }
                    }
                });
                let scroll = cx.listener(|app, delta: &Pixels, _, cx| {
                    if let Phase::Held {
                        menu: Some(menu), ..
                    } = &mut app.state.radial.phase
                    {
                        if menu.scroll(*delta) {
                            cx.notify();
                        }
                    }
                });
                Some(menu.render(
                    window,
                    cx,
                    move |position, window, cx| hover(&position, window, cx),
                    move |delta, window, cx| scroll(&delta, window, cx),
                ))
            }
            _ => None,
        };

        div()
            .child(catcher)
            .when_some(menu, |el, menu| el.child(menu))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_lines_parse_with_and_without_labels() {
        assert_eq!(
            parse_line("Save | level_editor::SaveScene"),
            Some((Some("Save"), "level_editor::SaveScene"))
        );
        assert_eq!(
            parse_line("  level_editor::Undo  "),
            Some((None, "level_editor::Undo"))
        );
        assert_eq!(
            parse_line(" | level_editor::Undo"),
            Some((None, "level_editor::Undo"))
        );
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("# comment"), None);
    }

    #[test]
    fn action_names_humanize() {
        assert_eq!(
            humanize("level_editor::DuplicateObject"),
            "Duplicate Object"
        );
        assert_eq!(humanize("Undo"), "Undo");
    }

    #[test]
    fn default_items_all_parse() {
        let parsed: Vec<_> = pulsar_settings::editor::radial_menu::DEFAULT_ITEMS
            .lines()
            .filter_map(parse_line)
            .collect();
        assert!(parsed.len() >= 8);
        assert!(parsed
            .iter()
            .all(|(label, name)| label.is_some() && name.contains("::")));
    }
}
