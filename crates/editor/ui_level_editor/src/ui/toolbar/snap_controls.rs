use gpui::{prelude::FluentBuilder as _, *};
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    popover::Popover,
    v_flex, ActiveTheme as _, IconName, Selectable as _, Sizable as _,
};

use crate::LevelEditorState;

#[derive(Clone, Copy)]
pub enum SnapKind {
    Location,
    Rotation,
    Scale,
}

impl SnapKind {
    fn title(self) -> &'static str {
        match self {
            Self::Location => "Location",
            Self::Rotation => "Rotation",
            Self::Scale => "Scale",
        }
    }
    fn current(self, state: &LevelEditorState) -> f32 {
        match self {
            Self::Location => state.editor.location_snap,
            Self::Rotation => state.editor.rotation_snap,
            Self::Scale => state.editor.scale_snap,
        }
    }
    fn values(self) -> &'static [f32] {
        match self {
            Self::Location => &[0.1, 0.5, 1.0, 5.0, 10.0],
            Self::Rotation => &[1.0, 5.0, 10.0, 15.0, 30.0, 45.0, 90.0],
            Self::Scale => &[0.01, 0.05, 0.1, 0.25, 0.5, 1.0],
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Self::Location => "↔",
            Self::Rotation => "⟳",
            Self::Scale => "↗",
        }
    }
    fn set(self, state: &mut LevelEditorState, value: f32) {
        match self {
            Self::Location => state.editor.location_snap = value,
            Self::Rotation => state.editor.rotation_snap = value,
            Self::Scale => state.editor.scale_snap = value,
        }
    }
}

pub struct SnapPanel {
    focus: FocusHandle,
    kind: SnapKind,
    input: Entity<InputState>,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    mailbox: Option<engine_backend::subsystems::render::HelioEditorMailbox>,
}

impl SnapPanel {
    pub fn new(
        cx: &mut Context<Self>,
        kind: SnapKind,
        input: Entity<InputState>,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        mailbox: Option<engine_backend::subsystems::render::HelioEditorMailbox>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            kind,
            input,
            state,
            mailbox,
        }
    }

    fn choose(&self, value: f32) {
        let (location, rotation, scale) = {
            let mut state = self.state.write();
            self.kind.set(&mut state, value);
            (
                state.editor.location_snap,
                state.editor.rotation_snap,
                state.editor.scale_snap,
            )
        };
        let (key, persisted_value) = match self.kind {
            SnapKind::Location => ("location_snap", location),
            SnapKind::Rotation => ("rotation_snap", rotation),
            SnapKind::Scale => ("scale_snap", scale),
        };
        if let Err(error) = engine_state::GlobalSettings::new().set_and_save(
            "viewport",
            key,
            engine_state::ConfigValue::Float(persisted_value as f64),
        ) {
            tracing::warn!(%error, "Could not persist viewport snap setting");
        }
        if let Some(mailbox) = &self.mailbox {
            mailbox.set_gizmo_snap_settings(location, rotation, scale);
        }
    }
}

impl EventEmitter<DismissEvent> for SnapPanel {}
impl Focusable for SnapPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SnapPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.kind.title();
        let current = self.kind.current(&self.state.read());
        let values = self.kind.values();
        let theme = cx.theme();
        let mut choices = v_flex().gap_0p5();
        for value in values {
            let selected = (*value - current).abs() < 0.0001;
            let value = *value;
            let panel = cx.entity();
            choices = choices.child(
                h_flex()
                    .id(format!("snap-choice-{title}-{value}"))
                    .w_full()
                    .h_7()
                    .items_center()
                    .justify_start()
                    .gap_2()
                    .px_2()
                    .rounded_sm()
                    .text_sm()
                    .text_left()
                    .when(selected, |row| row.bg(theme.input))
                    .child(div().flex_1().text_left().child(compact(value)))
                    .when(selected, |row| row.child(div().child("✓")))
                    .on_click(move |_, _, cx| panel.update(cx, |this, _| this.choose(value))),
            );
        }
        v_flex()
            .w(px(160.0))
            .gap_2()
            .p_2()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{title} Snap")),
            )
            .child(choices)
            .child(div().flex_1())
            .child(div().h_px().w_full().bg(theme.border))
            .child(TextInput::new(&self.input).w_full())
    }
}

pub struct TransformSnapControls;

impl TransformSnapControls {
    pub fn render(panels: &[Entity<SnapPanel>; 3], state: &LevelEditorState) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(trigger(
                "snap-location",
                SnapKind::Location,
                panels[0].clone(),
                state.editor.location_snap,
            ))
            .child(trigger(
                "snap-rotation",
                SnapKind::Rotation,
                panels[1].clone(),
                state.editor.rotation_snap,
            ))
            .child(trigger(
                "snap-scale",
                SnapKind::Scale,
                panels[2].clone(),
                state.editor.scale_snap,
            ))
    }
}

fn trigger(
    id: &'static str,
    kind: SnapKind,
    panel: Entity<SnapPanel>,
    current: f32,
) -> impl IntoElement {
    Popover::<SnapPanel>::new(id)
        .anchor(Corner::TopLeft)
        .trigger(
            Button::new(id)
                .label(format!("{} {}", kind.glyph(), compact(current)))
                .icon(IconName::ChevronDown)
                .small()
                .ghost()
                .tooltip(format!("{} snap", kind.title())),
        )
        .content(move |_, _| panel.clone())
}

fn compact(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}
