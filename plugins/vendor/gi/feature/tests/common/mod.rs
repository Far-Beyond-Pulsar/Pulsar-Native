//! What the GI tests share: buffer readback and the probes' records read
//! back on the CPU.
#![allow(dead_code)]

use glam::Vec3;
use helio_pass_gi::PROBE_WORDS;

pub fn read(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u32> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec()
}

pub fn half2(w: u32) -> [f32; 2] {
    let f = |h: u32| {
        let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 31, h & 1023);
        let v = if e == 0 {
            m as f32 * 2f32.powi(-24)
        } else {
            (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15)
        };
        if s == 1 {
            -v
        } else {
            v
        }
    };
    [f(w & 0xffff), f(w >> 16)]
}

/// Probe `index`'s irradiance / pi toward unit `n` (its L1 harmonics), its
/// state.
pub fn probe(words: &[u32], index: u32, n: Vec3) -> (Vec3, u32) {
    let at = (index * PROBE_WORDS) as usize;
    let h: Vec<[f32; 2]> = (0..6).map(|w| half2(words[at + w])).collect();
    let (r, g, b) = (
        [h[0][0], h[1][1], h[3][0], h[4][1]],
        [h[0][1], h[2][0], h[3][1], h[5][0]],
        [h[1][0], h[2][1], h[4][0], h[5][1]],
    );
    let basis = [0.282095, 0.488603 * n.y, 0.488603 * n.z, 0.488603 * n.x];
    let dot = |c: [f32; 4]| (0..4).map(|k| c[k] * basis[k]).sum::<f32>().max(0.0);
    (Vec3::new(dot(r), dot(g), dot(b)), words[at + 6])
}

/// Unit `n` packed as the tracers pack normals.
pub fn pack(n: Vec3) -> u32 {
    let e = (n * 0.5 + 0.5).clamp(Vec3::ZERO, Vec3::ONE) * 1023.0 + 0.5;
    e.x as u32 | ((e.y as u32) << 10) | ((e.z as u32) << 20)
}

/// A directional light straight down, illuminance `e` (Helio's `GpuLight`).
pub fn sun_down(e: f32) -> [u32; 32] {
    let mut light = [0u32; 32];
    light[4..8].copy_from_slice(&[
        0f32.to_bits(),
        (-1f32).to_bits(),
        0f32.to_bits(),
        0f32.to_bits(),
    ]);
    light[8..12].copy_from_slice(&[1f32.to_bits(), 1f32.to_bits(), 1f32.to_bits(), e.to_bits()]);
    light[13] = 0;
    light
}

/// A point light at `at` of `intensity` (white) and `range` (`GpuLight`).
pub fn point_light(at: Vec3, intensity: f32, range: f32) -> [u32; 32] {
    let mut light = [0u32; 32];
    light[0..4].copy_from_slice(&[
        at.x.to_bits(),
        at.y.to_bits(),
        at.z.to_bits(),
        range.to_bits(),
    ]);
    light[8..12].copy_from_slice(&[
        1f32.to_bits(),
        1f32.to_bits(),
        1f32.to_bits(),
        intensity.to_bits(),
    ]);
    light[12] = u32::MAX;
    light[13] = 1;
    light
}
