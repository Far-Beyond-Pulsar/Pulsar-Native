//! Gizmos - 3D manipulation tools for the viewport
//! 
//! This module provides interactive 3D gizmos for transforming objects:
//! - Translation (move) gizmo - 3 colored arrows for X, Y, Z axes
//! - Rotation gizmo - 3 colored rings for pitch, yaw, roll
//! - Scale gizmo - 3 colored cubes for uniform or per-axis scaling
//! - Selection highlighting - Outline/wireframe for selected objects
//!
//! Gizmos are rendered in the Bevy viewport and support:
//! - Mouse picking (ray casting)
//! - Dragging with visual feedback
//! - Snapping to grid
//! - Local vs World space modes


/// Simple 2D vector
#[derive(Clone, Copy, Debug)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

/// Simple 3D vector
#[derive(Clone, Copy, Debug)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0, z: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0, z: 0.0 };
    pub const Z: Self = Self { x: 0.0, y: 0.0, z: 1.0 };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn dot(&self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn normalize(&self) -> Self {
        let len = self.length();
        if len > 0.0 {
            Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            }
        } else {
            *self
        }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

impl std::ops::AddAssign for Vec3 {
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
        self.z += other.z;
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self {
        Self {
            x: self.x * scalar,
            y: self.y * scalar,
            z: self.z * scalar,
        }
    }
}

/// Simple quaternion for rotations
#[derive(Clone, Copy, Debug)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub fn from_rotation_x(angle: f32) -> Self {
        let half = angle * 0.5;
        Self {
            x: half.sin(),
            y: 0.0,
            z: 0.0,
            w: half.cos(),
        }
    }

    pub fn from_rotation_y(angle: f32) -> Self {
        let half = angle * 0.5;
        Self {
            x: 0.0,
            y: half.sin(),
            z: 0.0,
            w: half.cos(),
        }
    }

    pub fn from_rotation_z(angle: f32) -> Self {
        let half = angle * 0.5;
        Self {
            x: 0.0,
            y: 0.0,
            z: half.sin(),
            w: half.cos(),
        }
    }
}

impl std::ops::Mul for Quat {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self {
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            z: self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        }
    }
}

impl std::ops::Mul<Vec3> for Quat {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        let qv = Vec3::new(self.x, self.y, self.z);
        let uv = Vec3 {
            x: qv.y * v.z - qv.z * v.y,
            y: qv.z * v.x - qv.x * v.z,
            z: qv.x * v.y - qv.y * v.x,
        };
        let uuv = Vec3 {
            x: qv.y * uv.z - qv.z * uv.y,
            y: qv.z * uv.x - qv.x * uv.z,
            z: qv.x * uv.y - qv.y * uv.x,
        };
        v + (uv * (2.0 * self.w)) + (uuv * 2.0)
    }
}

/// Simple transform
#[derive(Clone, Copy, Debug)]
pub struct GizmoTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

/// Gizmo type - determines which manipulation tool is active
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoType {
    None,
    Translate,
    Rotate,
    Scale,
}

/// Gizmo axis - which axis is being manipulated
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoAxis {
    None,
    X,
    Y,
    Z,
    XY,
    XZ,
    YZ,
    XYZ, // For uniform operations
}

/// Gizmo state - tracks current interaction
#[derive(Clone, Debug)]
pub struct GizmoState {
    /// Current gizmo type
    pub gizmo_type: GizmoType,
    /// Which axis is being dragged (None if not dragging)
    pub active_axis: GizmoAxis,
    /// Is the user currently dragging a gizmo?
    pub is_dragging: bool,
    /// Starting mouse position when drag began
    pub drag_start_pos: Option<Vec2>,
    /// Starting object transform when drag began
    pub drag_start_transform: Option<GizmoTransform>,
    /// Current object being manipulated
    pub target_object_id: Option<String>,
    /// Snapping enabled
    pub snap_enabled: bool,
    /// Snap increment (for translation and scale)
    pub snap_increment: f32,
    /// Rotation snap in degrees
    pub rotation_snap: f32,
    /// Local vs World space
    pub local_space: bool,
}

/// Gizmo colors (matches industry standards)
pub struct GizmoColors {
    pub x_axis: [f32; 4],      // RGBA
    pub y_axis: [f32; 4],
    pub z_axis: [f32; 4],
    pub selected: [f32; 4],
    pub hover: [f32; 4],
}

impl Default for GizmoColors {
    fn default() -> Self {
        Self {
            x_axis: [1.0, 0.2, 0.2, 1.0],     // Red for X
            y_axis: [0.2, 1.0, 0.2, 1.0],     // Green for Y
            z_axis: [0.2, 0.5, 1.0, 1.0],     // Blue for Z
            selected: [1.0, 1.0, 0.0, 1.0],   // Yellow when selected
            hover: [1.0, 0.8, 0.0, 1.0],      // Orange when hovering
        }
    }
}

impl Default for GizmoState {
    fn default() -> Self {
        Self {
            gizmo_type: GizmoType::Translate,
            active_axis: GizmoAxis::None,
            is_dragging: false,
            drag_start_pos: None,
            drag_start_transform: None,
            target_object_id: None,
            snap_enabled: false,
            snap_increment: 0.5,
            rotation_snap: 15.0,
            local_space: false,
        }
    }
}

impl GizmoState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the gizmo type (tool)
    pub fn set_gizmo_type(&mut self, gizmo_type: GizmoType) {
        self.gizmo_type = gizmo_type;
        // Reset drag state when changing tools
        if !self.is_dragging {
            self.active_axis = GizmoAxis::None;
        }
    }

    /// Start dragging a gizmo axis
    pub fn start_drag(
        &mut self,
        axis: GizmoAxis,
        mouse_pos: Vec2,
        object_transform: GizmoTransform,
        object_id: String,
    ) {
        self.active_axis = axis;
        self.is_dragging = true;
        self.drag_start_pos = Some(mouse_pos);
        self.drag_start_transform = Some(object_transform);
        self.target_object_id = Some(object_id);
    }

    /// Update drag (called on mouse move)
    pub fn update_drag(
        &mut self,
        current_mouse_pos: Vec2,
        camera_transform: &GizmoTransform,
    ) -> Option<GizmoTransform> {
        if !self.is_dragging {
            return None;
        }

        let start_pos = self.drag_start_pos?;
        let start_transform = self.drag_start_transform?;
        let delta = current_mouse_pos - start_pos;

        // Calculate new transform based on gizmo type and axis
        let mut new_transform = start_transform;

        match self.gizmo_type {
            GizmoType::Translate => {
                // Translation: mouse delta maps to world space movement
                let movement = self.calculate_translation(delta, camera_transform);
                new_transform.translation += movement;

                // Apply snapping if enabled
                if self.snap_enabled {
                    new_transform.translation = self.snap_translation(new_transform.translation);
                }
            }
            GizmoType::Rotate => {
                // Rotation: mouse X delta rotates around the active axis
                let rotation = self.calculate_rotation(delta);
                new_transform.rotation = rotation * start_transform.rotation;

                // Apply snapping if enabled (simplified - no Euler conversion for now)
                if self.snap_enabled {
                    // TODO: Implement proper Euler angle snapping
                }
            }
            GizmoType::Scale => {
                // Scale: mouse Y delta scales along the active axis
                let scale_delta = delta.y * 0.01;
                let scale_factor = 1.0 + scale_delta;

                match self.active_axis {
                    GizmoAxis::X => new_transform.scale.x = start_transform.scale.x * scale_factor,
                    GizmoAxis::Y => new_transform.scale.y = start_transform.scale.y * scale_factor,
                    GizmoAxis::Z => new_transform.scale.z = start_transform.scale.z * scale_factor,
                    GizmoAxis::XYZ => new_transform.scale = start_transform.scale * scale_factor,
                    _ => {}
                }

                // Apply snapping if enabled
                if self.snap_enabled {
                    new_transform.scale = self.snap_scale(new_transform.scale);
                }
            }
            GizmoType::None => {}
        }

        Some(new_transform)
    }

    /// End drag
    pub fn end_drag(&mut self) {
        self.is_dragging = false;
        self.active_axis = GizmoAxis::None;
        self.drag_start_pos = None;
        self.drag_start_transform = None;
    }

    /// Calculate translation from mouse delta
    fn calculate_translation(&self, delta: Vec2, camera_transform: &GizmoTransform) -> Vec3 {
        // Sensitivity factor
        let sensitivity = 0.01;

        match self.active_axis {
            GizmoAxis::X => Vec3::new(delta.x * sensitivity, 0.0, 0.0),
            GizmoAxis::Y => Vec3::new(0.0, -delta.y * sensitivity, 0.0), // Inverted for intuitive up/down
            GizmoAxis::Z => Vec3::new(0.0, 0.0, -delta.y * sensitivity),
            GizmoAxis::XY => Vec3::new(delta.x * sensitivity, -delta.y * sensitivity, 0.0),
            GizmoAxis::XZ => Vec3::new(delta.x * sensitivity, 0.0, -delta.y * sensitivity),
            GizmoAxis::YZ => Vec3::new(0.0, delta.x * sensitivity, -delta.y * sensitivity),
            _ => Vec3::ZERO,
        }
    }

    /// Calculate rotation from mouse delta
    fn calculate_rotation(&self, delta: Vec2) -> Quat {
        // Sensitivity factor
        let sensitivity = 0.01;
        let angle = delta.x * sensitivity;

        match self.active_axis {
            GizmoAxis::X => Quat::from_rotation_x(angle),
            GizmoAxis::Y => Quat::from_rotation_y(angle),
            GizmoAxis::Z => Quat::from_rotation_z(angle),
            _ => Quat::IDENTITY,
        }
    }

    /// Snap translation to grid
    fn snap_translation(&self, position: Vec3) -> Vec3 {
        Vec3::new(
            (position.x / self.snap_increment).round() * self.snap_increment,
            (position.y / self.snap_increment).round() * self.snap_increment,
            (position.z / self.snap_increment).round() * self.snap_increment,
        )
    }

    /// Snap scale to increment
    fn snap_scale(&self, scale: Vec3) -> Vec3 {
        Vec3::new(
            (scale.x / self.snap_increment).round() * self.snap_increment,
            (scale.y / self.snap_increment).round() * self.snap_increment,
            (scale.z / self.snap_increment).round() * self.snap_increment,
        )
    }

    /// Snap angle to rotation increment
    fn snap_angle(&self, angle_degrees: f32) -> f32 {
        (angle_degrees / self.rotation_snap).round() * self.rotation_snap
    }

    /// Toggle snapping
    pub fn toggle_snap(&mut self) {
        self.snap_enabled = !self.snap_enabled;
    }

    /// Toggle local/world space
    pub fn toggle_space(&mut self) {
        self.local_space = !self.local_space;
    }

    /// Check if a point intersects with a gizmo axis (for mouse picking)
    pub fn raycast_gizmo(
        &self,
        ray_origin: Vec3,
        ray_direction: Vec3,
        gizmo_position: Vec3,
        gizmo_rotation: Quat,
    ) -> Option<GizmoAxis> {
        // Simple distance-based picking for now
        // TODO: Implement proper ray-cylinder/ray-sphere intersection

        let threshold = 0.3; // Click distance threshold

        match self.gizmo_type {
            GizmoType::Translate => {
                // Check each arrow axis
                let x_axis = if self.local_space {
                    gizmo_rotation * Vec3::X
                } else {
                    Vec3::X
                };
                let y_axis = if self.local_space {
                    gizmo_rotation * Vec3::Y
                } else {
                    Vec3::Y
                };
                let z_axis = if self.local_space {
                    gizmo_rotation * Vec3::Z
                } else {
                    Vec3::Z
                };

                // Test each axis arrow
                if self.ray_intersects_arrow(ray_origin, ray_direction, gizmo_position, x_axis, threshold) {
                    return Some(GizmoAxis::X);
                }
                if self.ray_intersects_arrow(ray_origin, ray_direction, gizmo_position, y_axis, threshold) {
                    return Some(GizmoAxis::Y);
                }
                if self.ray_intersects_arrow(ray_origin, ray_direction, gizmo_position, z_axis, threshold) {
                    return Some(GizmoAxis::Z);
                }
            }
            GizmoType::Rotate => {
                // Check each rotation ring
                // Simplified: check distance to circle on each plane
                if self.ray_intersects_circle(ray_origin, ray_direction, gizmo_position, Vec3::X, 1.0, threshold) {
                    return Some(GizmoAxis::X);
                }
                if self.ray_intersects_circle(ray_origin, ray_direction, gizmo_position, Vec3::Y, 1.0, threshold) {
                    return Some(GizmoAxis::Y);
                }
                if self.ray_intersects_circle(ray_origin, ray_direction, gizmo_position, Vec3::Z, 1.0, threshold) {
                    return Some(GizmoAxis::Z);
                }
            }
            GizmoType::Scale => {
                // Check each scale handle (similar to translate)
                let x_axis = Vec3::X;
                let y_axis = Vec3::Y;
                let z_axis = Vec3::Z;

                if self.ray_intersects_arrow(ray_origin, ray_direction, gizmo_position, x_axis, threshold) {
                    return Some(GizmoAxis::X);
                }
                if self.ray_intersects_arrow(ray_origin, ray_direction, gizmo_position, y_axis, threshold) {
                    return Some(GizmoAxis::Y);
                }
                if self.ray_intersects_arrow(ray_origin, ray_direction, gizmo_position, z_axis, threshold) {
                    return Some(GizmoAxis::Z);
                }

                // Check center cube for uniform scaling
                if (gizmo_position - ray_origin).length() < threshold * 2.0 {
                    return Some(GizmoAxis::XYZ);
                }
            }
            GizmoType::None => {}
        }

        None
    }

    /// Helper: Ray-arrow intersection test
    fn ray_intersects_arrow(
        &self,
        ray_origin: Vec3,
        ray_direction: Vec3,
        arrow_base: Vec3,
        arrow_direction: Vec3,
        threshold: f32,
    ) -> bool {
        // Simplified: distance from ray to line segment
        let arrow_end = arrow_base + arrow_direction * 2.0;
        
        // Closest point on ray to arrow
        let v = arrow_end - arrow_base;
        let w = ray_origin - arrow_base;
        let c1 = w.dot(v);
        let c2 = v.dot(v);
        
        if c2 == 0.0 {
            return false;
        }
        
        let b = c1 / c2;
        if b < 0.0 || b > 1.0 {
            return false; // Outside arrow segment
        }
        
        let point_on_arrow = arrow_base + (v * b);
        let closest_point_on_ray = ray_origin + (ray_direction * ((point_on_arrow - ray_origin).dot(ray_direction)));
        
        let distance = (point_on_arrow - closest_point_on_ray).length();
        distance < threshold
    }

    /// Helper: Ray-circle intersection test
    fn ray_intersects_circle(
        &self,
        ray_origin: Vec3,
        ray_direction: Vec3,
        circle_center: Vec3,
        circle_normal: Vec3,
        circle_radius: f32,
        threshold: f32,
    ) -> bool {
        // Ray-plane intersection
        let denom = ray_direction.dot(circle_normal);
        if denom.abs() < 0.0001 {
            return false; // Ray parallel to plane
        }
        
        let t = (circle_center - ray_origin).dot(circle_normal) / denom;
        if t < 0.0 {
            return false; // Behind ray origin
        }
        
        let intersection_point = ray_origin + ray_direction * t;
        let distance_to_center = (intersection_point - circle_center).length();
        
        // Check if point is on the circle (within threshold)
        (distance_to_center - circle_radius).abs() < threshold
    }
}

/// Gizmo rendering data structure (will be sent to Bevy)
#[derive(Clone, Debug)]
pub struct GizmoRenderData {
    pub gizmo_type: GizmoType,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
    pub active_axis: GizmoAxis,
    pub local_space: bool,
}

impl GizmoRenderData {
    pub fn new(
        gizmo_type: GizmoType,
        position: Vec3,
        rotation: Quat,
        scale: f32,
        active_axis: GizmoAxis,
        local_space: bool,
    ) -> Self {
        Self {
            gizmo_type,
            position,
            rotation,
            scale,
            active_axis,
            local_space,
        }
    }
}
