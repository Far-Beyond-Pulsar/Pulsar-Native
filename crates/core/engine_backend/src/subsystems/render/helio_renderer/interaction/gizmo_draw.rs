//! Drawing the selected object's gizmo as debug geometry.
use super::drag_update::snapped_rotation_angle;
use super::math::{rotation_axis_color, rotation_axis_highlight_color, rotation_matrix};
use super::{DragState, Handle, SceneInteraction, ROTATION_SNAP};
use crate::scene::GizmoType;
use crate::subsystems::render::helio_renderer::gizmo_geometry::{rotation_grid, shapes};
use glam::{Mat3, Quat, Vec3};
use helio::{DebugBatch, Renderer};
use pulsar_scenedb::World;
use std::sync::atomic::Ordering;

/// The pose the gizmo is laid out on, in view coordinates, and which handle
/// is held or hovered.
#[derive(Clone, Copy)]
struct GizmoFrame {
    pivot: Vec3,
    basis: Mat3,
    length: f32,
    active: Option<Handle>,
    /// The axis of the rotation ring being dragged, if one is.
    active_rotation_axis: Option<usize>,
}

/// The axes drawn: only the dragged ring's, or all three.
fn rotation_axes(active_rotation_axis: Option<usize>) -> Vec<usize> {
    active_rotation_axis
        .map(|axis| vec![axis])
        .unwrap_or_else(|| vec![0, 1, 2])
}

/// `None` for a handle a rotation gizmo does not draw (only rings are).
fn handle_base_color(mode: GizmoType, handle: Handle, active: Option<Handle>) -> Option<[f32; 4]> {
    Some(if mode == GizmoType::Rotate {
        let Handle::Axis(axis) = handle else {
            return None;
        };
        rotation_axis_color(axis)
    } else if active == Some(handle) {
        [1.0, 0.8, 0.12, 1.0]
    } else {
        match handle {
            Handle::Axis(0) | Handle::Plane(0) => [0.95, 0.12, 0.09, 1.0],
            Handle::Axis(1) | Handle::Plane(1) => [0.2, 0.85, 0.12, 1.0],
            Handle::Axis(_) | Handle::Plane(_) => [0.12, 0.4, 1.0, 1.0],
            Handle::Center => [0.88, 0.9, 0.94, 1.0],
        }
    })
}

impl SceneInteraction {
    pub fn draw_gizmo(&self, renderer: &mut Renderer, world: &World) {
        let Some((_, t)) = self.selected(world) else {
            return;
        };
        let pivot = self.view.relative(t.position);
        let Some(length) = self.view.length(pivot) else {
            return;
        };
        // Keep the active rotation ring, its snap marks, and its reference
        // grid in the same starting frame used by the drag angle and bright
        // arc. Drawing those from the live object rotation makes the reference
        // frame chase each snapped update and appear to jump independently.
        let basis = self
            .drag
            .filter(|drag| drag.mode == GizmoType::Rotate)
            .map(|drag| rotation_matrix(drag.initial))
            .unwrap_or_else(|| rotation_matrix(t));
        let active_rotation_axis = self.drag.and_then(|drag| {
            (drag.mode == GizmoType::Rotate)
                .then_some(drag.handle)
                .and_then(|handle| match handle {
                    Handle::Axis(axis) => Some(axis),
                    _ => None,
                })
        });
        let frame = GizmoFrame {
            pivot,
            basis,
            length,
            active: self.drag.map(|d| d.handle).or(self.hovered),
            active_rotation_axis,
        };
        // Submit the complete widget under one lock and upload generation.
        renderer.debug_batch(|batch| {
            if self.mode == GizmoType::Rotate {
                self.draw_snap_ticks(batch, &frame);
            }
            if self.mode == GizmoType::Rotate {
                self.draw_rotation_grid(batch, &frame);
            }
            self.draw_handles(batch, &frame);
            if let Some(drag) = self.drag.filter(|drag| drag.mode == GizmoType::Rotate) {
                self.draw_rotation_arc(batch, &frame, &drag);
            }
        });
    }

    /// Dark tick marks around each rotation ring at the snap increment.
    fn draw_snap_ticks(&self, batch: &mut DebugBatch<'_>, frame: &GizmoFrame) {
        let GizmoFrame {
            pivot,
            basis,
            length,
            active_rotation_axis,
            ..
        } = *frame;
        let snap_degrees = f32::from_bits(ROTATION_SNAP.load(Ordering::Relaxed));
        let ticks_per_turn = (360.0 / snap_degrees).ceil().max(1.0) as usize;
        // Very fine custom increments should not create an unbounded
        // amount of debug geometry. Keep at most 720 visible marks.
        let stride = ticks_per_turn.div_ceil(720).max(1);
        let signs = self.quadrant_signs(pivot, basis);
        for axis in rotation_axes(active_rotation_axis) {
            let u = basis.col((axis + 1) % 3);
            let v = basis.col((axis + 2) % 3);
            for tick in (0..ticks_per_turn).step_by(stride) {
                let angle = (tick as f32 * snap_degrees).to_radians();
                let cos = angle.cos();
                let sin = angle.sin();
                if active_rotation_axis.is_none()
                    && (cos * signs[(axis + 1) % 3] < -0.0001
                        || sin * signs[(axis + 2) % 3] < -0.0001)
                {
                    continue;
                }
                let radial = u * cos + v * sin;
                let overlay = -self.view.forward * (length * 0.015);
                batch.line(
                    self.view
                        .vertex(pivot + radial * (0.825 * length) + overlay),
                    self.view
                        .vertex(pivot + radial * (0.875 * length) + overlay),
                    [0.015, 0.015, 0.015, 1.0],
                );
            }
        }
    }

    /// The reference grid behind each rotation ring.
    fn draw_rotation_grid(&self, batch: &mut DebugBatch<'_>, frame: &GizmoFrame) {
        let GizmoFrame {
            pivot,
            basis,
            length,
            active_rotation_axis,
            ..
        } = *frame;
        let signs = self.quadrant_signs(pivot, basis);
        let grid_basis = self
            .drag
            .filter(|drag| drag.mode == GizmoType::Rotate)
            .and_then(|drag| {
                let Handle::Axis(axis) = drag.handle else {
                    return None;
                };
                let snapped_angle = snapped_rotation_angle(drag.angle);
                let local_axis = [Vec3::X, Vec3::Y, Vec3::Z][axis];
                Some(basis * Mat3::from_quat(Quat::from_axis_angle(local_axis, snapped_angle)))
            })
            .unwrap_or(basis);
        for axis in rotation_axes(active_rotation_axis) {
            let color = match axis {
                0 => [0.8, 0.42, 0.38, 0.45],
                1 => [0.48, 0.75, 0.4, 0.45],
                _ => [0.4, 0.6, 0.85, 0.45],
            };
            for [a, b] in rotation_grid(axis, signs) {
                batch.line(
                    self.view.vertex(pivot + grid_basis * a * length),
                    self.view.vertex(pivot + grid_basis * b * length),
                    color,
                );
            }
        }
    }

    /// The solid handle shapes of the current mode.
    fn draw_handles(&self, batch: &mut DebugBatch<'_>, frame: &GizmoFrame) {
        let GizmoFrame {
            pivot,
            basis,
            length,
            active,
            active_rotation_axis,
        } = *frame;
        for shape in shapes(self.mode) {
            // Before a drag the three handles are quarter arcs. Once one
            // is captured, expose its entire 360-degree ring and remove
            // the other rings so the active rotation range is unambiguous.
            if let Some(axis) = active_rotation_axis {
                if shape.handle != Handle::Axis(axis) {
                    continue;
                }
            }
            if !self.handle_visible(shape.handle, pivot, basis, length) {
                continue;
            }
            let Some(base) = handle_base_color(self.mode, shape.handle, active) else {
                continue;
            };
            for tri in &shape.triangles {
                if active_rotation_axis.is_none()
                    && !self.triangle_visible(shape.handle, tri, pivot, basis)
                {
                    continue;
                }
                let [a, b, c] = tri.map(|p| pivot + basis * p * length);
                let n = (b - a).cross(c - a).normalize_or_zero();
                let shade = 0.65 + 0.35 * n.dot(Vec3::new(0.3, 0.8, 0.5).normalize()).abs();
                batch.tri(
                    self.view.vertex(a),
                    self.view.vertex(b),
                    self.view.vertex(c),
                    [base[0] * shade, base[1] * shade, base[2] * shade, base[3]],
                );
            }
        }
    }

    /// The bright band showing how far the held rotation ring has turned.
    fn draw_rotation_arc(&self, batch: &mut DebugBatch<'_>, frame: &GizmoFrame, drag: &DragState) {
        let Handle::Axis(axis) = drag.handle else {
            return;
        };
        let (pivot, length) = (frame.pivot, frame.length);
        let vertex = |v: Vec3| self.view.vertex(v);
        let snapped_angle = snapped_rotation_angle(drag.angle);
        let shown_angle = snapped_angle.clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
        let segments = ((shown_angle.abs() / std::f32::consts::TAU) * 96.0)
            .ceil()
            .max(1.0) as usize;
        let initial_basis = rotation_matrix(drag.initial);
        let axis_vector = initial_basis.col(axis);
        let u = initial_basis.col((axis + 1) % 3);
        let v = initial_basis.col((axis + 2) % 3);
        let color = rotation_axis_highlight_color(axis);
        let radial = |angle: f32| u * angle.cos() + v * angle.sin();
        for segment in 0..segments {
            let a = shown_angle * segment as f32 / segments as f32;
            let b = shown_angle * (segment + 1) as f32 / segments as f32;
            let inner_a = pivot + (radial(a) * 0.81 + axis_vector * 0.008) * length;
            let inner_b = pivot + (radial(b) * 0.81 + axis_vector * 0.008) * length;
            let outer_a = pivot + (radial(a) * 0.89 + axis_vector * 0.008) * length;
            let outer_b = pivot + (radial(b) * 0.89 + axis_vector * 0.008) * length;
            batch.tri(vertex(inner_a), vertex(inner_b), vertex(outer_b), color);
            batch.tri(vertex(inner_a), vertex(outer_b), vertex(outer_a), color);

            let inner_a = pivot + (radial(a) * 0.81 - axis_vector * 0.008) * length;
            let inner_b = pivot + (radial(b) * 0.81 - axis_vector * 0.008) * length;
            let outer_a = pivot + (radial(a) * 0.89 - axis_vector * 0.008) * length;
            let outer_b = pivot + (radial(b) * 0.89 - axis_vector * 0.008) * length;
            batch.tri(vertex(inner_a), vertex(outer_b), vertex(inner_b), color);
            batch.tri(vertex(inner_a), vertex(outer_a), vertex(outer_b), color);
        }
    }
}
