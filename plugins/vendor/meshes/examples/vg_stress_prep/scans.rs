//! The six Stanford scans: where they come from, how each is stood up, scaled
//! and centred (the exact transforms the old Blender script applied), and the
//! smooth vertex normals the exported GLB carries.

pub struct Source {
    pub file: &'static str,
    /// Tried in order; the first that serves the whole file wins.
    pub urls: &'static [&'static str],
    pub size: u64,
    pub sha256: &'static str,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Up {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Measure {
    /// Up-axis extent.
    Height,
    /// Largest bounding-box extent.
    Max,
}

pub struct Model {
    pub name: &'static str,
    pub archive: &'static str,
    /// Tar member (path suffix) for `.tar.gz` archives, `None` for bare `.ply.gz`.
    pub member: Option<&'static str>,
    pub up: Up,
    /// Yaw about the (Blender, Z) up axis in degrees, after standing the model up.
    pub yaw_deg: f32,
    pub size: f32,
    pub measure: Measure,
}

pub const SOURCES: &[Source] = &[
    Source {
        file: "lucy.tar.gz",
        urls: &[
            "http://graphics.stanford.edu/data/3Dscanrep/lucy.tar.gz",
            "https://graphics.stanford.edu/data/3Dscanrep/lucy.tar.gz",
        ],
        size: 321_969_388,
        sha256: "c4beb1f7bfa965643bbbf889bd1849a4b4b955e95c731941be61e6edac65616a",
    },
    Source {
        file: "xyzrgb_statuette.ply.gz",
        urls: &[
            "http://graphics.stanford.edu/data/3Dscanrep/xyzrgb/xyzrgb_statuette.ply.gz",
            "https://graphics.stanford.edu/data/3Dscanrep/xyzrgb/xyzrgb_statuette.ply.gz",
            "http://graphics.stanford.edu/data/3Dscanrep/xyzrgb_statuette.ply.gz",
        ],
        size: 106_051_627,
        sha256: "1d867b6540c02935caa777bd6746429a62d4a5d23f11c9bfdfebbaa90c05ca8b",
    },
    Source {
        file: "xyzrgb_dragon.ply.gz",
        urls: &[
            "http://graphics.stanford.edu/data/3Dscanrep/xyzrgb/xyzrgb_dragon.ply.gz",
            "https://graphics.stanford.edu/data/3Dscanrep/xyzrgb/xyzrgb_dragon.ply.gz",
            "http://graphics.stanford.edu/data/3Dscanrep/xyzrgb_dragon.ply.gz",
        ],
        size: 70_527_166,
        sha256: "8aa449f1966cbb50e5896ecc32cf57ab5f0cdfd3c3e37d3e6f60b948997da5c1",
    },
    Source {
        file: "xyzrgb_manuscript.ply.gz",
        urls: &[
            "http://graphics.stanford.edu/data/3Dscanrep/xyzrgb/xyzrgb_manuscript.ply.gz",
            "https://graphics.stanford.edu/data/3Dscanrep/xyzrgb/xyzrgb_manuscript.ply.gz",
            "http://graphics.stanford.edu/data/3Dscanrep/xyzrgb_manuscript.ply.gz",
        ],
        size: 43_020_968,
        sha256: "0c10b4a80e562fcd9c5dcd05eb6e943f2c79017132911cdc6a8d03fd532d6861",
    },
    Source {
        file: "happy_recon.tar.gz",
        urls: &[
            "http://graphics.stanford.edu/pub/3Dscanrep/happy/happy_recon.tar.gz",
            "https://graphics.stanford.edu/pub/3Dscanrep/happy/happy_recon.tar.gz",
        ],
        size: 14_456_495,
        sha256: "409cd294efbfd8244e15a382b95a9423f153b7776e736c9b09f19ec9d3c10ed0",
    },
    Source {
        file: "dragon_recon.tar.gz",
        urls: &[
            "http://graphics.stanford.edu/pub/3Dscanrep/dragon/dragon_recon.tar.gz",
            "https://graphics.stanford.edu/pub/3Dscanrep/dragon/dragon_recon.tar.gz",
        ],
        size: 11_197_764,
        sha256: "74ac1d90989c9b1732edee82d57e9ce71452144cf4355f108d8c9c616d28d02f",
    },
];

pub const MODELS: &[Model] = &[
    Model {
        name: "lucy",
        archive: "lucy.tar.gz",
        member: Some("lucy.ply"),
        up: Up::Z,
        yaw_deg: 180.0,
        size: 3.4,
        measure: Measure::Height,
    },
    Model {
        name: "thai_statue",
        archive: "xyzrgb_statuette.ply.gz",
        member: None,
        up: Up::Y,
        yaw_deg: 0.0,
        size: 2.8,
        measure: Measure::Height,
    },
    Model {
        name: "xyzrgb_dragon",
        archive: "xyzrgb_dragon.ply.gz",
        member: None,
        up: Up::Y,
        yaw_deg: -90.0,
        size: 2.2,
        measure: Measure::Height,
    },
    Model {
        name: "manuscript",
        archive: "xyzrgb_manuscript.ply.gz",
        member: None,
        up: Up::Y,
        yaw_deg: 0.0,
        size: 2.0,
        measure: Measure::Max,
    },
    Model {
        name: "happy_buddha",
        archive: "happy_recon.tar.gz",
        member: Some("happy_recon/happy_vrip.ply"),
        up: Up::Y,
        yaw_deg: 0.0,
        size: 2.2,
        measure: Measure::Height,
    },
    Model {
        name: "dragon",
        archive: "dragon_recon.tar.gz",
        member: Some("dragon_recon/dragon_vrip.ply"),
        up: Up::Y,
        yaw_deg: 90.0,
        size: 1.9,
        measure: Measure::Height,
    },
];

/// Bumped whenever the transform or output layout changes, so stale GLBs are
/// regenerated.
pub const RECIPE_VERSION: u32 = 2;

impl Model {
    pub fn recipe(&self) -> String {
        format!(
            "v{RECIPE_VERSION}:{}:{:?}:{:?}:{}:{}",
            self.archive, self.up, self.measure, self.yaw_deg, self.size
        )
    }
}

/// Stand the file-space `up` axis on Blender's +Z, yaw about Z, translate the
/// base centre to the origin, scale to `size` metres, then express the result
/// in engine axes (+Y up, the Blender -Y "front" facing +Z). Positions are
/// modified in place.
pub fn normalise(positions: &mut [[f32; 3]], model: &Model) {
    let (sy, cy) = model.yaw_deg.to_radians().sin_cos();
    let stand = |p: [f32; 3]| -> [f32; 3] {
        let q = match model.up {
            Up::Y => [p[0], -p[2], p[1]],
            Up::X => [-p[2], p[1], p[0]],
            Up::Z => p,
        };
        [q[0] * cy - q[1] * sy, q[0] * sy + q[1] * cy, q[2]]
    };
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in positions.iter_mut() {
        *p = stand(*p);
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let reference = match model.measure {
        Measure::Height => ext[2],
        Measure::Max => ext[0].max(ext[1]).max(ext[2]),
    };
    let scale = if reference > 0.0 {
        model.size / reference
    } else {
        1.0
    };
    let (cx, cyy) = ((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5);
    for p in positions.iter_mut() {
        let b = [
            (p[0] - cx) * scale,
            (p[1] - cyy) * scale,
            (p[2] - lo[2]) * scale,
        ];
        // Blender (x, y, z) -> FBX Y-up / -Z-forward export (x, z, -y).
        *p = [b[0], b[2], -b[1]];
    }
}

/// Area-weighted smooth vertex normals.
pub fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; positions.len()];
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (
            positions[t[0] as usize],
            positions[t[1] as usize],
            positions[t[2] as usize],
        );
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let f = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &i in t {
            let s = &mut n[i as usize];
            s[0] += f[0];
            s[1] += f[1];
            s[2] += f[2];
        }
    }
    for s in n.iter_mut() {
        let l = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
        *s = if l > 1e-20 {
            [s[0] / l, s[1] / l, s[2] / l]
        } else {
            [0.0, 1.0, 0.0]
        };
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(p: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for v in p {
            for i in 0..3 {
                lo[i] = lo[i].min(v[i]);
                hi[i] = hi[i].max(v[i]);
            }
        }
        (lo, hi)
    }

    fn model(up: Up, yaw: f32, size: f32, measure: Measure) -> Model {
        Model {
            name: "t",
            archive: "t",
            member: None,
            up,
            yaw_deg: yaw,
            size,
            measure,
        }
    }

    #[test]
    fn y_up_model_is_scaled_to_height_centred_and_grounded() {
        // File-space Y-up box 2 wide (x), 4 tall (y), 1 deep (z), offset from origin.
        let mut p = vec![[10.0, 5.0, -3.0], [12.0, 9.0, -2.0], [11.0, 7.0, -2.5]];
        normalise(&mut p, &model(Up::Y, 0.0, 2.0, Measure::Height));
        let (lo, hi) = bounds(&p);
        assert!((hi[1] - lo[1] - 2.0).abs() < 1e-5, "height {lo:?} {hi:?}");
        assert!(lo[1].abs() < 1e-5, "base on y=0");
        assert!((lo[0] + hi[0]).abs() < 1e-5 && (lo[2] + hi[2]).abs() < 1e-5);
        // Scale 0.5: the x extent 2 -> 1.
        assert!((hi[0] - lo[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn z_up_yaw_180_flips_facing_and_measure_max_uses_largest_extent() {
        let mut p = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 4.0]];
        let before = p.clone();
        normalise(&mut p, &model(Up::Z, 180.0, 4.0, Measure::Max));
        let (lo, hi) = bounds(&p);
        // Largest extent (z, 4 m) maps to engine Y and keeps its length.
        assert!((hi[1] - lo[1] - 4.0).abs() < 1e-5);
        // Blender -Y -> engine +Z: a point that was +Y in the file (before the
        // 180 degree yaw) ends up on the +Z side after the yaw+axis swap? It
        // must land on the opposite side from the un-yawed layout.
        let mut q = before;
        normalise(&mut q, &model(Up::Z, 0.0, 4.0, Measure::Max));
        assert!((p[2][2] + q[2][2]).abs() < 1e-5 && p[2][2].abs() > 0.1);
    }

    #[test]
    fn normals_are_unit_and_face_the_winding_direction() {
        let p = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [5.0, 5.0, 5.0],
        ];
        let n = smooth_normals(&p, &[0, 1, 2]);
        for i in 0..3 {
            assert!((n[i][2] - 1.0).abs() < 1e-6);
        }
        assert_eq!(
            n[3],
            [0.0, 1.0, 0.0],
            "unreferenced vertex gets a safe default"
        );
    }
}
