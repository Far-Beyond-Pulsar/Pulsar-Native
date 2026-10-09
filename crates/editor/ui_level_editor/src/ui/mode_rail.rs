//! Persistent navigation for the Level Editor's registered tool modes.
//!
//! The rail stays visible while the active mode's dock panels change. Keeping
//! the list in its own cached view avoids rebuilding it with every viewport
//! frame, while the frame pump still picks up modes registered at runtime.

use std::sync::Arc;

use gpui::*;
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    v_flex, ActiveTheme, IconName, Sizable,
};

use crate::{
    state::LevelEditorState,
    tool_modes::ToolModeId,
    ui::{frame_pump::spawn_frame_pump, toolbar::SetToolMode},
};

#[derive(Clone)]
struct ModeEntry {
    id: ToolModeId,
    label: String,
    description: String,
    icon: IconName,
}

impl PartialEq for ModeEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.description == other.description
            && format!("{:?}", self.icon) == format!("{:?}", other.icon)
    }
}

#[derive(Clone, PartialEq)]
struct RailSignature {
    selected: ToolModeId,
    modes: Vec<ModeEntry>,
}

impl RailSignature {
    fn of(state: &LevelEditorState) -> Self {
        Self {
            selected: state.editor.tool_mode_registry.selected_id(),
            modes: state
                .editor
                .tool_mode_registry
                .modes()
                .iter()
                .map(|mode| ModeEntry {
                    id: mode.id(),
                    label: t!(mode.label_key()).to_string(),
                    description: t!(mode.description_key()).to_string(),
                    icon: mode.icon(),
                })
                .collect(),
        }
    }
}

pub(crate) struct ModeRailView {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    signature: RailSignature,
    pump_started: bool,
}

impl ModeRailView {
    pub(crate) fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        let signature = RailSignature::of(&state.read());
        Self {
            state,
            signature,
            pump_started: false,
        }
    }

    pub(crate) fn cache_style() -> StyleRefinement {
        StyleRefinement::default().w(px(148.0)).h_full()
    }
}

impl EventEmitter<ui::dock::PanelEvent> for ModeRailView {}

impl Render for ModeRailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.pump_started {
            self.pump_started = true;
            spawn_frame_pump(&cx.entity(), window, |this, _, cx| {
                let signature = RailSignature::of(&this.state.read());
                if signature != this.signature {
                    this.signature = signature;
                    cx.notify();
                }
            });
        }

        self.signature = RailSignature::of(&self.state.read());
        let theme = cx.theme().clone();
        let selected = self.signature.selected;
        let mut modes = v_flex().gap_1();
        for mode in self.signature.modes.iter().cloned() {
            let is_selected = mode.id == selected;
            let button = Button::new(format!("tool-mode-{}", mode.id.0))
                .label(mode.label)
                .icon(mode.icon)
                .small()
                .w_full()
                .tooltip(mode.description)
                .on_click(move |_, _, cx| {
                    cx.dispatch_action(&SetToolMode(mode.id));
                });
            modes = modes.child(if is_selected {
                button.primary()
            } else {
                button.ghost()
            });
        }

        v_flex()
            .id("level-editor-mode-rail")
            .w(px(148.0))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border.opacity(0.65))
            .child(
                v_flex()
                    .flex_shrink_0()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.muted_foreground)
                            .child("WORK MODES"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Choose an editing workspace"),
                    ),
            )
            .child(
                v_flex()
                    .id("level-editor-mode-rail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_3()
                    .child(modes),
            )
    }
}
