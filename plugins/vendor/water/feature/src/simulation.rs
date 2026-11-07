// ---- GPU uniform structs (simulation parameters) ---------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DropUniform {
    pub center: [f32; 2],
    pub radius: f32,
    pub strength: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DeltaUniform {
    pub delta: [f32; 2],
    pub spring: f32,
    pub damping: f32,
    pub wind_dir: [f32; 2],
    pub wind_strength: f32,
    pub time: f32,
    pub wave_scale: f32,
    pub time_step: f32,
    pub cascade_patch_size: f32,
    pub cascade_id: u32,
    pub wave_speed: f32,
    pub _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct HitboxCountUniform {
    pub count: u32,
    pub _pad: [u32; 3],
    /// How far the world origin moved since the previous hitbox rows were
    /// captured (`xyz`, `hitbox.frag.wgsl`).
    pub previous_shift: [f32; 4],
}
