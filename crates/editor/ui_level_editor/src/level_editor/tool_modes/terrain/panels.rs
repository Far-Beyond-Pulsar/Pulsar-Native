//! Terrain mode's own GPUI dock panel — the brush palette.
//!
//! A mode owning a *real* panel rather than a control strip, as opposed to
//! the declarative [`ToolWidget`](super::super::ToolWidget) path:
//! [`TerrainPalettePanel`] renders `TerrainDomain`'s sculpt brush directly
//! with `ui::` components (segmented mode/shape pickers, +/− stepper
//! sliders), exactly the kind of content a declarative widget list cannot
//! express. It reads and writes the shared editor state through the same
//! `Arc<RwLock<LevelEditorState>>` every other panel does, and frame-pumps
//! like the rest of them so it stays honest about arbitrary state writes.
//!
//! Wired up by `super::layout::contributed_panels` +
//! [`TerrainMode::build_panel`](super::TerrainMode::build_panel) — the
//! "full GPUI in a tool mode" contract from the design doc's §11. Mode-owned
//! panels are created only on mode switch (not per render), and torn down
//! wholesale when the mode stops contributing them.

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable, IconName, Sizable,
};

use crate::level_editor::state::terrain::{BrushShape, SculptMode, TerrainTarget, TerrainDomain};
use crate::level_editor::state::LevelEditorState;

/// Sculpt-brush state this panel paints, copied out of the domain so the
/// frame pump can cheaply diff it. `TerrainTarget` is derived text (changes
/// only alongside these values), so it intentionally stays out of the
/// signature.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PaletteSignature {
    mode: SculptMode,
    shape: BrushShape,
    radius_m: f32,
    strength: f32,
    falloff: f32,
    material: u32,
}

impl PaletteSignature {
    fn of(state: &LevelEditorState) -> Self {
        let brush = &state.editor.terrain.sculpt;
        Self {
            mode: brush.mode,
            shape: brush.shape,
            radius_m: brush.radius_m,
            strength: brush.strength,
            falloff: brush.falloff,
            material: brush.material,
        }
    }
}

/// Full-GPUI brush palette dock panel for [`TerrainMode`](super::TerrainMode).
pub struct TerrainPalettePanel {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    /// Last signature this panel rendered; the frame pump re-notifies only
    /// when something it actually paints changed.
    last_signature: PaletteSignature,
    pump_started: bool,
}

impl TerrainPalettePanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_signature = PaletteSignature::of(&state.read());
        Self {
            state,
            focus_handle: cx.focus_handle(),
            last_signature,
            pump_started: false,
        }
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _window, cx| {
                let signature = PaletteSignature::of(&this.state.read());
                if signature != this.last_signature {
                    this.last_signature = signature;
                    cx.notify();
                }
            },
        );
    }
}

impl EventEmitter<ui::dock::PanelEvent> for TerrainPalettePanel {}

ui_common::panel_boilerplate!(TerrainPalettePanel);

impl Render for TerrainPalettePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("terrain palette panel: render");
        let _t = gpui::render_stats::scope("terrain palette panel: render");

        self.start_pump(window, cx);

        // Record what we are about to paint so the pump doesn't immediately
        // re-notify for a change this render already picked up (the same
        // pattern every frame-pumped panel here follows).
        self.last_signature = PaletteSignature::of(&self.state.read());

        let state = self.state.read();
        let terrain = &state.editor.terrain;
        let brush = terrain.sculpt;
        let state_arc = self.state.clone();
        let theme = cx.theme();

        let target_label = match &terrain.target {
            TerrainTarget::None => t!("LevelEditor.TerrainPalette.NoTarget").to_string(),
            TerrainTarget::Planet(id) => format!("Planet {id}"),
            TerrainTarget::Volume(id) => format!("Volume {id}"),
        };

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(t!("LevelEditor.TerrainPalette.Title").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(target_label),
                    ),
            )
            .child(div().w_full().h_px().bg(theme.border.opacity(0.4)))
            .child(Self::section_label(theme, "LevelEditor.Terrain.Section.Brush"))
            .child(Self::segmented_row(
                state_arc.clone(),
                cx,
                "palette_sculpt_mode",
                &[
                    ("LevelEditor.Terrain.Raise", "raise"),
                    ("LevelEditor.Terrain.Lower", "lower"),
                    ("LevelEditor.Terrain.Flatten", "flatten"),
                    ("LevelEditor.Terrain.Paint", "paint"),
                ],
                match brush.mode {
                    SculptMode::Raise => "raise",
                    SculptMode::Lower => "lower",
                    SculptMode::Flatten => "flatten",
                    SculptMode::Paint => "paint",
                },
                |domain, value| {
                    domain.set_sculpt_mode(match value {
                        "lower" => SculptMode::Lower,
                        "flatten" => SculptMode::Flatten,
                        "paint" => SculptMode::Paint,
                        _ => SculptMode::Raise,
                    })
                },
            ))
            .child(Self::segmented_row(
                state_arc.clone(),
                cx,
                "palette_brush_shape",
                &[
                    ("LevelEditor.Terrain.Shape.Sphere", "sphere"),
                    ("LevelEditor.Terrain.Shape.Box", "box"),
                ],
                match brush.shape {
                    BrushShape::Sphere => "sphere",
                    BrushShape::Box => "box",
                },
                |domain, value| {
                    domain.set_brush_shape(match value {
                        "box" => BrushShape::Box,
                        _ => BrushShape::Sphere,
                    })
                },
            ))
            .child(Self::slider_row(
                state_arc.clone(),
                cx,
                "palette_radius",
                "LevelEditor.Terrain.Radius",
                brush.radius_m,
                1.0,
                64.0,
                0.5,
                TerrainDomain::set_brush_radius,
            ))
            .child(Self::slider_row(
                state_arc.clone(),
                cx,
                "palette_strength",
                "LevelEditor.Terrain.Strength",
                brush.strength,
                0.1,
                10.0,
                0.1,
                TerrainDomain::set_brush_strength,
            ))
            .child(Self::slider_row(
                state_arc.clone(),
                cx,
                "palette_falloff",
                "LevelEditor.Terrain.Falloff",
                brush.falloff,
                0.0,
                1.0,
                0.05,
                TerrainDomain::set_brush_falloff,
            ))
            .child(Self::slider_row(
                state_arc.clone(),
                cx,
                "palette_material",
                "LevelEditor.Terrain.Material",
                brush.material as f32,
                1.0,
                15.0,
                1.0,
                |domain, value| domain.set_brush_material(value as u32),
            ))
    }
}

impl TerrainPalettePanel {
    fn section_label(cx_theme: &ui::Theme, label_key: &'static str) -> impl IntoElement {
        div()
            .w_full()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx_theme.muted_foreground)
            .child(t!(label_key).to_string().to_uppercase())
    }

    /// A one-of-many picker rendered as a row of small toggle buttons. The
    /// apply closure receives the *domain* because every mutation these
    /// buttons make is a brush-config write — none of them touch the scene,
    /// so they go straight to the domain's clamped setters, exactly like the
    /// tree of `SetTerrain*` toolbar actions they mirror.
    fn segmented_row<V, F>(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
        control_id: &'static str,
        options: &[(&'static str, &'static str)],
        selected: &'static str,
        apply: F,
    ) -> impl IntoElement
    where
        V: 'static,
        F: Fn(&mut TerrainDomain, &'static str) + Clone + Send + Sync + 'static,
    {
        let mut group = h_flex()
            .w_full()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .bg(cx.theme().muted.opacity(0.1))
            .p(px(2.0))
            .gap_1();

        for &(label_key, value) in options {
            let is_sel = value == selected;
            let state = state_arc.clone();
            let apply = apply.clone();
            let btn = Button::new(format!("{control_id}_{value}"))
                .label(t!(label_key))
                .small()
                .on_click(move |_, _, _| {
                    let mut st = state.write();
                    apply(&mut st.editor.terrain, value);
                });
            let btn = if is_sel { btn.primary() } else { btn.ghost() };
            group = group.child(btn);
        }

        group
    }

    /// A label + value + −/＋ stepper row, mirroring the `ToolWidget::Slider`
    /// rendering in `ui/mode_widgets.rs` but writing through the domain's
    /// clamped setter directly instead of a dispatcher id.
    fn slider_row<V, F>(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
        control_id: &'static str,
        label_key: &'static str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        apply: F,
    ) -> impl IntoElement
    where
        V: 'static,
        F: Fn(&mut TerrainDomain, f32) + Clone + Send + Sync + 'static,
    {
        let theme = cx.theme();
        let dec_val = (value - step).clamp(min, max);
        let inc_val = (value + step).clamp(min, max);

        let (dec_state, dec_apply) = (state_arc.clone(), apply.clone());
        let dec = Button::new(format!("{control_id}_dec"))
            .icon(IconName::Minus)
            .small()
            .ghost()
            .disabled(value <= min)
            .on_click(move |_, _, _| {
                let mut st = dec_state.write();
                dec_apply(&mut st.editor.terrain, dec_val);
            });

        let (inc_state, inc_apply) = (state_arc.clone(), apply);
        let inc = Button::new(format!("{control_id}_inc"))
            .icon(IconName::Plus)
            .small()
            .ghost()
            .disabled(value >= max)
            .on_click(move |_, _, _| {
                let mut st = inc_state.write();
                inc_apply(&mut st.editor.terrain, inc_val);
            });

        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{}:", t!(label_key))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(dec)
                    .child(
                        div()
                            .min_w(px(32.0))
                            .text_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if step < 1.0 {
                                format!("{:.1}", value)
                            } else {
                                format!("{:.0}", value)
                            }),
                    )
                    .child(inc),
            )
    }
}

impl ui::dock::Panel for TerrainPalettePanel {
    fn panel_name(&self) -> &'static str {
        super::layout::TERRAIN_PALETTE
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        t!("LevelEditor.TerrainPalette.Title").to_string().into_any_element()
    }
}