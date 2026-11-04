//! Scene generators shared by the reference and GPU tests.
#![allow(dead_code)]

use helio_pass_gbuffer::StaticObjectRow;
use helio_vg_format::{GpuVgInstance, INST_ENABLED};

pub struct Rng(pub u64);
impl Rng {
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

pub fn object(center: [f32; 3], radius: f32) -> StaticObjectRow {
    let mut t = [[0.0; 4]; 4];
    for i in 0..4 {
        t[i][i] = 1.0;
    }
    t[3][0] = center[0];
    t[3][1] = center[1];
    t[3][2] = center[2];
    StaticObjectRow {
        mesh_slot: 0,
        mesh_generation: 1,
        material_slot: 0,
        material_generation: 1,
        transform: t,
        prev_transform: t,
        normal_mat: [[0.0; 4]; 3],
        bounds: [center[0], center[1], center[2], radius],
        index_count: 3,
        first_index: 0,
        vertex_offset: 0,
        material_class: 0,
        graph_hash_lo: 0,
        graph_hash_hi: 0,
        flags: 0,
        skin_vertex_offset: 0,
        bone_palette_offset: 0,
        bone_palette_count: 0,
        _skin_pad: 0,
    }
}

pub fn dead_object() -> StaticObjectRow {
    StaticObjectRow {
        mesh_generation: 0,
        ..object([0.0; 3], 0.0)
    }
}

pub fn instance(object_row: u32) -> GpuVgInstance {
    GpuVgInstance {
        asset_slot: 0,
        flags: INST_ENABLED,
        lod_error_scale: 1.0,
        object_row,
    }
}

/// `n` instances at random positions in a `size`-wide cube, radii in
/// `[r_lo, r_hi)`; every 11th instance row is disabled and every 13th object
/// row is dead, so the compaction has holes to deal with. Instance rows
/// reference object rows through a shuffle-ish stride so `object_row != row`.
pub fn random_scene(
    seed: u64,
    n: usize,
    size: f32,
    r_lo: f32,
    r_hi: f32,
) -> (Vec<GpuVgInstance>, Vec<StaticObjectRow>) {
    let mut rng = Rng(seed);
    let mut objects = Vec::with_capacity(n);
    for i in 0..n {
        let c = [rng.unit() * size, rng.unit() * size, rng.unit() * size];
        let r = r_lo + rng.unit() * (r_hi - r_lo);
        objects.push(if i % 13 == 7 {
            dead_object()
        } else {
            object(c, r)
        });
    }
    let mut instances = Vec::with_capacity(n);
    for i in 0..n {
        let mut inst = instance(((i * 7 + 3) % n.max(1)) as u32);
        if i % 11 == 5 {
            inst.flags = 0;
        }
        instances.push(inst);
    }
    (instances, objects)
}

/// Dense cluster of tiny objects (batch-leaf candidates) plus a few big ones.
pub fn city_scene(
    seed: u64,
    blocks: usize,
    per_block: usize,
) -> (Vec<GpuVgInstance>, Vec<StaticObjectRow>) {
    let mut rng = Rng(seed);
    let mut objects = Vec::new();
    for b in 0..blocks {
        let origin = [(b % 8) as f32 * 40.0, 0.0, (b / 8) as f32 * 40.0];
        for _ in 0..per_block {
            let c = [
                origin[0] + rng.unit() * 2.0,
                origin[1] + rng.unit() * 2.0,
                origin[2] + rng.unit() * 2.0,
            ];
            objects.push(object(c, 0.05 + rng.unit() * 0.1));
        }
        objects.push(object([origin[0] + 10.0, 5.0, origin[2] + 10.0], 6.0)); // a building
    }
    let instances = (0..objects.len()).map(|i| instance(i as u32)).collect();
    (instances, objects)
}
