//! Sample sculpt and material-paint mode for voxel objects (SceneDB voxel
//! sources). Voxel worlds are sculpted with brushes (the voxel sculpt mode).

pub mod panels;

use std::sync::Arc;

use engine_backend::scene::{attachments, voxel_source::VoxelSourceSession, Transform};
use gpui::{AppContext, MouseButton};
use helio_component::{VoxelComponent, VoxelPayloadStore};
use helio_voxel_data::{VoxelEditClose, VoxelSampleEdit, VoxelSourceId, VOXEL_CHUNK_ENCODING_RAW};
use parking_lot::Mutex;
use pulsar_scenedb::Entity;
use rust_i18n::t;

use super::{
    BrushCursor, CameraFrame, ModeLayout, PointerKind, StatusReadout, ToolMode, ToolModeContext,
    ToolModeId, ToolPointerEvent, ToolPointerResult, ToolWidget, ViewportFrame,
};
use crate::state::{
    terrain::{BrushShape, SculptMode, TerrainTarget},
    LevelEditorState,
};

const SOURCE_ID: VoxelSourceId = VoxelSourceId(0x5445_5252_4149_4e01);
const MAX_BRUSH_RADIUS_SAMPLES: i64 = 19;

#[derive(Clone)]
struct SourceBounds {
    entity: Entity,
    origin: [f32; 3],
    voxel_size: f32,
    material_slots: u8,
    payloads: VoxelPayloadStore,
    min_sample: [i64; 3],
    max_sample: [i64; 3],
}

#[derive(Clone, Copy)]
struct Ray {
    origin: glam::Vec3,
    direction: glam::Vec3,
}

/// The session is shared when the mode is cloned for panel construction.
/// The active brush stroke owns it until mouse-up or a mode switch.
#[derive(Clone, Default)]
pub struct TerrainMode {
    cursor: Option<BrushCursor>,
    active_source: Arc<Mutex<Option<VoxelSourceSession>>>,
    stroke_source: Option<Entity>,
    flatten_plane: Option<i64>,
}

impl TerrainMode {
    fn target_from_state(
        state: &LevelEditorState,
        bounds: &[SourceBounds],
    ) -> Option<SourceBounds> {
        let id = match &state.editor.terrain.target {
            TerrainTarget::Volume(id) => id,
            TerrainTarget::None => return bounds.first().cloned(),
        };
        let bits = u64::from_str_radix(id, 16).ok()?;
        bounds
            .iter()
            .find(|source| source.entity.bits() == bits)
            .cloned()
    }

    fn sources(state: &LevelEditorState) -> Vec<SourceBounds> {
        let world = state.scene.world();
        let mut sources = Vec::new();
        for (entity, _, component) in attachments::enabled_components::<VoxelComponent>(&world) {
            if component.enabled && component.editable {
                if let Some(source) = object_bounds(&world, entity, component) {
                    sources.push(source);
                }
            }
        }
        sources
    }

    fn hit(
        &self,
        ctx: &ToolModeContext,
        event: &ToolPointerEvent,
    ) -> Option<(SourceBounds, glam::Vec3)> {
        let ray = viewport_ray(ctx.camera, ctx.viewport, event.norm_x, event.norm_y)?;
        let sources = Self::sources(ctx.state);
        let target = Self::target_from_state(ctx.state, &sources)?;
        let (min, max) = target.world_bounds()?;
        let (near, far) = ray_aabb(ray, min, max)?;
        let (sample, _distance) = ray_voxel_surface(&target, ray, near, far)?;
        let hit = glam::Vec3::from_array(target.origin)
            + glam::Vec3::new(
                sample[0] as f32 + 0.5,
                sample[1] as f32 + 0.5,
                sample[2] as f32 + 0.5,
            ) * target.voxel_size;
        Some((target, hit))
    }

    fn begin_stroke(&mut self, source: &SourceBounds, ctx: &ToolModeContext) -> bool {
        let scene = ctx.state.scene.shared_scene();
        match VoxelSourceSession::open(scene, source.entity, SOURCE_ID) {
            Ok(session) => {
                *self.active_source.lock() = Some(session);
                self.stroke_source = Some(source.entity);
                true
            }
            Err(error) => {
                tracing::warn!(%error, "could not open voxel source for terrain stroke");
                false
            }
        }
    }

    fn end_stroke(&mut self) {
        self.stroke_source = None;
        self.flatten_plane = None;
        if let Some(session) = self.active_source.lock().take() {
            std::thread::spawn(move || {
                let _ = session.finish(VoxelEditClose::Drain);
            });
        }
    }

    fn stamp(&self, source: SourceBounds, hit: glam::Vec3, ctx: &ToolModeContext) {
        let terrain = &ctx.state.editor.terrain;
        let brush = terrain.sculpt;
        let voxel = source.voxel_size.max(0.0001);
        let radius = (brush.radius_m / voxel)
            .ceil()
            .clamp(1.0, MAX_BRUSH_RADIUS_SAMPLES as f32) as i64;
        let center = [
            ((hit.x - source.origin[0]) / voxel).floor() as i64,
            ((hit.y - source.origin[1]) / voxel).floor() as i64,
            ((hit.z - source.origin[2]) / voxel).floor() as i64,
        ];
        if source.material_slots == 0 && brush.mode != SculptMode::Lower {
            return;
        }
        let slot = match brush.mode {
            SculptMode::Lower => 0,
            SculptMode::Raise | SculptMode::Flatten | SculptMode::Paint => {
                brush.material.clamp(1, u32::from(source.material_slots)) as u8
            }
        };
        let mut edits = Vec::new();
        if brush.mode == SculptMode::Flatten {
            let plane = self.flatten_plane.unwrap_or(center[1]);
            for z in -radius..=radius {
                for x in -radius..=radius {
                    let distance_sq = (x * x + z * z) as f32;
                    let inside = match brush.shape {
                        BrushShape::Sphere => distance_sq <= (radius * radius) as f32,
                        BrushShape::Box => true,
                    };
                    if !inside {
                        continue;
                    }
                    let falloff = 1.0 - distance_sq.sqrt() / radius as f32;
                    if falloff < (1.0 - brush.strength.clamp(0.1, 10.0) / 10.0) * brush.falloff {
                        continue;
                    }
                    for y in -radius..=radius {
                        let xyz = [center[0] + x, center[1] + y, center[2] + z];
                        if xyz.iter().zip(source.min_sample).any(|(&v, lo)| v < lo)
                            || xyz.iter().zip(source.max_sample).any(|(&v, hi)| v > hi)
                        {
                            continue;
                        }
                        edits.push(VoxelSampleEdit {
                            xyz,
                            lod: 0,
                            material_slot: if xyz[1] <= plane { slot } else { 0 },
                        });
                    }
                }
            }
            self.submit_edits(edits);
            return;
        }
        for z in -radius..=radius {
            for y in -radius..=radius {
                for x in -radius..=radius {
                    let distance_sq = (x * x + y * y + z * z) as f32;
                    let inside = match brush.shape {
                        BrushShape::Sphere => distance_sq <= (radius * radius) as f32,
                        BrushShape::Box => true,
                    };
                    if !inside {
                        continue;
                    }
                    let falloff = 1.0 - (distance_sq.sqrt() / radius as f32);
                    if falloff < (1.0 - brush.strength.clamp(0.1, 10.0) / 10.0) * brush.falloff {
                        continue;
                    }
                    let xyz = [center[0] + x, center[1] + y, center[2] + z];
                    if xyz.iter().zip(source.min_sample).any(|(&v, lo)| v < lo)
                        || xyz.iter().zip(source.max_sample).any(|(&v, hi)| v > hi)
                    {
                        continue;
                    }
                    edits.push(VoxelSampleEdit {
                        xyz,
                        lod: 0,
                        material_slot: slot,
                    });
                }
            }
        }
        if edits.is_empty() {
            return;
        }
        self.submit_edits(edits);
    }

    fn submit_edits(&self, edits: Vec<VoxelSampleEdit>) {
        if edits.is_empty() {
            return;
        }
        let session = self.active_source.lock();
        let Some(session) = session.as_ref() else {
            return;
        };
        if let Err(error) = session.try_submit_edits(Arc::from(edits)) {
            tracing::warn!(%error, "voxel brush stamp was rejected");
        }
    }
}

impl ToolMode for TerrainMode {
    fn build_panels(
        &self,
        ctx: &mut super::ModePanelContext<'_, '_>,
    ) -> Vec<Arc<dyn ui::dock::PanelView>> {
        let state = ctx.state.clone();
        let foliage = {
            let window = &mut *ctx.window;
            ctx.cx
                .new(|cx| panels::FoliageSetsPanel::new(state, window, cx))
        };
        let state = ctx.state.clone();
        let terrain = {
            let window = &mut *ctx.window;
            ctx.cx
                .new(|cx| panels::TerrainPanel::new(state, foliage.clone(), window, cx))
        };
        vec![Arc::new(terrain)]
    }

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
        if matches!(ctx.state.editor.terrain.target, TerrainTarget::None) {
            if let Some(source) = Self::sources(ctx.state).first() {
                ctx.state
                    .editor
                    .terrain
                    .set_target(TerrainTarget::Volume(format!(
                        "{:016x}",
                        source.entity.bits()
                    )));
            }
        }
    }
    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {
        self.end_stroke();
        self.cursor = None;
    }
    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        self.cursor
    }
    fn layout(&self) -> ModeLayout {
        ModeLayout {
            show_right_dock: true,
        }
    }
    fn toolbar_controls(&self, ctx: &ToolModeContext) -> Vec<ToolWidget> {
        let brush = ctx.state.editor.terrain.sculpt;
        vec![
            ToolWidget::Divider,
            ToolWidget::Slider {
                id: "radius",
                label_key: "LevelEditor.Terrain.BrushRadius",
                value: brush.radius_m,
                min: 1.0,
                max: 64.0,
                step: 1.0,
            },
            ToolWidget::Slider {
                id: "strength",
                label_key: "LevelEditor.Terrain.BrushStrength",
                value: brush.strength,
                min: 0.1,
                max: 10.0,
                step: 0.1,
            },
        ]
    }
    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let terrain = &ctx.state.editor.terrain;
        let mode = match terrain.sculpt.mode {
            SculptMode::Raise => t!("LevelEditor.Terrain.Raise").to_string(),
            SculptMode::Lower => t!("LevelEditor.Terrain.Lower").to_string(),
            SculptMode::Flatten => t!("LevelEditor.Terrain.Flatten").to_string(),
            SculptMode::Paint => t!("LevelEditor.Terrain.Paint").to_string(),
        };
        Some(StatusReadout {
            text: t!(
                "LevelEditor.Terrain.Status",
                mode => mode,
                radius => format!("{:.1}", terrain.sculpt.radius_m),
                material => terrain.sculpt.material
            )
            .to_string(),
            tooltip: Some(t!("LevelEditor.Terrain.NavigationHint").to_string()),
        })
    }
    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        match event.kind {
            PointerKind::Hover => {
                if let Some((_, hit)) = self.hit(ctx, event) {
                    self.cursor = Some(BrushCursor {
                        center: hit.to_array(),
                        radius: ctx.state.editor.terrain.sculpt.radius_m,
                        color: [0.35, 0.85, 0.95, 0.9],
                    });
                } else {
                    self.cursor = None;
                }
                ToolPointerResult::PassThrough
            }
            PointerKind::Down if event.button == Some(MouseButton::Left) => {
                let Some((source, hit)) = self.hit(ctx, event) else {
                    return ToolPointerResult::PassThrough;
                };
                if !self.begin_stroke(&source, ctx) {
                    return ToolPointerResult::PassThrough;
                }
                self.flatten_plane = (ctx.state.editor.terrain.sculpt.mode == SculptMode::Flatten)
                    .then(|| ((hit.y - source.origin[1]) / source.voxel_size).floor() as i64);
                self.cursor = Some(BrushCursor {
                    center: hit.to_array(),
                    radius: ctx.state.editor.terrain.sculpt.radius_m,
                    color: [0.35, 0.85, 0.95, 0.9],
                });
                self.stamp(source, hit, ctx);
                ToolPointerResult::Consumed
            }
            PointerKind::Drag if self.stroke_source.is_some() => {
                if let Some((source, hit)) = self.hit(ctx, event) {
                    self.stamp(source, hit, ctx);
                    self.cursor = Some(BrushCursor {
                        center: hit.to_array(),
                        radius: ctx.state.editor.terrain.sculpt.radius_m,
                        color: [0.35, 0.85, 0.95, 0.9],
                    });
                }
                ToolPointerResult::Consumed
            }
            PointerKind::Up if self.stroke_source.is_some() => {
                self.end_stroke();
                ToolPointerResult::Consumed
            }
            _ => ToolPointerResult::PassThrough,
        }
    }
    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(self.clone())
    }
}

fn object_bounds(
    world: &pulsar_scenedb::World,
    entity: Entity,
    component: &VoxelComponent,
) -> Option<SourceBounds> {
    // A voxel source instance is placed by its object's transform.
    let transform = attachments::owner_component::<Transform>(world, entity)
        .copied()
        .unwrap_or_default();
    if transform
        .rotation
        .iter()
        .any(|v| !v.is_finite() || v.abs() > 1e-5)
        || transform.position.iter().any(|v| !v.is_finite())
        || component
            .dimensions
            .iter()
            .any(|&size| size == 0 || size > 256)
        || !component.voxel_size.is_finite()
        || component.voxel_size <= 0.0
    {
        return None;
    }
    let scale = transform.scale[0];
    if !scale.is_finite()
        || scale <= 0.0
        || !transform.scale[1].is_finite()
        || !transform.scale[2].is_finite()
        || (transform.scale[1] - scale).abs() > 1e-5
        || (transform.scale[2] - scale).abs() > 1e-5
    {
        return None;
    }
    let voxel_size = component.voxel_size as f32 * scale;
    Some(SourceBounds {
        entity,
        origin: transform.position.map(|v| v as f32),
        voxel_size,
        material_slots: component.material_ids.len().min(usize::from(u8::MAX)) as u8,
        payloads: component.payload_store(),
        min_sample: [0; 3],
        max_sample: component.dimensions.map(|n| i64::from(n) - 1),
    })
}

impl SourceBounds {
    fn world_bounds(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        if !self.voxel_size.is_finite() || self.voxel_size <= 0.0 {
            return None;
        }
        let min = glam::Vec3::from_array(self.origin)
            + glam::Vec3::new(
                self.min_sample[0] as f32,
                self.min_sample[1] as f32,
                self.min_sample[2] as f32,
            ) * self.voxel_size;
        let max = glam::Vec3::from_array(self.origin)
            + (glam::Vec3::new(
                self.max_sample[0] as f32,
                self.max_sample[1] as f32,
                self.max_sample[2] as f32,
            ) + glam::Vec3::ONE)
                * self.voxel_size;
        Some((min, max))
    }
}

fn viewport_ray(
    camera: CameraFrame,
    viewport: ViewportFrame,
    norm_x: f32,
    norm_y: f32,
) -> Option<Ray> {
    if viewport.width <= 0.0 || viewport.height <= 0.0 {
        return None;
    }
    let (sy, cy) = camera.yaw.sin_cos();
    let (sp, cp) = camera.pitch.sin_cos();
    let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
    let position = glam::Vec3::from_array(camera.position);
    let projection = glam::Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_4,
        viewport.width / viewport.height,
        0.1,
        10_000.0,
    );
    let view = glam::Mat4::look_at_rh(position, position + forward, glam::Vec3::Y);
    let inverse = (projection * view).inverse();
    let x = norm_x.clamp(0.0, 1.0) * 2.0 - 1.0;
    let y = 1.0 - norm_y.clamp(0.0, 1.0) * 2.0;
    let near = inverse.project_point3(glam::Vec3::new(x, y, 0.0));
    let far = inverse.project_point3(glam::Vec3::new(x, y, 1.0));
    let direction = (far - near).normalize_or_zero();
    (direction != glam::Vec3::ZERO).then_some(Ray {
        origin: near,
        direction,
    })
}

fn ray_aabb(ray: Ray, min: glam::Vec3, max: glam::Vec3) -> Option<(f32, f32)> {
    let mut near = 0.0f32;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        let origin = ray.origin[axis];
        let direction = ray.direction[axis];
        if direction.abs() < 1e-6 {
            if origin < min[axis] || origin > max[axis] {
                return None;
            }
            continue;
        }
        let a = (min[axis] - origin) / direction;
        let b = (max[axis] - origin) / direction;
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    (far >= near && far > 0.0).then_some((near.max(0.0), far))
}

fn ray_voxel_surface(
    source: &SourceBounds,
    ray: Ray,
    near: f32,
    far: f32,
) -> Option<([i64; 3], f32)> {
    const EDGE: i64 = 8;
    const MAX_STEPS: usize = 200_000;
    let size = source.voxel_size;
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let start_t = near + size * 1.0e-4;
    let point = ray.origin + ray.direction * start_t;
    let local = (point - glam::Vec3::from_array(source.origin)) / size;
    let mut cell = [
        local.x.floor() as i64,
        local.y.floor() as i64,
        local.z.floor() as i64,
    ];
    let step = [
        ray.direction.x.signum() as i64,
        ray.direction.y.signum() as i64,
        ray.direction.z.signum() as i64,
    ];
    let mut next = [f32::INFINITY; 3];
    let mut delta = [f32::INFINITY; 3];
    for axis in 0..3 {
        let direction = ray.direction[axis];
        if direction.abs() < 1.0e-8 {
            continue;
        }
        let boundary_cell = if step[axis] > 0 {
            cell[axis] + 1
        } else {
            cell[axis]
        };
        let boundary = source.origin[axis] + boundary_cell as f32 * size;
        next[axis] = (boundary - ray.origin[axis]) / direction;
        delta[axis] = size / direction.abs();
    }
    let Ok(payloads) = source.payloads.read() else {
        return None;
    };
    let max_steps = ((far - start_t).max(0.0) / size * 3.0).ceil() as usize;
    for _ in 0..max_steps.min(MAX_STEPS) {
        if cell.iter().zip(source.min_sample).any(|(&v, lo)| v < lo)
            || cell.iter().zip(source.max_sample).any(|(&v, hi)| v > hi)
        {
            return None;
        }
        let key = [
            cell[0].div_euclid(EDGE) as u64,
            cell[1].div_euclid(EDGE) as u64,
            cell[2].div_euclid(EDGE) as u64,
            0,
        ];
        let index = cell[2].rem_euclid(EDGE) as usize * 64
            + cell[1].rem_euclid(EDGE) as usize * 8
            + cell[0].rem_euclid(EDGE) as usize;
        let material = payloads
            .1
            .get(&key)
            .filter(|payload| payload.encoding == VOXEL_CHUNK_ENCODING_RAW)
            .and_then(|payload| payload.bytes.get(index))
            .copied()
            .unwrap_or(0);
        if material != 0 {
            return Some((cell, start_t));
        }
        let axis = if next[0] <= next[1] && next[0] <= next[2] {
            0
        } else if next[1] <= next[2] {
            1
        } else {
            2
        };
        if next[axis] > far {
            return None;
        }
        cell[axis] += step[axis];
        next[axis] += delta[axis];
    }
    None
}
