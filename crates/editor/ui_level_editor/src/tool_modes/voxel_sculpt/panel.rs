//! Shared lifecycle for the voxel tools and material library dock panels.

use crate::state::{
    voxel::{VoxelSculptDomain, MAX_RADIUS_M, MIN_RADIUS_M},
    LevelEditorState,
};
use gpui::*;
use rust_i18n::t;
use std::{collections::HashSet, sync::Arc};
use ui::{
    button::Button,
    h_flex,
    input::{InputEvent, InputState},
    v_flex, ActiveTheme, Icon, IconName, Sizable,
};

type SharedState = Arc<parking_lot::RwLock<LevelEditorState>>;

#[derive(Clone, Copy)]
enum PanelKind {
    Tools,
    Materials,
}

pub struct VoxelSculptPanel {
    pub(super) state: SharedState,
    focus_handle: FocusHandle,
    kind: PanelKind,
    last_settings: VoxelSculptDomain,
    pump_started: bool,
    collapsed: HashSet<&'static str>,
    pub(super) radius: Entity<InputState>,
    pub(super) radius_error: bool,
    pub(super) search: Entity<InputState>,
    pub(super) favorites_only: bool,
    _subscriptions: Vec<Subscription>,
}

impl VoxelSculptPanel {
    pub fn new(state: SharedState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::create(state, PanelKind::Tools, window, cx)
    }

    pub fn materials(state: SharedState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::create(state, PanelKind::Materials, window, cx)
    }

    fn create(
        state: SharedState,
        kind: PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_settings = state.read().editor.voxel;
        let radius = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(format!("{:.2}", last_settings.radius_m), window, cx);
            input
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("LevelEditor.VoxelPanel.Search").to_string())
        });
        let radius_subscription = cx.subscribe_in(
            &radius,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    // Single-voxel mode ignores the stored volume brush radius.
                    if this.state.read().editor.voxel.single_block {
                        return;
                    }
                    let value = input
                        .read(cx)
                        .text()
                        .to_string()
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite() && (MIN_RADIUS_M..=MAX_RADIUS_M).contains(v));
                    this.radius_error = value.is_none();
                    if let Some(value) = value {
                        this.state.write().editor.voxel.set_radius(value);
                        input.update(cx, |input, cx| {
                            input.set_value(format!("{value:.2}"), window, cx)
                        });
                    }
                    cx.notify();
                }
            },
        );
        let search_subscription =
            cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
        Self {
            state,
            focus_handle: cx.focus_handle(),
            kind,
            last_settings,
            pump_started: false,
            collapsed: HashSet::new(),
            radius,
            radius_error: false,
            search,
            favorites_only: false,
            _subscriptions: vec![radius_subscription, search_subscription],
        }
    }

    pub(super) fn button(
        &self,
        id: &'static str,
        label: String,
        cx: &mut Context<Self>,
        action: impl Fn(&mut VoxelSculptDomain) + 'static,
    ) -> Button {
        Button::new(id)
            .label(label)
            .small()
            .on_click(cx.listener(move |this, _, _, cx| {
                {
                    let mut state = this.state.write();
                    let old_radius = state.editor.voxel.radius_m;
                    action(&mut state.editor.voxel);
                    if state.editor.voxel.radius_m != old_radius {
                        this.radius_error = false;
                    }
                }
                cx.notify();
            }))
    }

    pub(super) fn section(
        &self,
        id: &'static str,
        key: &'static str,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.collapsed.contains(id);
        let theme = cx.theme();
        let mut section = v_flex().w_full().gap_2().child(
            h_flex()
                .id(id)
                .gap_1()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .cursor_pointer()
                .child(
                    Icon::new(if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .size_3p5(),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!(key).to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.collapsed.remove(id) {
                        this.collapsed.insert(id);
                    }
                    cx.notify();
                })),
        );
        if !collapsed {
            section = section.child(body);
        }
        section.into_any_element()
    }
}

impl EventEmitter<ui::dock::PanelEvent> for VoxelSculptPanel {}
ui_common::panel_boilerplate!(VoxelSculptPanel);

impl Render for VoxelSculptPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.pump_started {
            self.pump_started = true;
            crate::ui::frame_pump::spawn_frame_pump(&cx.entity(), window, |this, _, cx| {
                let settings = this.state.read().editor.voxel;
                if settings != this.last_settings {
                    this.last_settings = settings;
                    cx.notify();
                }
            });
        }
        let settings = self.state.read().editor.voxel;
        self.last_settings = settings;
        let theme = cx.theme().clone();
        let (title, subtitle) = match self.kind {
            PanelKind::Tools => (
                "LevelEditor.VoxelPanel.ToolsTitle",
                "LevelEditor.VoxelPanel.ToolsSubtitle",
            ),
            PanelKind::Materials => (
                "LevelEditor.VoxelPanel.MaterialsTitle",
                "LevelEditor.VoxelPanel.MaterialsSubtitle",
            ),
        };
        let body = match self.kind {
            PanelKind::Tools => self.render_brush(settings, window, cx),
            PanelKind::Materials => self.render_materials(settings, cx),
        };
        v_flex()
            .size_full()
            .overflow_y_scroll()
            .bg(theme.sidebar)
            .p_3()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(t!(title).to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!(subtitle).to_string()),
            )
            .child(body)
    }
}

impl ui::dock::Panel for VoxelSculptPanel {
    fn panel_name(&self) -> &'static str {
        match self.kind {
            PanelKind::Tools => "voxel_sculpt.panel",
            PanelKind::Materials => "voxel_materials.panel",
        }
    }
    fn title(&self, _: &Window, _: &App) -> AnyElement {
        let key = match self.kind {
            PanelKind::Tools => "LevelEditor.VoxelPanel.ToolsTitle",
            PanelKind::Materials => "LevelEditor.VoxelPanel.MaterialsTitle",
        };
        t!(key).to_string().into_any_element()
    }
}
