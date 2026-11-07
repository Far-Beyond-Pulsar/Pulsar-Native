//! Instance-scale benchmark for the VG museum example (`VG_STRESS_INSTANCES=<N>`
//! or `--instance-bench <N>`, together with `VG_STRESS_CAPTURE=<dir>`).
//!
//! The scene is N instances of ONE asset in a seeded, jittered grid (varied yaw
//! and scale) plus a classic ground plane. The shots are fixed camera cases
//! (whole grid from far, looking away from the grid, along the edge, inside the
//! grid, and two cases with 1% of the instances moving every frame). Everything
//! is recorded in the normal capture JSON under the `bench` key.
//!
//! Env: `VG_STRESS_INSTANCES`, `VG_STRESS_BENCH_ASSET=thai|lucy|pine`,
//! `VG_STRESS_BENCH_SPACING` (metres, default 3.2), `VG_STRESS_BENCH_SEED`,
//! `VG_STRESS_BENCH_CLASSIC=<M>` (additionally M classic instances of one small
//! quad mesh, sharing one mesh and material: the hardware-instancing check),
//! `VG_STRESS_BENCH_MEM_GB` (private-memory guard, default 14).

use crate::vg_stress_capture::{CameraSpec, Shot};
use crate::vg_stress_data::{
    layout::{Placement, LUCY, THAI},
    materials::Mat,
};
use pulsar_scenedb::{Entity, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchAsset {
    Thai,
    Lucy,
    Pine,
}

#[derive(Clone, Debug)]
pub struct BenchConfig {
    pub instances: u32,
    pub asset: BenchAsset,
    pub spacing: f32,
    pub classic: u32,
    pub seed: u64,
    pub mem_limit_gb: f64,
}

impl BenchConfig {
    pub fn from_env_and_args() -> Option<Self> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let from_arg = args
            .iter()
            .position(|a| a == "--instance-bench")
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u32>().ok());
        let instances = from_arg.or_else(|| {
            std::env::var("VG_STRESS_INSTANCES")
                .ok()?
                .trim()
                .parse()
                .ok()
        })?;
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string());
        let asset = match env("VG_STRESS_BENCH_ASSET")
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("lucy") => BenchAsset::Lucy,
            Some("pine") => BenchAsset::Pine,
            _ => BenchAsset::Thai,
        };
        Some(Self {
            instances: instances.max(1),
            asset,
            spacing: env("VG_STRESS_BENCH_SPACING")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3.2),
            classic: env("VG_STRESS_BENCH_CLASSIC")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            seed: env("VG_STRESS_BENCH_SEED")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0x9e37_79b9_7f4a_7c15),
            mem_limit_gb: env("VG_STRESS_BENCH_MEM_GB")
                .and_then(|v| v.parse().ok())
                .unwrap_or(14.0),
        })
    }

    pub fn side(&self) -> u32 {
        (f64::from(self.instances)).sqrt().ceil() as u32
    }

    /// Width of the grid in metres.
    pub fn extent(&self) -> f32 {
        self.side() as f32 * self.spacing
    }

    /// The N placements of the bench asset (all share one VG asset).
    pub fn placements(&self) -> Vec<Placement> {
        let (asset, slots): (String, Vec<(String, Mat)>) = match self.asset {
            BenchAsset::Lucy => (LUCY.asset(), vec![(LUCY.slot(), LUCY.material)]),
            BenchAsset::Pine => {
                let tree = crate::vg_stress_prep::catalog::TREES
                    .iter()
                    .find(|t| t.id == "pine_tree_01")
                    .expect("pine_tree_01 in catalog");
                let p = Placement::tree("pine", tree, 0, [0.0; 3], 0.0, 1.0);
                (p.asset, p.slots)
            }
            BenchAsset::Thai => (THAI.asset(), vec![(THAI.slot(), THAI.material)]),
        };
        let side = self.side();
        let half = self.extent() * 0.5;
        let mut rng = Rng(self.seed | 1);
        (0..self.instances)
            .map(|i| {
                let (gx, gz) = (i % side, i / side);
                let jx = (rng.next() - 0.5) * self.spacing * 0.5;
                let jz = (rng.next() - 0.5) * self.spacing * 0.5;
                let s = 0.8 + 0.4 * rng.next();
                Placement {
                    name: format!("bench {i}"),
                    folder: "Bench",
                    asset: asset.clone(),
                    slots: slots.clone(),
                    position: [
                        f64::from(gx as f32 * self.spacing - half + jx),
                        0.0,
                        f64::from(gz as f32 * self.spacing - half + jz),
                    ],
                    rotation: [0.0, rng.next() * 360.0, 0.0],
                    scale: [s; 3],
                    virtual_geometry: true,
                    triangles: 0,
                }
            })
            .collect()
    }

    /// The fixed camera cases. A `-move` suffix makes 1% of the instances move per frame.
    pub fn shots(&self) -> Vec<Shot> {
        let h = self.extent() * 0.5;
        let far = cam([0.0, h + 5.0, h * 1.7 + 20.0], [0.0, 0.0, 0.0]);
        let inside = cam([0.0, 1.7, 0.0], [0.0, 1.7, -60.0]);
        let list = [
            ("bench-far", far.clone()),
            (
                "bench-away",
                cam([-h - 10.0, 3.0, 0.0], [-h - 200.0, 3.0, 0.0]),
            ),
            ("bench-edge", cam([-h - 5.0, 2.0, 0.0], [h, 2.0, 0.0])),
            ("bench-inside", inside.clone()),
            ("bench-far-move", far),
            ("bench-inside-move", inside),
        ];
        list.into_iter()
            .map(|(name, camera)| Shot {
                name: name.into(),
                camera,
                settle_frames: Some(70),
                format: Some("jpg".into()),
                jpeg_quality: Some(60),
                ..Default::default()
            })
            .collect()
    }
}

fn cam(pos: [f32; 3], look: [f32; 3]) -> CameraSpec {
    CameraSpec {
        position: Some(pos),
        look_at: Some(look),
        fov_deg: Some(50.0),
        ..Default::default()
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Grid transform of classic instance `i` (a flat quad lying on the ground).
pub fn classic_transform(i: u32, side: u32, spacing: f32, origin: glam::Vec3) -> glam::Mat4 {
    let p = origin
        + glam::Vec3::new(
            (i % side) as f32 * spacing,
            0.0,
            (i / side) as f32 * spacing,
        );
    glam::Mat4::from_translation(p)
}

/// min / mean / p50 / p95 / max of `values` as JSON.
pub fn summarize(values: &[f32]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let at = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
    let r = |x: f32| (x * 1000.0).round() / 1000.0;
    serde_json::json!({"min": r(v[0]), "mean": r(mean), "p50": r(at(0.5)), "p95": r(at(0.95)), "max": r(v[v.len() - 1])})
}

/// Process private bytes (commit charge); 0 where unsupported.
#[cfg(windows)]
pub fn private_bytes() -> u64 {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    // SAFETY: plain FFI into kernel32 with a correctly sized, zeroed out-struct.
    unsafe {
        let mut c: Counters = std::mem::zeroed();
        c.cb = std::mem::size_of::<Counters>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            c.private_usage as u64
        } else {
            0
        }
    }
}
#[cfg(not(windows))]
pub fn private_bytes() -> u64 {
    0
}

pub struct BenchState {
    pub cfg: BenchConfig,
    pub spawned_objects: u64,
    pub classic_objects: u32,
    pub spawn_ms: f64,
    pub private_before_assets: u64,
    pub private_before_spawn: u64,
    pub private_after_spawn: u64,
    pub entities: Vec<Entity>,
}

impl BenchState {
    pub fn new(cfg: BenchConfig) -> Self {
        Self {
            cfg,
            spawned_objects: 0,
            classic_objects: 0,
            spawn_ms: 0.0,
            private_before_assets: private_bytes(),
            private_before_spawn: 0,
            private_after_spawn: 0,
            entities: Vec::new(),
        }
    }

    /// Move 1% of the instances (a rotating window of the spawn order) by 2 cm.
    pub fn move_slice(&mut self, world: &mut World, frame: u32) {
        use helio_mesh_host::StaticObjectComponent;
        let n = self.entities.len();
        if n == 0 {
            return;
        }
        let k = (n / 100).max(1);
        let start = (frame as usize * k) % n;
        let sign = if frame % 2 == 0 { 0.02 } else { -0.02 };
        for i in 0..k {
            let e = self.entities[(start + i) % n];
            if let Some(mut o) = world.get_mut::<StaticObjectComponent>(e) {
                let mut m = o.transform();
                m.w_axis.x += sign;
                let bounds = [m.w_axis.x, m.w_axis.y, m.w_axis.z, o.bounds[3]];
                *o = o.with_transform(m, bounds);
            }
        }
    }
}
