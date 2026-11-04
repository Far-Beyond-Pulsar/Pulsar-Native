//! Pure layout of the VG museum and the stress grid: what goes where.
//!
//! Nothing here touches a [`pulsar_scenedb::World`]; `scene.rs` turns the
//! placements and lights into scene objects. Keeping the layout pure makes it
//! unit-testable and lets the headless smoke test count statues without a GPU.
//!
//! Axes: +Y up, +Z toward the entrance (the temple faces +Z), the temple is
//! centred on the origin; yaw (rotation about +Y, degrees) turns +Z toward +X.

use std::path::{Path, PathBuf};

use super::dims::Dims;
use super::materials::{kit_slots, Mat};
use crate::vg_stress_prep::catalog::{self, Config};

pub const KIT_DIR: &str = "assets/vg_stress/kit/meshes";
pub const STATUE_DIR: &str = "assets/vg_stress/meshes";
pub const TREE_ROOT: &str = "assets/vg_stress";

// ── placements ───────────────────────────────────────────────────────────────

/// One mesh instance: an architectural kit piece, a statue or a stress-grid mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub name: String,
    /// Scene folder the object is filed under.
    pub folder: &'static str,
    /// Mesh asset path relative to the assets dir.
    pub asset: String,
    /// `(slot name, material)`; slots not listed keep the mesh's own surface.
    pub slots: Vec<(String, Mat)>,
    pub position: [f64; 3],
    /// `[pitch, yaw, roll]` degrees (the engine's YXZ Euler convention).
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    /// Gets a `VirtualGeometryComponent` (see `scene::attach_virtual_geometry`).
    pub virtual_geometry: bool,
    /// Source triangles this placement puts in the scene (for the startup report).
    pub triangles: u64,
}

/// Triangles of a kit piece, from the committed kit manifest.
pub fn kit_triangles(piece: &str) -> u64 {
    static MANIFEST: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let manifest = MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/vg_stress/kit/meshes/kit_manifest.json"
        )))
        .unwrap_or_default()
    });
    manifest[piece]["triangles"].as_u64().unwrap_or(0)
}

impl Placement {
    pub fn kit(
        name: impl Into<String>,
        folder: &'static str,
        piece: &str,
        position: [f32; 3],
        yaw: f32,
    ) -> Self {
        Self {
            name: name.into(),
            folder,
            asset: format!("{KIT_DIR}/{piece}.fbx"),
            slots: kit_slots(piece)
                .iter()
                .map(|slot| (slot.to_string(), Mat::for_kit_slot(slot)))
                .collect(),
            position: position.map(f64::from),
            rotation: [0.0, yaw, 0.0],
            scale: [1.0; 3],
            virtual_geometry: false,
            triangles: kit_triangles(piece),
        }
    }

    /// One variant (glTF mesh) of a Poly Haven tree. The asset key carries the
    /// variant after `#`; the loader splits it again.
    pub fn tree(
        name: impl Into<String>,
        tree: &'static catalog::TreeDef,
        variant: usize,
        position: [f32; 3],
        yaw: f32,
        scale: f32,
    ) -> Self {
        let v = &tree.variants[variant];
        Self {
            name: name.into(),
            folder: "Trees",
            asset: format!("{TREE_ROOT}/{}#{}", tree.gltf_rel(), v.mesh),
            slots: tree
                .slots
                .iter()
                .filter_map(|s| Mat::tree_slot(tree.id, s.name).map(|m| (s.name.to_string(), m)))
                .collect(),
            position: position.map(f64::from),
            rotation: [0.0, yaw, 0.0],
            scale: [scale; 3],
            virtual_geometry: true,
            triangles: v.triangles,
        }
    }

    /// Replace the material of the named slot (no-op when the mesh has no such slot).
    pub fn with_slot(mut self, slot: &str, mat: Mat) -> Self {
        for entry in &mut self.slots {
            if entry.0 == slot {
                entry.1 = mat;
            }
        }
        self
    }

    pub fn with_marble(self, mat: Mat) -> Self {
        self.with_slot("marble", mat)
    }

    pub fn with_scale(mut self, scale: [f32; 3]) -> Self {
        self.scale = scale;
        self
    }

    pub fn kit_piece(&self) -> Option<&str> {
        self.asset
            .strip_prefix(KIT_DIR)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|rest| rest.strip_suffix(".fbx"))
    }
}

/// A star exhibit: one of the Stanford scans prepared by `vg_stress_prep`.
#[derive(Clone, Copy, Debug)]
pub struct StatueDef {
    pub label: &'static str,
    /// GLB file stem in `assets/vg_stress/meshes/` (written by `vg_stress_prep`).
    pub stem: &'static str,
    /// Height (up-axis extent) in metres after conversion.
    pub height: f32,
    /// Triangles in the full-resolution scan.
    pub triangles: u64,
    pub material: Mat,
}

pub const LUCY: StatueDef = StatueDef {
    label: "Lucy",
    stem: "lucy",
    height: 3.4,
    triangles: 28_055_728,
    material: Mat::StatueMarble,
};
pub const THAI: StatueDef = StatueDef {
    label: "Thai Statue",
    stem: "thai_statue",
    height: 2.8,
    triangles: 10_000_000,
    material: Mat::StatueBronze,
};
pub const XYZ_DRAGON: StatueDef = StatueDef {
    label: "XYZ RGB Dragon",
    stem: "xyzrgb_dragon",
    height: 2.2,
    triangles: 7_219_045,
    material: Mat::StatueGreenStone,
};
pub const MANUSCRIPT: StatueDef = StatueDef {
    label: "Manuscript",
    stem: "manuscript",
    height: 2.0,
    triangles: 4_305_818,
    material: Mat::StatueLimestone,
};
pub const HAPPY: StatueDef = StatueDef {
    label: "Happy Buddha",
    stem: "happy_buddha",
    height: 2.2,
    triangles: 1_087_716,
    material: Mat::StatueMarble,
};
pub const DRAGON: StatueDef = StatueDef {
    label: "Dragon",
    stem: "dragon",
    height: 1.9,
    triangles: 871_414,
    material: Mat::StatueBronze,
};

impl StatueDef {
    pub fn asset(&self) -> String {
        format!("{STATUE_DIR}/{}.glb", self.stem)
    }

    /// The material slot name `vg_stress_prep` gives the statue.
    pub fn slot(&self) -> String {
        format!("{}_stone", self.stem)
    }
}

/// A light of the layout. `direction` is the way the light travels (spot and
/// directional only).
#[derive(Clone, Debug, PartialEq)]
pub struct LightSpec {
    pub name: String,
    pub folder: &'static str,
    pub kind: LightKind,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub inner_deg: f32,
    pub outer_deg: f32,
    pub cast_shadows: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Point,
    Spot,
    Directional,
}

/// A statue standing in the museum, for tests and tooling.
#[derive(Clone, Debug)]
pub struct StatueInstance {
    pub def: StatueDef,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct MuseumLayout {
    pub placements: Vec<Placement>,
    pub lights: Vec<LightSpec>,
    /// Display names of the star exhibits (for reporting).
    pub statues: Vec<StatueInstance>,
}

impl MuseumLayout {
    pub fn statue_placements(&self) -> impl Iterator<Item = &Placement> {
        self.placements.iter().filter(|p| p.virtual_geometry)
    }

    /// Triangles the statues put on screen if every instance were fully resolved.
    pub fn statue_triangles(&self) -> u64 {
        self.statues.iter().map(|s| s.def.triangles).sum()
    }

    /// Source triangle totals: (architecture kit, statues, trees, all).
    pub fn triangle_report(&self) -> (u64, u64, u64, u64) {
        let (mut kit, mut statues, mut trees) = (0, 0, 0);
        for p in &self.placements {
            if p.kit_piece().is_some() {
                kit += p.triangles;
            } else if p.asset.contains('#') {
                trees += p.triangles;
            } else {
                statues += p.triangles;
            }
        }
        (kit, statues, trees, kit + statues + trees)
    }

    pub fn tree_count(&self) -> usize {
        self.placements
            .iter()
            .filter(|p| p.asset.contains('#'))
            .count()
    }
}

/// Euler `[pitch, yaw, 0]` degrees (YXZ) whose rotation turns the object-space
/// "down" (-Y, the way a light shines) into `direction`.
pub fn aim_rotation(direction: [f32; 3]) -> [f32; 3] {
    let d = glam::Vec3::from_array(direction).normalize_or_zero();
    if d == glam::Vec3::ZERO {
        return [0.0; 3];
    }
    let pitch = (-d.y).clamp(-1.0, 1.0).acos();
    let yaw = if pitch.sin().abs() < 1e-5 {
        0.0
    } else {
        (-d.x).atan2(-d.z)
    };
    [pitch.to_degrees(), yaw.to_degrees(), 0.0]
}

// ── the museum ───────────────────────────────────────────────────────────────

struct Builder<'a> {
    d: &'a Dims,
    out: MuseumLayout,
}

impl Builder<'_> {
    fn add(&mut self, p: Placement) {
        self.out.placements.push(p);
    }

    fn light(
        &mut self,
        name: String,
        folder: &'static str,
        kind: LightKind,
        position: [f32; 3],
        direction: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        range: f32,
        cones: (f32, f32),
        shadows: bool,
    ) {
        self.out.lights.push(LightSpec {
            name,
            folder,
            kind,
            position,
            direction,
            color,
            intensity,
            range,
            inner_deg: cones.0,
            outer_deg: cones.1,
            cast_shadows: shadows,
        });
    }

    /// Place a statue on a pedestal (or directly on `base_y`) with a spotlight.
    #[allow(clippy::too_many_arguments)]
    fn exhibit(
        &mut self,
        def: StatueDef,
        tag: &str,
        x: f32,
        z: f32,
        base_y: f32,
        pedestal: Option<&str>,
        yaw: f32,
        statue_dy: f32,
        spot: Option<(f32, f32)>,
    ) {
        let mut top = base_y;
        if let Some(piece) = pedestal {
            let height = pedestal_height(piece);
            self.add(
                Placement::kit(
                    format!("Pedestal {tag}"),
                    "Pedestals",
                    piece,
                    [x, base_y, z],
                    0.0,
                )
                .with_marble(Mat::MarbleCartago),
            );
            top += height;
        }
        top += statue_dy;
        let mut p = Placement {
            name: format!("{} {tag}", def.label),
            folder: "Statues",
            asset: def.asset(),
            slots: vec![(def.slot(), def.material)],
            position: [f64::from(x), f64::from(top), f64::from(z)],
            rotation: [0.0, yaw, 0.0],
            scale: [1.0; 3],
            virtual_geometry: true,
            triangles: def.triangles,
        };
        p.virtual_geometry = true;
        self.add(p);
        self.out.statues.push(StatueInstance {
            def,
            name: format!("{} {tag}", def.label),
        });
        if let Some((height_above, forward)) = spot {
            let target = [x, top + def.height * 0.55, z];
            let at = [x, top + def.height * 0.55 + height_above, z + forward];
            let dir = [target[0] - at[0], target[1] - at[1], target[2] - at[2]];
            self.light(
                format!("Spot {} {tag}", def.label),
                "Gallery Lights",
                LightKind::Spot,
                at,
                dir,
                [1.0, 0.93, 0.80],
                420.0,
                18.0,
                (10.0, 26.0),
                true,
            );
        }
    }
}

fn pedestal_height(piece: &str) -> f32 {
    match piece {
        "pedestal_small" => 1.1,
        "pedestal_medium" => 0.9,
        "pedestal_large" => 0.8,
        "pedestal_tall" => 1.7,
        "pedestal_round" => 1.0,
        other => panic!("not a pedestal: {other}"),
    }
}

/// Deterministic SplitMix64.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Everything of the Greek temple-museum, its furniture, exhibits and lights.
pub fn museum_layout(d: &Dims) -> MuseumLayout {
    museum_layout_with(d, &Config::default())
}

/// The museum for a configuration (tree set and density).
pub fn museum_layout_with(d: &Dims, config: &Config) -> MuseumLayout {
    let mut b = Builder {
        d,
        out: MuseumLayout::default(),
    };
    let (colx, colz) = d.column_half();
    let s = d.spacing;
    let sty = d.stylobate_y;
    let floor = d.floor_y();
    let eave = d.eave_y();

    // ── shell: one-off meshes at the origin
    for (name, folder, piece) in [
        ("Ground", "Architecture", "ground"),
        ("Plaza", "Architecture", "plaza"),
        ("Crepidoma", "Architecture", "crepidoma"),
        ("Floor", "Architecture", "floor"),
        ("Cella Walls", "Architecture", "cella_walls"),
        ("Ceilings", "Architecture", "ceilings"),
        ("Roof", "Architecture", "roof"),
        ("Impluvium", "Architecture", "impluvium"),
    ] {
        let p = Placement::kit(name, folder, piece, [0.0; 3], 0.0);
        // Marble sets per surface: dark veined Cartago floor with the mosaic
        // inlay, white marble walls, mosaic plaza.
        b.add(match piece {
            "floor" => p.with_marble(Mat::MarbleCartago),
            "plaza" => p.with_marble(Mat::MarbleMosaic),
            "impluvium" => p.with_marble(Mat::MarbleCartago),
            _ => p,
        });
    }

    // ── Doric peristyle: columns
    let front_x: Vec<f32> = (0..d.front_cols)
        .map(|i| (i as f32 - (d.front_cols as f32 - 1.0) / 2.0) * s)
        .collect();
    let side_z: Vec<f32> = (0..d.side_cols)
        .map(|i| (i as f32 - (d.side_cols as f32 - 1.0) / 2.0) * s)
        .collect();
    let mut columns: Vec<(f32, f32)> = Vec::new();
    for &x in &front_x {
        columns.push((x, colz));
        columns.push((x, -colz));
    }
    for &z in &side_z[1..side_z.len() - 1] {
        columns.push((colx, z));
        columns.push((-colx, z));
    }
    for (i, &(x, z)) in columns.iter().enumerate() {
        b.add(Placement::kit(
            format!("Doric Column {}", i + 1),
            "Columns",
            "doric_column",
            [x, sty, z],
            0.0,
        ));
    }

    // ── entablature: one unit per column per facade; the end columns of a
    // facade take the half-length corner units.
    let top = d.doric_top();
    struct Facade {
        yaw: f32,
        label: &'static str,
        /// world (x, z) of every column on the facade
        cols: Vec<(f32, f32)>,
    }
    let facades = [
        Facade {
            yaw: 0.0,
            label: "Front",
            cols: front_x.iter().map(|&x| (x, colz)).collect(),
        },
        Facade {
            yaw: 180.0,
            label: "Rear",
            cols: front_x.iter().map(|&x| (x, -colz)).collect(),
        },
        Facade {
            yaw: 90.0,
            label: "East",
            cols: side_z.iter().map(|&z| (colx, z)).collect(),
        },
        Facade {
            yaw: 270.0,
            label: "West",
            cols: side_z.iter().map(|&z| (-colx, z)).collect(),
        },
    ];
    for f in &facades {
        // facade-local x axis in world (x, z): R_y(yaw) maps local +X to (cos, -sin)
        let ax = (f.yaw.to_radians().cos(), -f.yaw.to_radians().sin());
        let locals: Vec<f32> = f.cols.iter().map(|&(x, z)| x * ax.0 + z * ax.1).collect();
        let lo = locals.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = locals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        for (i, &(x, z)) in f.cols.iter().enumerate() {
            let piece = if (locals[i] - lo).abs() < 1e-3 {
                "doric_entablature_end_l"
            } else if (locals[i] - hi).abs() < 1e-3 {
                "doric_entablature_end_r"
            } else {
                "doric_entablature_mid"
            };
            b.add(Placement::kit(
                format!("Entablature {} {}", f.label, i + 1),
                "Entablature",
                piece,
                [x, top, z],
                f.yaw,
            ));
        }
    }

    // ── pediments and acroteria
    for (label, z, yaw) in [("Front", colz, 0.0), ("Rear", -colz, 180.0)] {
        b.add(Placement::kit(
            format!("Pediment {label}"),
            "Entablature",
            "pediment",
            [0.0, eave, z],
            yaw,
        ));
        b.add(Placement::kit(
            format!("Acroterion {label}"),
            "Roof Details",
            "acroterion",
            [0.0, eave + d.pediment_h - 0.18, z],
            yaw,
        ));
        for sx in [-1.0f32, 1.0] {
            b.add(Placement::kit(
                format!(
                    "Corner Acroterion {label} {}",
                    if sx < 0.0 { "W" } else { "E" }
                ),
                "Roof Details",
                "acroterion_small",
                [sx * (colx + d.cornice_out - 0.35), eave + 0.14, z],
                yaw,
            ));
        }
    }
    // antefixes along both eaves
    let eave_x = colx + d.cornice_out - 0.05;
    let n_ante = (2.0 * colz / 1.6).floor() as i32;
    for side in [-1.0f32, 1.0] {
        for i in 0..=n_ante {
            let z = -colz + i as f32 * (2.0 * colz / n_ante as f32);
            b.add(Placement::kit(
                format!("Antefix {} {}", if side < 0.0 { "W" } else { "E" }, i + 1),
                "Roof Details",
                "antefix",
                [side * eave_x, eave - 0.0, z],
                side * 90.0,
            ));
        }
    }

    // ── cella interior: Ionic colonnades and their entablature
    let ionic_top = sty + d.ionic_height;
    for (si, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
        for (i, &z) in d.ionic_z.iter().enumerate() {
            let tag = format!("{} {}", if si == 0 { "W" } else { "E" }, i + 1);
            b.add(
                Placement::kit(
                    format!("Ionic Column {tag}"),
                    "Columns",
                    "ionic_column",
                    [sx * d.ionic_x, sty, z],
                    0.0,
                )
                .with_marble(Mat::MarbleGrey),
            );
            b.add(Placement::kit(
                format!("Ionic Entablature {tag}"),
                "Entablature",
                "ionic_entablature",
                [sx * d.ionic_x, ionic_top, z],
                90.0,
            ));
        }
    }
    // Corinthian columns of the pronaos
    let pronaos_z = 5.0 * s;
    let corinthian_scale = (d.peristyle_ceiling_y - 0.14 - sty) / d.doric_height;
    for (i, x) in [-4.8f32, -1.6, 1.6, 4.8].into_iter().enumerate() {
        b.add(
            Placement::kit(
                format!("Corinthian Column {}", i + 1),
                "Columns",
                "corinthian_column",
                [x, sty, pronaos_z],
                0.0,
            )
            .with_scale([1.0, corinthian_scale, 1.0]),
        );
    }

    // ── exhibits
    let nave_pool_z = (d.pool.z0 + d.pool.z1) / 2.0;
    b.exhibit(
        LUCY,
        "Apse",
        0.0,
        -10.4,
        floor,
        Some("pedestal_tall"),
        0.0,
        0.0,
        Some((3.6, 3.4)),
    );
    b.exhibit(
        THAI,
        "Impluvium",
        0.0,
        nave_pool_z,
        d.floor_y(),
        Some("pedestal_round"),
        0.0,
        0.0,
        Some((3.4, 4.2)),
    );
    for (sx, tag) in [(-1.0f32, "West"), (1.0, "East")] {
        b.exhibit(
            HAPPY,
            &format!("Nave {tag}"),
            sx * 1.95,
            -7.9,
            floor,
            Some("pedestal_small"),
            -sx * 22.0,
            0.0,
            Some((3.2, 3.0)),
        );
    }
    // ambulatory: dragons at bay centres, benches between
    let amb_x = (d.cella_half_w + d.cella_wall_t / 2.0 + colx - 0.55) / 2.0;
    for (sx, tag) in [(-1.0f32, "West"), (1.0, "East")] {
        for (zi, z) in [-8.0f32, 8.0].into_iter().enumerate() {
            b.exhibit(
                DRAGON,
                &format!("{tag} Gallery {}", zi + 1),
                sx * amb_x,
                z,
                floor,
                Some("pedestal_medium"),
                0.0,
                0.0,
                Some((2.4, 2.2)),
            );
        }
        for (zi, z) in [-4.8f32, 4.8].into_iter().enumerate() {
            b.add(
                Placement::kit(
                    format!("Bench {tag} {}", zi + 1),
                    "Props",
                    "bench",
                    [sx * amb_x, floor, z],
                    sx * 90.0,
                )
                .with_marble(Mat::MarbleGrey),
            );
        }
    }
    // pronaos: the manuscript pages on steles either side of the door
    for (sx, tag) in [(-1.0f32, "West"), (1.0, "East")] {
        let x = sx * 3.3;
        let z = d.cella_z + d.cella_wall_t / 2.0 + 1.0;
        b.add(
            Placement::kit(
                format!("Stele {tag}"),
                "Pedestals",
                "stele",
                [x, floor, z],
                0.0,
            )
            .with_marble(Mat::MarbleGrey),
        );
        b.exhibit(
            MANUSCRIPT,
            tag,
            x,
            z + 0.16,
            floor + 0.0,
            None,
            0.0,
            0.26,
            Some((2.6, 2.4)),
        );
    }

    // ── outdoor sculpture court
    for (sx, tag) in [(-1.0f32, "West"), (1.0, "East")] {
        for (zi, z) in [27.0f32, 41.0].into_iter().enumerate() {
            b.exhibit(
                XYZ_DRAGON,
                &format!("Court {tag} {}", zi + 1),
                sx * 9.5,
                z,
                0.0,
                Some("pedestal_large"),
                -sx * 35.0,
                0.0,
                None,
            );
        }
        b.exhibit(
            HAPPY,
            &format!("Court {tag}"),
            sx * 5.5,
            34.0,
            0.0,
            Some("pedestal_small"),
            -sx * 20.0,
            0.0,
            None,
        );
        b.exhibit(
            DRAGON,
            &format!("Avenue {tag}"),
            sx * 5.5,
            50.0,
            0.0,
            Some("pedestal_medium"),
            -sx * 25.0,
            0.0,
            None,
        );
    }

    // ── fire and cloth
    fn brazier(b: &mut Builder, name: String, x: f32, y: f32, z: f32, intensity: f32, range: f32) {
        b.add(Placement::kit(
            format!("Brazier {name}"),
            "Props",
            "brazier",
            [x, y, z],
            0.0,
        ));
        b.add(Placement::kit(
            format!("Flame {name}"),
            "Props",
            "flame",
            [x, y + 1.22, z],
            0.0,
        ));
        b.light(
            format!("Fire {name}"),
            "Fire Lights",
            LightKind::Point,
            [x, y + 1.6, z],
            [0.0, -1.0, 0.0],
            [1.0, 0.55, 0.18],
            intensity,
            range,
            (0.0, 0.0),
            false,
        );
    }
    for (i, &x) in [-3.2f32, 3.2].iter().enumerate() {
        brazier(
            &mut b,
            format!("Portico {}", i + 1),
            x,
            floor,
            colz + 0.8,
            70.0,
            11.0,
        );
    }
    let aisle_x = (d.ionic_x + d.cella_half_w - d.cella_wall_t / 2.0) / 2.0;
    for (si, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
        for (i, z) in [-8.0f32, -1.6, 1.6, 8.0].into_iter().enumerate() {
            brazier(
                &mut b,
                format!("Aisle {} {}", if si == 0 { "W" } else { "E" }, i + 1),
                sx * aisle_x,
                floor,
                z,
                45.0,
                8.0,
            );
        }
    }
    for (i, z) in [26.0f32, 32.0, 38.0, 44.0, 50.0, 56.0]
        .into_iter()
        .enumerate()
    {
        for (si, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
            brazier(
                &mut b,
                format!("Avenue {} {}", if si == 0 { "W" } else { "E" }, i + 1),
                sx * 3.4,
                0.0,
                z,
                60.0,
                10.0,
            );
        }
    }
    // banners on the facade and between the nave columns
    for (i, x) in [-6.4f32, 0.0, 6.4].into_iter().enumerate() {
        b.add(
            Placement::kit(
                format!("Facade Banner {}", i + 1),
                "Props",
                "banner",
                [x, top - 0.1, colz + 0.35],
                0.0,
            )
            .with_slot(
                "cloth_red",
                if i == 1 {
                    Mat::ClothBlue
                } else {
                    Mat::ClothRed
                },
            ),
        );
    }
    for (si, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
        for (i, z) in [-8.0f32, -4.8, -1.6, 1.6, 4.8, 8.0].into_iter().enumerate() {
            b.add(
                Placement::kit(
                    format!("Nave Banner {} {}", if si == 0 { "W" } else { "E" }, i + 1),
                    "Props",
                    "banner",
                    [sx * (d.ionic_x - 0.55), ionic_top - 0.05, z],
                    -sx * 90.0,
                )
                .with_slot(
                    "cloth_red",
                    if (i + si) % 2 == 0 {
                        Mat::ClothRed
                    } else {
                        Mat::ClothBlue
                    },
                ),
            );
        }
    }
    // urns along the avenue and at the stair head
    for (i, z) in [29.0f32, 35.0, 41.0, 47.0, 53.0].into_iter().enumerate() {
        for (si, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
            b.add(Placement::kit(
                format!("Avenue Urn {} {}", if si == 0 { "W" } else { "E" }, i + 1),
                "Props",
                "urn",
                [sx * 3.4, 0.0, z],
                (i * 37) as f32,
            ));
        }
    }
    for (i, x) in [-8.2f32, 8.2].into_iter().enumerate() {
        b.add(
            Placement::kit(
                format!("Stylobate Urn {}", i + 1),
                "Props",
                "urn",
                [x, floor, colz + 1.0],
                0.0,
            )
            .with_scale([1.25; 3]),
        );
    }
    // cypresses: a jittered ring round the precinct
    let mut rng = Rng::new(0xC1A55);
    let mut tree = 0;
    for side in [-1.0f32, 1.0] {
        for i in 0..14 {
            let z = -26.0 + i as f32 * 6.2 + rng.range(-1.0, 1.0);
            let x = side * (d.plaza.half_w - 1.5 + rng.range(-0.5, 0.8));
            tree += 1;
            b.add(
                Placement::kit(
                    format!("Cypress {tree}"),
                    "Trees",
                    "cypress",
                    [x, 0.0, z],
                    rng.range(0.0, 360.0),
                )
                .with_scale([rng.range(0.85, 1.2); 3]),
            );
        }
    }
    for i in 0..8 {
        let x = -d.plaza.half_w + 1.5 + i as f32 * 6.0 + rng.range(-1.0, 1.0);
        tree += 1;
        b.add(
            Placement::kit(
                format!("Cypress {tree}"),
                "Trees",
                "cypress",
                [x, 0.0, -27.0 + rng.range(-1.0, 1.0)],
                rng.range(0.0, 360.0),
            )
            .with_scale([rng.range(0.9, 1.25); 3]),
        );
    }

    // ── accents
    // a cool bounce from the open cella, down onto the pool
    b.light(
        "Pool Glow".into(),
        "Gallery Lights",
        LightKind::Point,
        [0.0, floor + 2.4, nave_pool_z],
        [0.0; 3],
        [0.55, 0.75, 1.0],
        70.0,
        9.0,
        (0.0, 0.0),
        false,
    );
    b.light(
        "Pronaos Glow".into(),
        "Gallery Lights",
        LightKind::Point,
        [0.0, d.peristyle_ceiling_y - 1.0, d.cella_z + 3.2],
        [0.0; 3],
        [1.0, 0.82, 0.6],
        90.0,
        12.0,
        (0.0, 0.0),
        false,
    );

    scatter_statuary(&mut b, config, amb_x, floor);
    scatter_trees(&mut b, config);
    b.out
}

/// Statue table: (definition, weight).
const CROWD: &[(StatueDef, u32)] = &[
    (THAI, 3),
    (DRAGON, 3),
    (HAPPY, 2),
    (XYZ_DRAGON, 2),
    (MANUSCRIPT, 1),
    (LUCY, 1),
];

fn pick_statue(rng: &mut Rng, table: &[(StatueDef, u32)]) -> StatueDef {
    let total: u32 = table.iter().map(|e| e.1).sum();
    let mut roll = (rng.next_u64() % u64::from(total.max(1))) as u32;
    for (def, w) in table {
        if roll < *w {
            return *def;
        }
        roll -= w;
    }
    table[0].0
}

/// A hall of statue copies in the ambulatories and a crowd in the courtyard,
/// every copy sharing its original's GPU geometry (one mesh, many objects).
fn scatter_statuary(b: &mut Builder, config: &Config, amb_x: f32, floor: f32) {
    let density = config.density;
    let mut rng = Rng::new(0x57A7_0E5);
    // Hall: along both ambulatories, skipping the original exhibits and benches.
    let blocked = [-8.0f32, -4.8, 4.8, 8.0];
    let mut slots: Vec<(f32, f32, f32)> = Vec::new(); // x, z, yaw base
    let rows: &[f32] = if density > 1.5 {
        &[-0.55, 0.55]
    } else {
        &[0.0]
    };
    for sx in [-1.0f32, 1.0] {
        for (ri, dx) in rows.iter().enumerate() {
            let mut z = -16.5 + if ri == 1 { 0.95 } else { 0.0 };
            while z < 16.5 {
                if blocked.iter().all(|bz| (z - bz).abs() > 1.3) {
                    slots.push((sx * (amb_x + dx), z, -sx * 90.0));
                }
                z += 1.9;
            }
        }
    }
    let want = ((24.0 * density).round() as usize).min(slots.len());
    for i in (1..slots.len()).rev() {
        slots.swap(i, rng.below(i + 1));
    }
    for (i, &(x, z, yaw)) in slots.iter().take(want).enumerate() {
        let def = pick_statue(&mut rng, CROWD);
        b.exhibit(
            def,
            &format!("Hall {}", i + 1),
            x,
            z,
            floor,
            Some("pedestal_small"),
            yaw + rng.range(-18.0, 18.0),
            0.0,
            None,
        );
    }

    // Courtyard crowd on the plaza, away from the avenue and each other.
    let mut taken: Vec<(f32, f32)> = b
        .out
        .placements
        .iter()
        .filter(|p| p.folder == "Statues" || p.folder == "Pedestals")
        .map(|p| (p.position[0] as f32, p.position[2] as f32))
        .collect();
    let crowd = (30.0 * density).round() as usize;
    let mut placed = 0;
    let mut attempts = 0;
    while placed < crowd && attempts < crowd * 80 + 200 {
        attempts += 1;
        let x = rng.range(-19.5, 19.5);
        let z = rng.range(24.0, 60.0);
        if x.abs() < 4.6 || taken.iter().any(|&(tx, tz)| (tx - x).hypot(tz - z) < 2.6) {
            continue;
        }
        taken.push((x, z));
        placed += 1;
        let def = pick_statue(&mut rng, CROWD);
        let scale = rng.range(0.85, 1.3);
        let yaw = rng.range(0.0, 360.0);
        let mut p = Placement {
            name: format!("{} Crowd {placed}", def.label),
            folder: "Statues",
            asset: def.asset(),
            slots: vec![(def.slot(), def.material)],
            position: [f64::from(x), 0.0, f64::from(z)],
            rotation: [0.0, yaw, 0.0],
            scale: [scale; 3],
            virtual_geometry: true,
            triangles: def.triangles,
        };
        // A third of the crowd stands on a plinth.
        if rng.below(3) == 0 {
            b.add(
                Placement::kit(
                    format!("Plinth {placed}"),
                    "Pedestals",
                    "pedestal_small",
                    [x, 0.0, z],
                    0.0,
                )
                .with_marble(Mat::MarbleGrey),
            );
            p.position[1] = f64::from(pedestal_height("pedestal_small"));
        }
        b.add(p);
        b.out.statues.push(StatueInstance {
            def,
            name: format!("{} Crowd {placed}", def.label),
        });
    }
}

/// Hero trees at the entrance, accent trees round the courtyard and forests at
/// the edges of the grounds, all seeded and counted by the density.
/// Instances of one variant share the variant's GPU geometry.
fn scatter_trees(b: &mut Builder, config: &Config) {
    if config.trees.is_empty() {
        return;
    }
    let mut rng = Rng::new(0x72EE5);
    let density = config.density;
    let all: Vec<(&'static catalog::TreeDef, usize)> = config
        .trees
        .iter()
        .flat_map(|t| (0..t.variants.len()).map(move |v| (*t, v)))
        .collect();
    let mut tallest = all.clone();
    tallest.sort_by(|a, b| {
        b.0.variants[b.1]
            .height
            .total_cmp(&a.0.variants[a.1].height)
    });
    let heroes: Vec<_> = tallest.iter().take(4).copied().collect();
    let big: Vec<_> = all.iter().copied().filter(|(t, _)| t.large).collect();
    let small: Vec<_> = all.iter().copied().filter(|(t, _)| !t.large).collect();
    let mut n = 0;
    let mut hero_spots: Vec<(f32, f32)> = Vec::new();
    // Hero trees flank the approach; small trees are scaled up to hero size.
    let spots = [
        (27.0, 30.0),
        (-27.0, 33.0),
        (29.0, 47.0),
        (-29.0, 50.0),
        (27.0, 63.0),
        (-27.0, 62.0),
        (11.0, 73.0),
        (-11.0, 74.0),
    ];
    for (i, &(x, z)) in spots.iter().enumerate() {
        let (tree, variant) = heroes[i % heroes.len()];
        let height = tree.variants[variant].height;
        let scale = if tree.large {
            rng.range(0.95, 1.15)
        } else {
            11.0 / height
        };
        n += 1;
        hero_spots.push((x, z));
        b.add(Placement::tree(
            format!("Hero Tree {n}"),
            tree,
            variant,
            [x, 0.0, z],
            rng.range(0.0, 360.0),
            scale,
        ));
    }
    // Courtyard accents just outside the plaza edge.
    let accents = (96.0 * density).round() as usize;
    let mut placed = 0;
    let mut tries = 0;
    while placed < accents && tries < accents * 60 + 100 {
        tries += 1;
        let side = if rng.below(2) == 0 { -1.0f32 } else { 1.0 };
        let x = side * rng.range(23.5, 36.0);
        let z = rng.range(18.0, 68.0);
        if hero_spots
            .iter()
            .any(|&(hx, hz)| (hx - x).hypot(hz - z) < 8.0)
        {
            continue;
        }
        let pool = if small.is_empty() { &all } else { &small };
        let (tree, variant) = pool[rng.below(pool.len())];
        placed += 1;
        n += 1;
        let s = rng.range(tree.forest_scale.0, tree.forest_scale.1);
        b.add(Placement::tree(
            format!("Courtyard Tree {n}"),
            tree,
            variant,
            [x, 0.0, z],
            rng.range(0.0, 360.0),
            s,
        ));
    }
    // Forests: denser with distance from the temple; the approach stays open.
    let forest = (2000.0 * density).round() as usize;
    let mut placed = 0;
    let mut tries = 0;
    while placed < forest && tries < forest * 40 + 200 {
        tries += 1;
        let x = rng.range(-125.0, 125.0);
        let z = rng.range(-125.0, 120.0);
        let temple = x.abs() < 17.0 && z.abs() < 29.0;
        let plaza = x.abs() < 38.0 && (14.0..80.0).contains(&z);
        if temple || plaza {
            continue;
        }
        let r = (x * x + (z - 20.0) * (z - 20.0)).sqrt();
        let t = ((r - 30.0) / 60.0).clamp(0.0, 1.0);
        if rng.unit() > t * t * (3.0 - 2.0 * t) {
            continue;
        }
        // A quarter of the forest are the big conifers when present.
        let pool = if !big.is_empty() && rng.below(4) == 0 {
            &big
        } else if small.is_empty() {
            &all
        } else {
            &small
        };
        let (tree, variant) = pool[rng.below(pool.len())];
        placed += 1;
        n += 1;
        let s = rng.range(tree.forest_scale.0, tree.forest_scale.1);
        b.add(Placement::tree(
            format!("Forest Tree {n}"),
            tree,
            variant,
            [x, 0.0, z],
            rng.range(0.0, 360.0),
            s,
        ));
    }
}

// ── the stress grid ──────────────────────────────────────────────────────────

/// One mesh of the stress grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GridInstance {
    /// Index into the sorted file list.
    pub file: usize,
    pub position: [f32; 3],
    pub yaw: f32,
}

/// Every `fbx/obj/gltf/glb` directly inside `dir`, sorted by file name. Names
/// starting with `_` or `.` (previews, scratch) are skipped, as is anything
/// that is not a regular file.
pub fn scan_mesh_folder(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with('_') || name.starts_with('.') || !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if matches!(ext.as_deref(), Some("fbx" | "obj" | "gltf" | "glb")) {
            files.push(path);
        }
    }
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(files)
}

/// A `grid_size` x `grid_size` field of cells `spacing` apart, centred on the
/// origin, each holding one of `file_count` meshes with seeded jitter and yaw.
/// The same inputs always give the same result.
pub fn stress_grid_layout(
    file_count: usize,
    grid_size: usize,
    spacing: f32,
    seed: u64,
) -> Vec<GridInstance> {
    if file_count == 0 || grid_size == 0 {
        return Vec::new();
    }
    let mut rng = Rng::new(seed);
    let half = (grid_size as f32 - 1.0) / 2.0;
    let mut out = Vec::with_capacity(grid_size * grid_size);
    for row in 0..grid_size {
        for col in 0..grid_size {
            let jitter = spacing * 0.22;
            let x = (col as f32 - half) * spacing + rng.range(-jitter, jitter);
            let z = (row as f32 - half) * spacing + rng.range(-jitter, jitter);
            let file = rng.below(file_count);
            let yaw = rng.range(0.0, 360.0);
            out.push(GridInstance {
                file,
                position: [x, 0.0, z],
                yaw,
            });
        }
    }
    out
}

/// The asset path (`/` separated) the level should store for `file`, relative to
/// `assets_root` when it lives under it, else the absolute path.
pub fn asset_path(file: &Path, assets_root: &Path) -> String {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let file = canon(file);
    let rel = file.strip_prefix(canon(assets_root)).unwrap_or(&file);
    let text = rel.to_string_lossy().replace('\\', "/");
    text.strip_prefix("//?/").unwrap_or(&text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(density: f32, trees: &str) -> MuseumLayout {
        let config = Config::resolve(Some(trees), Some(&density.to_string()), None, None);
        museum_layout_with(&Dims::load(), &config)
    }

    #[test]
    fn layout_is_deterministic() {
        assert_eq!(
            layout(1.0, "full").placements,
            layout(1.0, "full").placements
        );
    }

    #[test]
    fn default_density_scene_counts_and_triangles() {
        let l = layout(1.0, "full");
        let (kit, statues, trees, total) = l.triangle_report();
        assert_eq!(kit + statues + trees, total);
        assert!(
            l.tree_count() >= 1800 && l.tree_count() <= 2600,
            "{}",
            l.tree_count()
        );
        assert!(l.statues.len() > 60, "{}", l.statues.len());
        assert!(trees > 100_000_000, "{trees}");
        assert!(kit > 100_000, "{kit}");
        eprintln!(
            "default: trees {} statues {} tris kit {kit} statues {statues} trees {trees} total {total}",
            l.tree_count(),
            l.statues.len()
        );
    }

    #[test]
    fn density_scales_instances_and_small_preset_has_no_conifers() {
        let low = layout(0.4, "full");
        let mid = layout(1.0, "full");
        let high = layout(3.0, "full");
        assert!(low.tree_count() < mid.tree_count() && mid.tree_count() < high.tree_count());
        assert!(low.statues.len() < mid.statues.len() && mid.statues.len() < high.statues.len());
        let small = layout(1.0, "small");
        assert!(small
            .placements
            .iter()
            .all(|p| !p.asset.contains("pine_tree") && !p.asset.contains("fir_tree")));
        assert!(small.tree_count() > 100);
        assert_eq!(layout(1.0, "none").tree_count(), 0);
        let sm = layout(1.0, "small");
        let ins = layout(8.0, "full");
        eprintln!(
            "small: trees {} tris {:?}; insane: trees {} statues {} tris {:?}",
            sm.tree_count(),
            sm.triangle_report(),
            ins.tree_count(),
            ins.statues.len(),
            ins.triangle_report()
        );
        eprintln!(
            "high: trees {} statues {} total tris {}",
            high.tree_count(),
            high.statues.len(),
            high.triangle_report().3
        );
    }

    #[test]
    fn tree_placements_have_every_slot_and_stay_off_the_temple() {
        let l = layout(2.0, "full");
        for p in l.placements.iter().filter(|p| p.asset.contains('#')) {
            let tree = catalog::TREES
                .iter()
                .find(|t| p.asset.contains(t.id))
                .unwrap();
            assert_eq!(p.slots.len(), tree.slots.len(), "{}", p.name);
            assert!(
                !(p.position[0].abs() < 14.0 && p.position[2].abs() < 26.0),
                "{} inside the temple",
                p.name
            );
            assert!(p.virtual_geometry && p.triangles > 100_000);
        }
    }

    #[test]
    fn marbles_map_to_each_surface() {
        let l = layout(1.0, "none");
        let slot = |name: &str, slot: &str| {
            l.placements
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .slots
                .iter()
                .find(|s| s.0 == slot)
                .map(|s| s.1)
        };
        assert_eq!(slot("Floor", "marble"), Some(Mat::MarbleCartago));
        assert_eq!(slot("Floor", "marble_green"), Some(Mat::MarbleMosaic));
        assert_eq!(slot("Floor", "marble_alt"), Some(Mat::MarbleGrey));
        assert_eq!(slot("Doric Column 1", "marble"), Some(Mat::MarbleWhite));
        assert_eq!(slot("Cella Walls", "marble"), Some(Mat::MarbleWhite));
        assert_eq!(slot("Plaza", "marble"), Some(Mat::MarbleMosaic));
        assert_eq!(slot("Ionic Column W 1", "marble"), Some(Mat::MarbleGrey));
        assert_eq!(slot("Pedestal Apse", "marble"), Some(Mat::MarbleCartago));
    }
}
