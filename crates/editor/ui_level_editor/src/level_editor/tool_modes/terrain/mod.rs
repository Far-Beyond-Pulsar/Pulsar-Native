//! Terrain Tool Mode
//!
//! Authoring mode for voxel terrain sculpting. Surfaces sculpt mode
//! (Raise/Lower/Flatten/Paint), brush radius, strength and falloff, and turns
//! pointer input into canonical `EditOp` stamps against the active planet
//! through [`TerrainEditApi`] (design doc §5.3/§5.4).
//!
//! # Pointer contract
//!
//! `Consumed` is returned only when the brush actually had terrain under it.
//! Clicking empty space (or clicking with no planet in the scene) falls
//! through to the ordinary pick/gizmo path, so the user can still select
//! objects and frame the camera without leaving the mode. `Hover` always falls
//! through — it only refreshes the brush ring, and the renderer still needs it
//! for its own hover feedback.

pub mod foliage;
pub mod layout;
pub mod panels;
pub mod ray;
pub mod scene_planets;
pub mod sculpt;

use engine_backend::services::terrain_edit::{
    BrushCursorRequest, TerrainEditApi, TerrainHit, TerrainTarget as PlanetTarget,
};

use gpui::AppContext;

use super::{
    BrushCursor, ModeLayout, ModePanelDescriptor, PanelTab, PointerKind, StatusReadout, ToolMode,
    ToolModeContext, ToolModeId, ToolPointerEvent, ToolPointerResult, ToolWidget,
};
use crate::level_editor::core::commands::{execute_command, SceneCommand};
use crate::level_editor::state::terrain::{BrushShape, SculptMode, TerrainTarget};

/// Brush ring colour per sculpt mode, so the mode is readable at a glance
/// without looking back at the toolbar.
const RAISE_COLOR: [f32; 4] = [0.35, 0.95, 0.45, 0.9];
const LOWER_COLOR: [f32; 4] = [0.95, 0.45, 0.35, 0.9];
const FLATTEN_COLOR: [f32; 4] = [0.45, 0.65, 0.95, 0.9];
const PAINT_COLOR: [f32; 4] = [0.95, 0.85, 0.35, 0.9];
/// Brush ring colour while the Foliage sub-mode is active, distinct from
/// every sculpt-mode colour (which are all green/red/blue/yellow) so the
/// toggle is readable at a glance.
const FOLIAGE_COLOR: [f32; 4] = [0.85, 0.35, 0.85, 0.9];

/// Tool mode for voxel terrain editing and foliage painting.
#[derive(Clone, Default)]
pub struct TerrainMode {
    /// Hit point of the last committed sculpt stamp, for drag coalescing.
    /// Cleared at stroke boundaries so the first stamp of every stroke
    /// always commits.
    last_stamp_center_m: Option<[f32; 3]>,
    /// Hit point of the last committed foliage stamp. Tracked separately
    /// from `last_stamp_center_m` even though the two brushes are mutually
    /// exclusive (the `paint_foliage` toggle), so switching sub-modes
    /// mid-drag can never let one brush's coalescing state leak into the
    /// other's.
    last_foliage_stamp_center_m: Option<[f32; 3]>,
    /// Latest brush ring, refreshed on every pointer event that hit terrain.
    cursor: Option<BrushCursor>,
    /// Surface normal at `cursor`, needed to orient the drawn ring. Not part
    /// of the shell-facing [`BrushCursor`], which is plane-agnostic.
    cursor_normal: [f32; 3],
    /// Monotonic stroke id handed to `TerrainDomain::begin_stroke`.
    next_stroke_id: u64,
}

impl TerrainMode {
    /// Ray-cast the pointer position against the active terrain.
    fn hit_at(
        &self,
        api: &TerrainEditApi,
        ctx: &ToolModeContext,
        event: &ToolPointerEvent,
    ) -> Option<TerrainHit> {
        let ray = ray::viewport_ray(ctx.camera, ctx.viewport, event.norm_x, event.norm_y)?;
        api.hit_terrain(ray)
    }

    /// Publish the brush ring for this hit, both to the mode (for
    /// [`ToolMode::brush_cursor`]) and to the renderer's debug-draw mailbox.
    fn update_cursor(&mut self, api: &TerrainEditApi, ctx: &ToolModeContext, hit: &TerrainHit) {
        let terrain = &ctx.state.editor.terrain;
        let (radius_m, color) = if terrain.paint_foliage {
            (terrain.foliage.radius_m.max(1.0), FOLIAGE_COLOR)
        } else {
            (
                sculpt::effective_radius_m(&terrain.sculpt),
                mode_color(terrain.sculpt.mode),
            )
        };
        self.cursor = Some(BrushCursor {
            center: hit.position_m,
            radius: radius_m,
            color,
        });
        self.cursor_normal = hit.normal;
        api.set_brush_cursor(Some(BrushCursorRequest {
            center_m: hit.position_m,
            normal: hit.normal,
            radius_m,
            color,
        }));
    }

    fn clear_cursor(&mut self, api: Option<&TerrainEditApi>) {
        self.cursor = None;
        if let Some(api) = api {
            api.set_brush_cursor(None);
        }
    }

    /// Apply one brush stamp. Returns `true` when an op was actually
    /// committed — a coalesced-away drag returns `false` but is still
    /// `Consumed`, because the user *is* sculpting, just not far enough yet.
    fn stamp(&mut self, api: &TerrainEditApi, ctx: &mut ToolModeContext, hit: &TerrainHit) -> bool {
        let brush = ctx.state.editor.terrain.sculpt;
        let radius_m = sculpt::effective_radius_m(&brush);
        if !sculpt::should_stamp(self.last_stamp_center_m, hit.position_m, radius_m) {
            return false;
        }
        let Some(definition) = api.body_definition(hit.target) else {
            return false;
        };
        let anchor = ctx
            .state
            .editor
            .terrain
            .active_stroke
            .map(|stroke| stroke.start_height);
        let stamp = sculpt::build_stamp(hit, &definition, &brush, anchor);

        match api.apply_edit(hit.target, stamp.op) {
            Ok(committed) => {
                ctx.state.editor.terrain_undo.record_stamp(committed);
                self.last_stamp_center_m = Some(hit.position_m);
                true
            }
            Err(error) => {
                tracing::warn!(%error, "terrain sculpt stamp was rejected");
                false
            }
        }
    }

    /// Open a stroke: capture the undo anchor and the flatten reference
    /// altitude, and remember which planet is being edited.
    fn begin_stroke(&mut self, api: &TerrainEditApi, ctx: &mut ToolModeContext, hit: &TerrainHit) {
        let start_height = api
            .body_definition(hit.target)
            .map(|definition| sculpt::altitude_m(&definition, hit.position_m))
            .unwrap_or_default();

        // The undo anchor must exist before the first stamp lands, or the
        // stroke would be unrevertable.
        if !ctx
            .state
            .editor
            .terrain_undo
            .begin_stroke(api, hit.target)
        {
            tracing::warn!("terrain stroke could not capture an undo anchor; sculpting anyway");
        }

        self.next_stroke_id = self.next_stroke_id.wrapping_add(1);
        let stroke_id = self.next_stroke_id;
        ctx.state
            .editor
            .terrain
            .begin_stroke(stroke_id, start_height);
        ctx.state.editor.terrain.set_target(editor_target(hit.target));
        self.last_stamp_center_m = None;
    }

    /// Close a stroke and commit it to terrain undo history.
    ///
    /// Shared by both brushes: `terrain_undo.end_stroke()` is a no-op when
    /// no anchor was ever opened (see [`Self::begin_foliage_stroke`]), so
    /// calling it unconditionally after a foliage stroke is harmless.
    fn end_stroke(&mut self, ctx: &mut ToolModeContext) {
        ctx.state.editor.terrain.end_stroke();
        ctx.state.editor.terrain_undo.end_stroke();
        self.last_stamp_center_m = None;
        self.last_foliage_stamp_center_m = None;
    }

    /// Open a foliage stroke: unlike [`Self::begin_stroke`], this never
    /// touches `TerrainUndoDomain` -- foliage painting mutates no voxels, so
    /// there is nothing for that history to snapshot. Each stamp is its own
    /// `SceneCommand::AddObject`, already undo-tracked by the scene's own
    /// snapshot undo (`execute_command`). See this module's `foliage`
    /// submodule doc and issue #713's "undo pairing" question for why the
    /// two undo systems are deliberately kept independent rather than
    /// merged into one stroke-level unit here.
    fn begin_foliage_stroke(&mut self, ctx: &mut ToolModeContext, hit: &TerrainHit) {
        self.next_stroke_id = self.next_stroke_id.wrapping_add(1);
        let stroke_id = self.next_stroke_id;
        ctx.state.editor.terrain.begin_stroke(stroke_id, 0.0);
        ctx.state.editor.terrain.set_target(editor_target(hit.target));
        self.last_foliage_stamp_center_m = None;
    }

    /// Apply one foliage brush stamp: coalesce like sculpt does (reusing
    /// `sculpt::should_stamp` rather than a second coalescing rule), then
    /// add a `FoliageComponent`-carrying scene object at the hit through the
    /// ordinary undo-tracked command path. Returns `true` only when an
    /// object was actually added.
    fn stamp_foliage(&mut self, ctx: &mut ToolModeContext, hit: &TerrainHit) -> bool {
        let brush = ctx.state.editor.terrain.foliage.clone();
        let radius_m = brush.radius_m.max(1.0);
        if !sculpt::should_stamp(self.last_foliage_stamp_center_m, hit.position_m, radius_m) {
            return false;
        }
        let data = foliage::stamp_object_data(&brush, hit);
        let result = execute_command(
            ctx.state,
            SceneCommand::AddObject {
                data,
                parent_id: None,
            },
        );
        if result.changed {
            self.last_foliage_stamp_center_m = Some(hit.position_m);
        } else {
            tracing::warn!(
                reason = result.no_op_reason,
                "foliage stamp was rejected"
            );
        }
        result.changed
    }
}

fn mode_color(mode: SculptMode) -> [f32; 4] {
    match mode {
        SculptMode::Raise => RAISE_COLOR,
        SculptMode::Lower => LOWER_COLOR,
        SculptMode::Flatten => FLATTEN_COLOR,
        SculptMode::Paint => PAINT_COLOR,
    }
}

/// Engine-side target → the editor domain's display form.
fn editor_target(target: PlanetTarget) -> TerrainTarget {
    match target {
        PlanetTarget::Planet(_) => TerrainTarget::Planet(target.to_hex()),
        PlanetTarget::Volume(_) => TerrainTarget::Volume(target.to_hex()),
    }
}

impl ToolMode for TerrainMode {
    fn id(&self) -> ToolModeId {
        ToolModeId::TERRAIN
    }

    fn label_key(&self) -> &'static str {
        "LevelEditor.ToolMode.Terrain"
    }

    fn icon(&self) -> ui::IconName {
        ui::IconName::Globe
    }

    fn description_key(&self) -> &'static str {
        "LevelEditor.ToolMode.TerrainDesc"
    }

    fn on_mode_entered(&mut self, ctx: &mut ToolModeContext) {
        self.last_stamp_center_m = None;
        self.last_foliage_stamp_center_m = None;
        // Adopt whichever planet the runtime has, so the status bar is honest
        // before the user's first click.
        if let Some(api) = ctx.terrain {
            if let Some(target) = api.default_target() {
                ctx.state.editor.terrain.set_target(editor_target(target));
            }
        }
    }

    fn on_mode_exited(&mut self, ctx: &mut ToolModeContext) {
        // Leaving mid-drag must not strand an open stroke: drop both the
        // domain's stroke marker and the undo anchor together.
        ctx.state.editor.terrain.end_stroke();
        ctx.state.editor.terrain_undo.abort_stroke();
        self.last_stamp_center_m = None;
        self.last_foliage_stamp_center_m = None;
        self.clear_cursor(ctx.terrain);
    }

    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        self.cursor
    }

    fn layout(&self) -> ModeLayout {
        // Sculpt + foliage controls (create-world action, sculpt-mode picker,
        // radius/strength/falloff, foliage toggle + its own three sliders) are
        // too many to sit comfortably in the horizontal toolbar strip — they
        // move to a dedicated left-hand panel instead. The right dock stays:
        // picking objects and inspecting World Settings while sculpting is a
        // normal part of the workflow (e.g. selecting the planet object).
        ModeLayout {
            show_right_dock: true,
            show_mode_panel: true,
        }
    }

    fn toolbar_controls(&self, _ctx: &ToolModeContext) -> Vec<ToolWidget> {
        // `layout()` sets `show_mode_panel: true` unconditionally, so the
        // toolbar never renders this mode's widgets (`ui/toolbar/mod.rs`
        // skips the call entirely) — all of Terrain's real content lives in
        // `panel_tabs` instead, organized into Sculpt/Foliage tabs. Empty,
        // not removed: `ToolMode::toolbar_controls` has no default that
        // would make omitting the method itself meaningful, and a future
        // mode auditing "what does every mode put in the toolbar" should see
        // an explicit, documented empty answer rather than infer one.
        Vec::new()
    }

    fn panel_tabs(&self, ctx: &ToolModeContext) -> Vec<PanelTab> {
        let terrain = &ctx.state.editor.terrain;

        // Shown at the top of both tabs (not tab-specific) so the user can
        // switch which brush a click fires without hunting for a control
        // that lives on only one page. See `on_pointer`/`TerrainDomain::
        // paint_foliage` for what this actually switches.
        let brush_switch = ToolWidget::Toggle {
            id: super::dispatcher::PAINT_FOLIAGE_TOGGLE,
            label_key: "LevelEditor.Terrain.PaintFoliage",
            on: terrain.paint_foliage,
        };

        let sculpt = &terrain.sculpt;
        let selected_mode_str = match sculpt.mode {
            SculptMode::Raise => "raise",
            SculptMode::Lower => "lower",
            SculptMode::Flatten => "flatten",
            SculptMode::Paint => "paint",
        };
        let selected_shape_str = match sculpt.shape {
            BrushShape::Sphere => "sphere",
            BrushShape::Box => "box",
        };
        let sculpt_tab = PanelTab {
            id: "sculpt",
            label_key: "LevelEditor.Terrain.Tab.Sculpt",
            widgets: vec![
                brush_switch.clone(),
                ToolWidget::Divider,
                ToolWidget::Action {
                    id: super::dispatcher::CREATE_FLAT_WORLD,
                    label_key: "LevelEditor.Terrain.CreateFlatWorld",
                },
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.Brush",
                },
                ToolWidget::Segmented {
                    id: "sculpt_mode",
                    options: vec![
                        ("LevelEditor.Terrain.Raise", "raise"),
                        ("LevelEditor.Terrain.Lower", "lower"),
                        ("LevelEditor.Terrain.Flatten", "flatten"),
                        ("LevelEditor.Terrain.Paint", "paint"),
                    ],
                    selected: selected_mode_str,
                },
                // Only meaningful for Raise/Lower/Paint -- Flatten always
                // picks the shape that matches the body being levelled (see
                // `sculpt.rs`'s `build_stamp`), so this has no effect there.
                // Shown regardless of mode rather than hidden/disabled: a
                // widget that vanishes based on another widget's value is a
                // worse surprise than one that is occasionally a no-op.
                ToolWidget::Segmented {
                    id: "brush_shape",
                    options: vec![
                        ("LevelEditor.Terrain.Shape.Sphere", "sphere"),
                        ("LevelEditor.Terrain.Shape.Box", "box"),
                    ],
                    selected: selected_shape_str,
                },
                ToolWidget::Slider {
                    id: "radius",
                    label_key: "LevelEditor.Terrain.Radius",
                    value: sculpt.radius_m,
                    min: 1.0,
                    max: 64.0,
                    step: 0.5,
                },
                ToolWidget::Slider {
                    id: "strength",
                    label_key: "LevelEditor.Terrain.Strength",
                    value: sculpt.strength,
                    min: 0.1,
                    max: 10.0,
                    step: 0.1,
                },
                ToolWidget::Slider {
                    id: "falloff",
                    label_key: "LevelEditor.Terrain.Falloff",
                    value: sculpt.falloff,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                },
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.Material",
                },
                ToolWidget::Slider {
                    id: "material",
                    label_key: "LevelEditor.Terrain.Material",
                    value: sculpt.material as f32,
                    min: 1.0,
                    max: 15.0,
                    step: 1.0,
                },
            ],
        };

        let foliage = &terrain.foliage;
        let foliage_tab = PanelTab {
            id: "foliage",
            label_key: "LevelEditor.Terrain.Tab.Foliage",
            widgets: vec![
                brush_switch,
                ToolWidget::Divider,
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.General",
                },
                ToolWidget::Slider {
                    id: "foliage_density",
                    label_key: "LevelEditor.Terrain.FoliageDensity",
                    value: foliage.density,
                    min: 0.0,
                    max: 2048.0,
                    step: 1.0,
                },
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.Placement",
                },
                ToolWidget::Slider {
                    id: "foliage_radius",
                    label_key: "LevelEditor.Terrain.Radius",
                    value: foliage.radius_m,
                    min: 1.0,
                    max: 64.0,
                    step: 0.5,
                },
                ToolWidget::Slider {
                    id: "foliage_slope_min",
                    label_key: "LevelEditor.Terrain.FoliageSlopeMin",
                    value: foliage.slope_limit.0,
                    min: 0.0,
                    max: 90.0,
                    step: 0.5,
                },
                ToolWidget::Slider {
                    id: "foliage_slope_max",
                    label_key: "LevelEditor.Terrain.FoliageSlopeMax",
                    value: foliage.slope_limit.1,
                    min: 0.0,
                    max: 90.0,
                    step: 0.5,
                },
                ToolWidget::Slider {
                    id: "foliage_height_min",
                    label_key: "LevelEditor.Terrain.FoliageHeightMin",
                    value: foliage.height_range.0,
                    min: 0.01,
                    max: 10.0,
                    step: 0.01,
                },
                ToolWidget::Slider {
                    id: "foliage_height_max",
                    label_key: "LevelEditor.Terrain.FoliageHeightMax",
                    value: foliage.height_range.1,
                    min: 0.01,
                    max: 10.0,
                    step: 0.01,
                },
                ToolWidget::Slider {
                    id: "foliage_width_min",
                    label_key: "LevelEditor.Terrain.FoliageWidthMin",
                    value: foliage.width_range.0,
                    min: 0.001,
                    max: 5.0,
                    step: 0.001,
                },
                ToolWidget::Slider {
                    id: "foliage_width_max",
                    label_key: "LevelEditor.Terrain.FoliageWidthMax",
                    value: foliage.width_range.1,
                    min: 0.001,
                    max: 5.0,
                    step: 0.001,
                },
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.Rendering",
                },
                ToolWidget::Toggle {
                    id: "foliage_two_sided",
                    label_key: "LevelEditor.Terrain.FoliageTwoSided",
                    on: foliage.two_sided,
                },
                ToolWidget::Toggle {
                    id: "foliage_casts_shadow",
                    label_key: "LevelEditor.Terrain.FoliageCastsShadow",
                    on: foliage.casts_shadow,
                },
                ToolWidget::Slider {
                    id: "foliage_roughness",
                    label_key: "LevelEditor.Terrain.FoliageRoughness",
                    value: foliage.roughness,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                },
                ToolWidget::Slider {
                    id: "foliage_metallic",
                    label_key: "LevelEditor.Terrain.FoliageMetallic",
                    value: foliage.metallic,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                },
                ToolWidget::Slider {
                    id: "foliage_lod_distance",
                    label_key: "LevelEditor.Terrain.FoliageLodDistance",
                    value: foliage.lod_distance,
                    min: 1.0,
                    max: 500.0,
                    step: 1.0,
                },
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.Wind",
                },
                ToolWidget::Toggle {
                    id: "foliage_wind_enabled",
                    label_key: "LevelEditor.Terrain.FoliageWindEnabled",
                    on: foliage.wind_enabled,
                },
                ToolWidget::Slider {
                    id: "foliage_trunk_sway",
                    label_key: "LevelEditor.Terrain.FoliageTrunkSway",
                    value: foliage.trunk_sway,
                    min: 0.0,
                    max: 5.0,
                    step: 0.05,
                },
                ToolWidget::Slider {
                    id: "foliage_branch_flutter",
                    label_key: "LevelEditor.Terrain.FoliageBranchFlutter",
                    value: foliage.branch_flutter,
                    min: 0.0,
                    max: 5.0,
                    step: 0.05,
                },
                ToolWidget::Slider {
                    id: "foliage_leaf_jitter",
                    label_key: "LevelEditor.Terrain.FoliageLeafJitter",
                    value: foliage.leaf_jitter,
                    min: 0.0,
                    max: 5.0,
                    step: 0.05,
                },
                ToolWidget::Slider {
                    id: "foliage_wind_speed",
                    label_key: "LevelEditor.Terrain.FoliageWindSpeed",
                    value: foliage.wind_speed,
                    min: 0.0,
                    max: 20.0,
                    step: 0.1,
                },
                ToolWidget::Section {
                    label_key: "LevelEditor.Terrain.Section.Interaction",
                },
                ToolWidget::Slider {
                    id: "foliage_interactor_radius",
                    label_key: "LevelEditor.Terrain.FoliageInteractorRadius",
                    value: foliage.interactor_radius,
                    min: 0.0,
                    max: 10.0,
                    step: 0.1,
                },
            ],
        };

        vec![sculpt_tab, foliage_tab]
    }

    fn contributes_panels(&self) -> Vec<ModePanelDescriptor> {
        // Declarative half of the mode's own dock contributions — ids, tabs,
        // placements. The GPUI half lives in `super::panels`; see that file
        // and the design doc's §11 for why the two are split.
        layout::contributed_panels()
    }

    fn build_panel(
        &self,
        state: std::sync::Arc<parking_lot::RwLock<crate::level_editor::state::LevelEditorState>>,
        panel: &ModePanelDescriptor,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<Box<dyn ui::dock::PanelView>> {
        if panel.id != layout::TERRAIN_PALETTE {
            return None;
        }
        let view = cx.new(|cx| panels::TerrainPalettePanel::new(state.clone(), window, cx));
        Some(Box::new(view) as Box<dyn ui::dock::PanelView>)
    }

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let terrain = &ctx.state.editor.terrain;
        let text = if terrain.paint_foliage {
            format!(
                "Foliage | Radius: {:.1}m | Density: {:.0}",
                terrain.foliage.radius_m, terrain.foliage.density
            )
        } else {
            format!(
                "Radius: {:.1}m | Strength: {:.1}",
                terrain.sculpt.radius_m, terrain.sculpt.strength
            )
        };
        // Read from the domain's target rather than from `ctx.terrain`: the
        // toolbar and status bar build a context without the seam (they only
        // need widget data), and a seam-derived readout would flicker between
        // "active" and "no runtime" depending on which caller rendered it.
        // `on_mode_entered`/`begin_stroke` keep the target current.
        let tooltip = match &terrain.target {
            TerrainTarget::Planet(id) => Some(format!("Editing planet {id}")),
            TerrainTarget::Volume(id) => Some(format!("Editing volume {id}")),
            TerrainTarget::None => {
                Some("No terrain target — add a PlanetTerrainComponent to the scene".to_string())
            }
        };
        Some(StatusReadout { text, tooltip })
    }

    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        let Some(api) = ctx.terrain else {
            return ToolPointerResult::PassThrough;
        };

        match event.kind {
            PointerKind::Hover => {
                // Cursor feedback only; the renderer still wants this event.
                match self.hit_at(api, ctx, event) {
                    Some(hit) => self.update_cursor(api, ctx, &hit),
                    None => self.clear_cursor(Some(api)),
                }
                ToolPointerResult::PassThrough
            }

            PointerKind::Down => {
                if event.button != Some(gpui::MouseButton::Left) {
                    return ToolPointerResult::PassThrough;
                }
                let Some(hit) = self.hit_at(api, ctx, event) else {
                    // Nothing under the brush: let the click select objects.
                    self.clear_cursor(Some(api));
                    return ToolPointerResult::PassThrough;
                };
                self.update_cursor(api, ctx, &hit);
                if ctx.state.editor.terrain.paint_foliage {
                    self.begin_foliage_stroke(ctx, &hit);
                    self.stamp_foliage(ctx, &hit);
                } else {
                    self.begin_stroke(api, ctx, &hit);
                    self.stamp(api, ctx, &hit);
                }
                ToolPointerResult::Consumed
            }

            PointerKind::Drag => {
                if ctx.state.editor.terrain.active_stroke.is_none() {
                    return ToolPointerResult::PassThrough;
                }
                let Some(hit) = self.hit_at(api, ctx, event) else {
                    // Dragged off the planet: hold the stroke open (the user
                    // may drag back on) but stop drawing a ring nowhere.
                    self.clear_cursor(Some(api));
                    return ToolPointerResult::Consumed;
                };
                self.update_cursor(api, ctx, &hit);
                if ctx.state.editor.terrain.paint_foliage {
                    self.stamp_foliage(ctx, &hit);
                } else {
                    self.stamp(api, ctx, &hit);
                }
                ToolPointerResult::Consumed
            }

            PointerKind::Up => {
                if ctx.state.editor.terrain.active_stroke.is_none() {
                    return ToolPointerResult::PassThrough;
                }
                self.end_stroke(ctx);
                ToolPointerResult::Consumed
            }

            PointerKind::Scroll { .. } => ToolPointerResult::PassThrough,
        }
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(self.clone())
    }
}
