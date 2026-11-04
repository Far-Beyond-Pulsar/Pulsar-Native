//! Scripted, deterministic capture mode of the VG museum.
//!
//! `VG_STRESS_CAPTURE=<dir>` (or `--capture <dir>`) renders the museum
//! offscreen at an exact 60 Hz step through a data-driven shot list
//! (`assets/vg_stress/shots.json`, or the built-in default), writing one JPEG
//! (or a numbered JPEG sequence) plus a stats JSON per shot and an
//! `index.json`. See VG_STRESS.md ("Capture mode").
//! Fog and post-process volumes left Helio with their plugins (Pulsar
//! `plugins/vendor/fog`, `plugins/vendor/post`): shots still record their
//! medium, but no longer draw it, and the post stack is the host's baseline.

use super::{AppState, TAU_RANGE};
use helio::Camera;
use helio_vg_format::debug_view;
use helio_vg_stream::VgRuntimeSettings;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

// ── the shot list ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Keyframe {
    /// Seconds from the start of the shot.
    pub t: f32,
    pub position: [f32; 3],
    pub look_at: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov_deg: Option<f32>,
}

/// A fixed camera (`position` + `look_at`, or `position` + `yaw_deg`/`pitch_deg`;
/// yaw 0 looks along -Z like the interactive camera) or a path (`keyframes`,
/// Catmull-Rom through them, `duration_s` at `fps`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CameraSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub look_at: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fov_deg: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<Keyframe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: glam::Vec3,
    pub target: glam::Vec3,
    pub fov_deg: f32,
}

pub const DEFAULT_FOV_DEG: f32 = 50.0;

fn catmull_rom(p: [glam::Vec3; 4], u: f32) -> glam::Vec3 {
    let (u2, u3) = (u * u, u * u * u);
    0.5 * ((2.0 * p[1])
        + (-p[0] + p[2]) * u
        + (2.0 * p[0] - 5.0 * p[1] + 4.0 * p[2] - p[3]) * u2
        + (-p[0] + 3.0 * p[1] - 3.0 * p[2] + p[3]) * u3)
}

impl CameraSpec {
    /// The same camera looking at `target` instead.
    pub fn with_look(mut self, target: [f32; 3]) -> Self {
        self.look_at = Some(target);
        self
    }

    pub fn is_path(&self) -> bool {
        self.keyframes.len() >= 2
    }

    pub fn fps(&self) -> f32 {
        self.fps.filter(|f| *f > 0.0).unwrap_or(30.0)
    }

    /// Number of images: 1 for a fixed camera, `duration * fps` for a path.
    pub fn frame_count(&self) -> u32 {
        if !self.is_path() {
            return 1;
        }
        let end = self.keyframes.last().map_or(0.0, |k| k.t);
        let duration = self.duration_s.filter(|d| *d > 0.0).unwrap_or(end);
        ((duration * self.fps()).round() as u32).max(1)
    }

    /// The pose at `frame` (path shots), always the single pose for a fixed camera.
    pub fn pose(&self, frame: u32) -> Pose {
        if !self.is_path() {
            let position = glam::Vec3::from_array(self.position.unwrap_or([0.0, 3.0, 34.0]));
            let target = match self.look_at {
                Some(t) => glam::Vec3::from_array(t),
                None => {
                    let (yaw, pitch) = (
                        self.yaw_deg.unwrap_or(0.0).to_radians(),
                        self.pitch_deg.unwrap_or(0.0).to_radians(),
                    );
                    position
                        + glam::Vec3::new(
                            yaw.sin() * pitch.cos(),
                            pitch.sin(),
                            -yaw.cos() * pitch.cos(),
                        )
                }
            };
            return Pose {
                position,
                target,
                fov_deg: self.fov_deg.unwrap_or(DEFAULT_FOV_DEG),
            };
        }
        let t = frame as f32 / self.fps();
        self.pose_at(t)
    }

    /// Smooth interpolation at time `t` seconds (clamped to the key range).
    pub fn pose_at(&self, t: f32) -> Pose {
        let keys = &self.keyframes;
        let last = keys.len() - 1;
        let t = t.clamp(keys[0].t, keys[last].t);
        let i = (0..last).rev().find(|&i| keys[i].t <= t).unwrap_or(0);
        let span = (keys[i + 1].t - keys[i].t).max(1e-6);
        let u = ((t - keys[i].t) / span).clamp(0.0, 1.0);
        let at = |j: isize| &keys[j.clamp(0, last as isize) as usize];
        let (a, b, c, d) = (
            at(i as isize - 1),
            at(i as isize),
            at(i as isize + 1),
            at(i as isize + 2),
        );
        let v = |f: &dyn Fn(&Keyframe) -> [f32; 3]| {
            catmull_rom([a, b, c, d].map(|k| glam::Vec3::from_array(f(k))), u)
        };
        let fov = |k: &Keyframe| k.fov_deg.or(self.fov_deg).unwrap_or(DEFAULT_FOV_DEG);
        Pose {
            position: v(&|k| k.position),
            target: v(&|k| k.look_at),
            fov_deg: fov(b) + (fov(c) - fov(b)) * u,
        }
    }
}

/// A shot's medium. The capture records it in its report; fog is the fog
/// plugin's (Pulsar `plugins/vendor/fog`), which Helio's hosts do not draw.
/// `shafts` still gives the sun its shadow map and volumetric flags.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FogSpec {
    pub enabled: bool,
    /// Extinction in 1/m.
    pub density: f32,
    pub albedo: [f32; 3],
    pub anisotropy: f32,
    /// Height-based falloff (0 = uniform).
    pub height_falloff: f32,
    pub height: f32,
    /// Give the sun a shadow map and volumetric participation (light shafts).
    pub shafts: bool,
}

impl Default for FogSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            density: 0.012,
            albedo: [0.85, 0.80, 0.72],
            anisotropy: 0.55,
            height_falloff: 0.0,
            height: 0.0,
            shafts: true,
        }
    }
}

/// The sun by compass azimuth (0 = toward +Z, 90 = toward +X) and elevation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SunSpec {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Default for SunSpec {
    fn default() -> Self {
        Self {
            azimuth_deg: 215.0,
            elevation_deg: 48.0,
            color: [1.0, 0.94, 0.82],
            intensity: 3.0,
        }
    }
}

impl SunSpec {
    /// The direction the light travels (what `directional_light` takes).
    pub fn direction(&self) -> [f32; 3] {
        let (az, el) = (
            self.azimuth_deg.to_radians(),
            self.elevation_deg.to_radians(),
        );
        [-(el.cos() * az.sin()), -el.sin(), -(el.cos() * az.cos())]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AmbientSpec {
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Default for AmbientSpec {
    fn default() -> Self {
        Self {
            color: [1.0, 0.95, 0.85],
            intensity: 0.05,
        }
    }
}

/// A shot's lens. Focus is computed from the camera: `focus_target` (a world
/// position), else `focus_distance_m`, else the `look_at` distance.
/// `focal_length_mm` sets the camera field of view (full-frame 36 mm
/// sensor); without it the focal length follows the shot's `fov_deg`. The
/// capture records the lens in its report; depth of field is the lens
/// plugin's (Pulsar `plugins/vendor/lens`), which Helio's hosts do not draw.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LensSpec {
    pub f_number: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length_mm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_target: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_distance_m: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_coc_px: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture_blades: Option<u32>,
}

impl Default for LensSpec {
    fn default() -> Self {
        Self {
            f_number: 2.0,
            focal_length_mm: None,
            focus_target: None,
            focus_distance_m: None,
            max_coc_px: None,
            aperture_blades: Some(7),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Shot {
    pub name: String,
    pub camera: CameraSpec,
    /// `normal` (beauty), a VG key (`tri_size`, `cluster`, `tier`, `error`, `residency`,
    /// `cull`, `mask`, `dense_overlap`, `dense_mask`, `sw_overdraw`, `material`, `bary`,
    /// `instance`, `lod`, `detail`, `triangle`, `vis_normal`) or a classic view
    /// (`albedo`, `lighting-only`, `wireframe`, or any registered view's name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tau_px: Option<f32>,
    /// `auto` (Tier 3 RT when it can), `nonrt` (depth-peel Tier 3), `hw` (no Tier 3), `off`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Skip the shot unless the GPU supports ray queries (Tier 3 RT).
    pub requires_rt: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog: Option<FogSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sun: Option<SunSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient: Option<AmbientSpec>,
    /// Background / sky colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_color: Option<[f32; 3]>,
    /// Parsed for the shot list's sake; ray-traced lighting is not implemented
    /// in this example (the shot is rendered rasterised and says so in its JSON).
    pub ray_tracing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle_frames: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jpeg_quality: Option<u8>,
    /// Depth of field (hero/cinematic shots only; debug views stay clean).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens: Option<LensSpec>,
    /// The post stack (grading, bloom, vignette) as the host's baseline.
    pub post: bool,
    /// `png` (default) or `jpg` (image sequences).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ShotFile {
    List(Vec<Shot>),
    Object { shots: Vec<Shot> },
}

pub fn parse_shots(text: &str) -> Result<Vec<Shot>, String> {
    let shots = match serde_json::from_str::<ShotFile>(text).map_err(|e| e.to_string())? {
        ShotFile::List(s) | ShotFile::Object { shots: s } => s,
    };
    let mut seen = std::collections::HashSet::new();
    for shot in &shots {
        if shot.name.trim().is_empty() || shot.name.contains(['/', '\\', ':']) {
            return Err(format!("invalid shot name {:?}", shot.name));
        }
        if !seen.insert(shot.name.clone()) {
            return Err(format!("duplicate shot name {:?}", shot.name));
        }
        if shot.camera.position.is_none() && !shot.camera.is_path() {
            return Err(format!(
                "shot {}: camera needs `position` or >= 2 `keyframes`",
                shot.name
            ));
        }
        if shot.camera.is_path() && shot.camera.keyframes.windows(2).any(|w| w[1].t <= w[0].t) {
            return Err(format!("shot {}: keyframe times must increase", shot.name));
        }
    }
    Ok(shots)
}

/// `1080p`, `1440p`, `4k` or `WxH`.
pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let t = text.trim().to_ascii_lowercase();
    match t.as_str() {
        "720p" => return Some((1280, 720)),
        "1080p" => return Some((1920, 1080)),
        "1440p" => return Some((2560, 1440)),
        "4k" | "2160p" => return Some((3840, 2160)),
        _ => {}
    }
    let (w, h) = t.split_once('x')?;
    let (w, h) = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
    ((16..=8192).contains(&w) && (16..=8192).contains(&h)).then_some((w, h))
}

// ── the built-in shot list ───────────────────────────────────────────────────

fn fixed(position: [f32; 3], look_at: [f32; 3], fov: f32) -> CameraSpec {
    CameraSpec {
        position: Some(position),
        look_at: Some(look_at),
        fov_deg: Some(fov),
        ..Default::default()
    }
}

/// The overview pose shared by every `dbg-*` shot.
const OVERVIEW: ([f32; 3], [f32; 3], f32) = ([-34.0, 11.0, 78.0], [6.0, 3.0, 22.0], 62.0);
/// Lucy (courtyard) and the direction the distance ladder looks from.
const LUCY: [f32; 3] = [13.1, 1.2, 25.8];
/// The courtyard Lucy faces (sin 316 deg, 0, cos 316 deg).
const LUCY_DIR: [f32; 3] = [-0.69, 0.06, 0.72];
/// A dense stretch of forest for the Tier 3 comparison.
const FOREST: ([f32; 3], [f32; 3], f32) = ([-22.0, 2.0, 30.0], [-70.0, 8.0, 8.0], 58.0);

fn golden_sun() -> SunSpec {
    SunSpec {
        azimuth_deg: 250.0,
        elevation_deg: 11.0,
        color: [1.0, 0.66, 0.36],
        intensity: 4.5,
    }
}

fn golden_fog() -> FogSpec {
    FogSpec {
        density: 0.0045,
        albedo: [0.95, 0.80, 0.62],
        anisotropy: 0.6,
        ..FogSpec::default()
    }
}

fn from_lucy(distance: f32) -> CameraSpec {
    let d = glam::Vec3::from_array(LUCY_DIR).normalize();
    let p = glam::Vec3::from_array(LUCY) + d * distance;
    fixed(p.to_array(), LUCY, 45.0)
}

/// The distance and tau ladders look at Lucy from a raised north-east direction (clear of the
/// statues and trees even at 12 m, looking down at long range), always from the same direction.
const LADDER_DIR: [f32; 3] = [0.55, 0.42, 0.72];

fn from_lucy_ladder(distance: f32) -> CameraSpec {
    let d = glam::Vec3::from_array(LADDER_DIR).normalize();
    let p = glam::Vec3::from_array(LUCY) + d * distance;
    fixed(p.to_array(), LUCY, 45.0)
}

fn flythrough_path() -> CameraSpec {
    let key = |t: f32, p: [f32; 3], l: [f32; 3]| Keyframe {
        t,
        position: p,
        look_at: l,
        fov_deg: Some(60.0),
    };
    CameraSpec {
        keyframes: vec![
            key(0.0, [0.0, 2.6, 76.0], [0.0, 4.0, 40.0]),
            key(2.0, [0.0, 2.4, 46.0], [0.0, 4.5, 12.0]),
            key(4.0, [-3.0, 2.3, 28.0], [-4.0, 4.0, 0.0]),
            key(5.5, [-8.0, 2.2, 22.0], [-12.0, 3.0, 34.0]),
            key(7.0, [-9.0, 2.4, 36.0], [14.0, 1.8, 28.0]),
            key(8.5, [4.0, 2.6, 44.0], [-40.0, 8.0, 20.0]),
            key(10.0, [-14.0, 3.0, 40.0], [-62.0, 9.0, 18.0]),
        ],
        duration_s: Some(10.0),
        fps: Some(30.0),
        ..Default::default()
    }
}

const HERO: [u32; 2] = [2560, 1440];

fn hero_lens(f_number: f32, focal_mm: Option<f32>, target: [f32; 3]) -> Option<LensSpec> {
    Some(LensSpec {
        f_number,
        focal_length_mm: focal_mm,
        focus_target: Some(target),
        ..Default::default()
    })
}

pub fn default_shots() -> Vec<Shot> {
    let mut shots = Vec::new();
    let ambient = Some(AmbientSpec {
        color: [0.80, 0.84, 1.0],
        intensity: 0.30,
    });
    // A beauty shot: sun, fog, post stack, depth of field, 1440p.
    let beauty = |name: &str,
                  camera: CameraSpec,
                  sun: SunSpec,
                  fog: Option<FogSpec>,
                  clear: [f32; 3],
                  lens: Option<LensSpec>| Shot {
        name: name.into(),
        camera,
        sun: Some(sun),
        fog,
        clear_color: Some(clear),
        ambient: ambient.clone(),
        lens,
        post: true,
        settle_frames: Some(120),
        resolution: Some(HERO),
        ..Default::default()
    };
    let noon = |az: f32, el: f32| SunSpec {
        azimuth_deg: az,
        elevation_deg: el,
        color: [1.0, 0.88, 0.70],
        intensity: 4.0,
    };

    shots.push(beauty(
        "hero-temple-fog",
        fixed([-4.0, 2.8, 70.0], [0.0, 5.0, 14.0], 50.0),
        golden_sun(),
        Some(golden_fog()),
        [0.95, 0.62, 0.38],
        hero_lens(2.8, Some(40.0), [0.0, 4.0, 20.6]),
    ));
    shots.push(beauty(
        "hero-courtyard-statues",
        fixed([-2.0, 1.8, 40.0], [13.1, 1.8, 25.8], 40.0),
        noon(40.0, 26.0),
        Some(FogSpec {
            density: 0.006,
            shafts: false,
            ..golden_fog()
        }),
        [0.55, 0.65, 0.85],
        hero_lens(2.0, Some(85.0), [13.1, 1.7, 25.8]),
    ));
    shots.push(beauty(
        "hero-forest-dusk",
        fixed([-22.0, 2.0, 30.0], [-70.0, 8.0, 8.0], 58.0),
        SunSpec {
            azimuth_deg: 270.0,
            elevation_deg: 5.0,
            color: [1.0, 0.42, 0.22],
            intensity: 3.5,
        },
        Some(FogSpec {
            density: 0.0055,
            albedo: [0.90, 0.72, 0.66],
            ..golden_fog()
        }),
        [0.75, 0.42, 0.34],
        hero_lens(4.0, None, [-70.0, 8.0, 8.0]),
    ));
    shots.push(beauty(
        "statue-lucy-closeup",
        from_lucy(4.5),
        noon(300.0, 30.0),
        None,
        [0.45, 0.55, 0.75],
        hero_lens(2.0, Some(85.0), [13.1, 1.7, 25.8]),
    ));
    shots.push(beauty(
        "statue-thai-closeup",
        fixed([19.1, 2.4, 38.5], [19.1, 2.4, 33.1], 45.0),
        noon(200.0, 30.0),
        None,
        [0.45, 0.55, 0.75],
        hero_lens(2.0, Some(85.0), [19.1, 2.4, 33.1]),
    ));
    shots.push(beauty(
        "trees-canopy-closeup",
        fixed([40.0, 9.0, 44.0], [27.0, 11.0, 30.0], 55.0),
        golden_sun(),
        Some(FogSpec {
            density: 0.004,
            shafts: false,
            ..golden_fog()
        }),
        [0.55, 0.65, 0.85],
        hero_lens(2.8, None, [27.0, 11.0, 30.0]),
    ));
    // Additional hero shots.
    shots.push(beauty(
        "hero-colonnade-sunshafts",
        fixed([16.0, 1.7, 23.0], [3.0, 2.8, 19.0], 50.0),
        SunSpec {
            azimuth_deg: 325.0,
            elevation_deg: 8.0,
            color: [1.0, 0.78, 0.5],
            intensity: 6.0,
        },
        Some(FogSpec {
            density: 0.020,
            albedo: [0.95, 0.88, 0.75],
            anisotropy: 0.72,
            ..FogSpec::default()
        }),
        [0.9, 0.7, 0.5],
        hero_lens(2.0, Some(85.0), [7.0, 2.4, 20.6]),
    ));
    shots.push(beauty(
        "hero-lucy-portrait",
        from_lucy(2.1).with_look([13.1, 2.0, 25.8]),
        noon(300.0, 28.0),
        Some(FogSpec {
            density: 0.004,
            shafts: false,
            ..golden_fog()
        }),
        [0.5, 0.58, 0.72],
        hero_lens(1.6, Some(100.0), [13.1, 2.0, 25.8]),
    ));
    shots.push(beauty(
        "hero-thai-pedestal",
        fixed([-19.5, 2.2, 25.4], [-14.0, 2.5, 26.7], 45.0),
        noon(260.0, 22.0),
        None,
        [0.5, 0.58, 0.72],
        hero_lens(1.8, Some(100.0), [-14.0, 2.7, 26.7]),
    ));
    let near_dragon = [13.4, 1.0, 40.4];
    let far_dragon = [13.4, 1.0, 33.4];
    for (suffix, focus) in [("a", near_dragon), ("b", far_dragon)] {
        shots.push(beauty(
            &format!("hero-dragon-court-rack-focus-{suffix}"),
            fixed([17.8, 1.4, 47.0], [13.4, 1.1, 37.0], 45.0),
            noon(30.0, 28.0),
            Some(FogSpec {
                density: 0.008,
                shafts: false,
                ..golden_fog()
            }),
            [0.5, 0.58, 0.72],
            hero_lens(1.8, Some(85.0), focus),
        ));
    }
    shots.push(beauty(
        "hero-pine-bokeh",
        fixed([20.0, 1.6, 36.0], [27.0, 3.0, 30.0], 40.0),
        golden_sun(),
        Some(FogSpec {
            density: 0.006,
            shafts: false,
            ..golden_fog()
        }),
        [0.6, 0.6, 0.65],
        hero_lens(1.4, Some(135.0), [27.0, 2.2, 30.0]),
    ));
    shots.push(beauty(
        "hero-interior-hall-dust",
        fixed([0.0, 1.7, 15.5], [0.0, 3.0, 40.0], 60.0),
        SunSpec {
            azimuth_deg: 15.0,
            elevation_deg: 12.0,
            color: [1.0, 0.85, 0.62],
            intensity: 6.0,
        },
        Some(FogSpec {
            density: 0.016,
            albedo: [0.95, 0.9, 0.8],
            anisotropy: 0.7,
            ..FogSpec::default()
        }),
        [0.85, 0.75, 0.6],
        hero_lens(2.8, Some(35.0), [0.0, 2.0, 34.0]),
    ));

    // Debug gallery at ONE overview pose: no lens, no fog, no grading.
    let overview = || fixed(OVERVIEW.0, OVERVIEW.1, OVERVIEW.2);
    for (name, view) in [
        ("dbg-normal", "normal"),
        ("dbg-albedo", "albedo"),
        ("dbg-lighting-only", "lighting-only"),
        ("dbg-triangle-id", "triangle"),
        ("dbg-cluster-id", "cluster"),
        ("dbg-lod-level", "lod"),
        ("dbg-detail-vs-root", "detail"),
        ("dbg-triangle-size", "tri_size"),
        ("dbg-raster-tier", "tier"),
        ("dbg-sw-overdraw", "sw_overdraw"),
        ("dbg-dense-overlap", "dense_overlap"),
        ("dbg-dense-mask", "dense_mask"),
        ("dbg-page-residency", "residency"),
        ("dbg-culling-survival", "cull"),
        ("dbg-error-vs-tau", "error"),
        ("dbg-instance-id", "instance"),
        ("dbg-barycentrics", "bary"),
        ("dbg-material-bin", "material"),
        ("dbg-classic-vs-vg-mask", "mask"),
    ] {
        shots.push(Shot {
            name: name.into(),
            camera: overview(),
            debug_view: Some(view.into()),
            ..Default::default()
        });
    }

    // Distance ladder on Lucy, same direction.
    for (label, distance) in [("3m", 3.0), ("12m", 12.0), ("48m", 48.0), ("150m", 150.0)] {
        for (suffix, view) in [
            ("normal", None),
            ("triangles", Some("triangle")),
            ("clusters", Some("cluster")),
        ] {
            shots.push(Shot {
                name: format!("lucy-{label}-{suffix}"),
                camera: from_lucy_ladder(distance),
                debug_view: view.map(String::from),
                ..Default::default()
            });
        }
    }

    // Tau ladder on one close shot.
    for (label, tau) in [
        ("0p5", 0.5),
        ("1", 1.0),
        ("4", 4.0),
        ("16", 16.0),
        ("64", 64.0),
    ] {
        for (suffix, view) in [("", None), ("-triangles", Some("triangle"))] {
            shots.push(Shot {
                name: format!("tau-{label}{suffix}"),
                camera: from_lucy_ladder(6.0),
                debug_view: view.map(String::from),
                tau_px: Some(tau),
                ..Default::default()
            });
        }
    }

    // Tier 3 comparison on a dense forest view.
    for (label, mode, rt) in [
        ("off", "hw", false),
        ("tier2", "tier2", false),
        ("on-peel", "nonrt", false),
        ("on-rt", "auto", true),
    ] {
        for (suffix, view) in [("", None), ("-dense-overlap", Some("dense_overlap"))] {
            shots.push(Shot {
                name: format!("tier3-{label}{suffix}"),
                camera: fixed(FOREST.0, FOREST.1, FOREST.2),
                debug_view: view.map(String::from),
                mode: Some(mode.into()),
                requires_rt: rt,
                settle_frames: Some(300),
                ..Default::default()
            });
        }
    }

    // Cinematics: JPEG sequences (a video encoder takes them), subtle focus pull.
    shots.push(Shot {
        name: "flythrough".into(),
        camera: flythrough_path(),
        sun: Some(golden_sun()),
        fog: Some(FogSpec {
            density: 0.006,
            shafts: false,
            ..golden_fog()
        }),
        clear_color: Some([0.8, 0.6, 0.45]),
        ambient,
        lens: Some(LensSpec {
            f_number: 4.0,
            ..Default::default()
        }),
        post: true,
        resolution: Some([1280, 720]),
        format: Some("jpg".into()),
        jpeg_quality: Some(88),
        ..Default::default()
    });
    shots.push(Shot {
        name: "flythrough-debug".into(),
        camera: flythrough_path(),
        debug_view: Some("cluster".into()),
        resolution: Some([960, 540]),
        format: Some("jpg".into()),
        jpeg_quality: Some(80),
        ..Default::default()
    });
    shots
}

/// `assets/vg_stress/shots.json` if it exists and parses, else the default.
pub fn load_shots(root: &Path) -> Vec<Shot> {
    let path = root.join("assets/vg_stress/shots.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => match parse_shots(&text) {
            Ok(shots) => {
                println!("[Capture] {} shots from {}", shots.len(), path.display());
                return shots;
            }
            Err(e) => eprintln!("[Capture] {}: {e}; using the built-in list", path.display()),
        },
        Err(_) => {}
    }
    default_shots()
}

// ── options ──────────────────────────────────────────────────────────────────

pub struct Options {
    pub dir: PathBuf,
    pub size: (u32, u32),
    /// Run only shots whose name contains one of these (`VG_STRESS_CAPTURE_ONLY=a,b`).
    pub only: Vec<String>,
}

impl Options {
    pub fn from_env_and_args() -> Option<Self> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::from_sources(&args, &|k| std::env::var(k).ok())
    }

    pub fn from_sources(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let dir = args
            .iter()
            .position(|a| a == "--capture")
            .and_then(|i| args.get(i + 1).cloned())
            .or_else(|| {
                args.iter()
                    .find_map(|a| a.strip_prefix("--capture=").map(String::from))
            })
            .or_else(|| env("VG_STRESS_CAPTURE"))
            .filter(|d| !d.trim().is_empty())?;
        Some(Self {
            dir: PathBuf::from(dir),
            size: env("VG_STRESS_CAPTURE_RES")
                .and_then(|r| parse_resolution(&r))
                .unwrap_or((1600, 900)),
            only: env("VG_STRESS_CAPTURE_ONLY")
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

// ── the post stack: grading + lens ───────────────────────────────────────────

/// The museum's post settings: cool, slightly bright grading with bloom and a
/// vignette, published as the host's baseline (`set_post_baseline`) when on
/// and the defaults when off. (Its warmer temple-interior volume left Helio
/// with the post-volume plugin.)
pub struct PostStack {
    settings: helio_pass_postprocess::PostProcessSettings,
}

impl PostStack {
    pub fn install(renderer: &mut helio::Renderer) -> Self {
        use helio_pass_postprocess::PostProcessSettings;
        let settings = PostProcessSettings {
            bloom_enabled: true,
            bloom_intensity: 0.22,
            bloom_threshold: 1.1,
            bloom_knee: 0.5,
            bloom_tint: [1.0, 0.92, 0.8],
            color_gain: [0.98, 1.0, 1.04],
            color_saturation: [1.04, 1.04, 1.06],
            color_contrast: [1.04, 1.04, 1.04],
            vignette_enabled: true,
            vignette_intensity: 0.28,
            vignette_smoothness: 2.0,
            vignette_roundness: 1.0,
            vignette_color: [0.05, 0.04, 0.04],
            ..PostProcessSettings::default()
        };
        let stack = Self { settings };
        stack.set_on(renderer, false);
        stack
    }

    pub fn set_on(&self, renderer: &mut helio::Renderer, on: bool) {
        let settings = if on {
            self.settings.clone()
        } else {
            helio_pass_postprocess::PostProcessSettings::default()
        };
        super::v3_demo_common::set_post_baseline(renderer, &settings);
    }
}

/// The lens a shot is taken through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotLens {
    pub focal_length_mm: f32,
    pub f_number: f32,
    pub focus_distance_m: f32,
}

/// Full-frame sensor width.
const SENSOR_WIDTH_MM: f32 = 36.0;

/// The lens for `spec` looking from `pose` (focus follows the shot), and the
/// field of view it implies.
pub fn lens_component(spec: &LensSpec, pose: &Pose, aspect: f32) -> (ShotLens, f32) {
    let sensor_h = SENSOR_WIDTH_MM / aspect;
    let fov_deg = match spec.focal_length_mm {
        Some(f) => (2.0 * (sensor_h / (2.0 * f)).atan()).to_degrees(),
        None => pose.fov_deg,
    };
    let lens = ShotLens {
        focal_length_mm: spec
            .focal_length_mm
            .unwrap_or(sensor_h / (2.0 * (fov_deg.to_radians() * 0.5).tan())),
        f_number: spec.f_number,
        focus_distance_m: spec
            .focus_target
            .map(|t| (glam::Vec3::from_array(t) - pose.position).length())
            .or(spec.focus_distance_m)
            .unwrap_or_else(|| (pose.target - pose.position).length())
            .max(0.2),
    };
    (lens, fov_deg)
}
// ── the runner ───────────────────────────────────────────────────────────────

const DT: f32 = 1.0 / 60.0;
/// Frames the scene must stay fully ready before the first shot.
const READY_FRAMES: u32 = 120;

enum Phase {
    Waiting {
        ready_frames: u32,
        started: std::time::Instant,
    },
    Shot(ShotRun),
    Done,
}

struct ShotRun {
    index: usize,
    frame: u32,
    frames: u32,
    settle: u32,
    gpu: BTreeMap<&'static str, (f64, u32)>,
    last_generation: u64,
    started: std::time::Instant,
    applied: bool,
    size: (u32, u32),
}

pub struct Capture {
    options: Options,
    shots: Vec<Shot>,
    phase: Phase,
    target: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    readback: Option<(wgpu::Buffer, u64)>,
    index: Vec<serde_json::Value>,
    post: Option<PostStack>,
    last_lens: Option<(f32, f32, f32, bool)>,
    rt_hardware: bool,
    bytes_written: u64,
    default_ambient: AmbientSpec,
    default_clear: [f32; 4],
    default_sun: SunSpec,
    /// Instance benchmark: the current shot moves 1% of the instances per frame.
    moving: bool,
    /// (CPU render-call ms, wall ms incl. GPU wait) of this shot's frames.
    samples: Vec<(f32, f32)>,
    /// Frames this shot spent in BVH skip / refit / rebuild.
    bvh_modes: [u32; 3],
    derive_baseline: (u64, u64),
}

impl Capture {
    pub fn new(options: Options, shots: Vec<Shot>) -> Self {
        let shots: Vec<Shot> = if options.only.is_empty() {
            shots
        } else {
            shots
                .into_iter()
                .filter(|s| options.only.iter().any(|o| s.name.contains(o.as_str())))
                .collect()
        };
        println!(
            "[Capture] {} shots -> {} at {}x{}",
            shots.len(),
            options.dir.display(),
            options.size.0,
            options.size.1
        );
        Self {
            options,
            shots,
            phase: Phase::Waiting {
                ready_frames: 0,
                started: std::time::Instant::now(),
            },
            target: None,
            readback: None,
            index: Vec::new(),
            post: None,
            last_lens: None,
            rt_hardware: false,
            bytes_written: 0,
            default_ambient: AmbientSpec::default(),
            default_clear: [0.018, 0.022, 0.035, 1.0],
            default_sun: SunSpec {
                azimuth_deg: 0.0,
                elevation_deg: 0.0,
                color: [1.0, 0.94, 0.82],
                intensity: 3.0,
            },
            moving: false,
            samples: Vec::new(),
            bvh_modes: [0; 3],
            derive_baseline: (0, 0),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.options.size
    }

    fn ensure_target(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.target.as_ref().is_some_and(|t| t.2 == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("VG museum capture target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.target = Some((texture, view, size));
        let padded = padded_row(size.0) as u64 * u64::from(size.1);
        self.readback = Some((
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VG museum capture readback"),
                size: padded,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            padded,
        ));
    }

    /// One call = one rendered frame. Returns true when everything is written.
    pub fn step(&mut self, s: &mut AppState) -> bool {
        s.poll_loader();
        let size = self.options.size;
        match &mut self.phase {
            Phase::Done => return true,
            Phase::Waiting { .. } => {
                if self.post.is_none() {
                    self.post = Some(PostStack::install(&mut s.renderer));
                }
                self.ensure_target(&s.device, size);
                let first = self.shots.first().map(|x| x.camera.pose(0));
                let pose = first.unwrap_or(Pose {
                    position: glam::Vec3::new(0.0, 3.0, 34.0),
                    target: glam::Vec3::new(0.0, 3.0, 0.0),
                    fov_deg: 50.0,
                });
                self.render(s, pose, size);
                let visible = s
                    .targets
                    .lock()
                    .map(|t| (t.visible, t.targets.len()))
                    .unwrap_or((0, 0));
                let loader_done = s.loader.is_none();
                let ready = loader_done && visible.0 == visible.1 && visible.1 > 0;
                let Phase::Waiting {
                    ready_frames,
                    started,
                } = &mut self.phase
                else {
                    unreachable!()
                };
                *ready_frames = if ready { *ready_frames + 1 } else { 0 };
                if s.frames % 120 == 0 {
                    println!(
                        "[Capture] waiting: loader {}, VG instances visible {}/{}, status: {}",
                        if loader_done { "done" } else { "running" },
                        visible.0,
                        visible.1,
                        s.loading_status
                    );
                }
                // A cold cook can take long; the wait is bounded by VG_STRESS_CAPTURE_WAIT_MIN.
                let limit = std::env::var("VG_STRESS_CAPTURE_WAIT_MIN")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(120);
                let timed_out = started.elapsed().as_secs() > limit * 60;
                let settled = *ready_frames >= READY_FRAMES;
                if loader_done && timed_out && !settled {
                    eprintln!(
                        "[Capture] WARNING: only {}/{} VG instances became ready; capturing anyway",
                        visible.0, visible.1
                    );
                }
                if settled || (loader_done && timed_out) {
                    self.rt_hardware = s.runtime.tier3_stats().rt_hardware;
                    println!(
                        "[Capture] scene ready ({} VG assets, {} VG instances, {} loading errors); starting shots",
                        s.vg_meshes.len(),
                        visible.1,
                        s.loading_errors
                    );
                    let _ = std::fs::create_dir_all(&self.options.dir);
                    self.default_clear = [0.018, 0.022, 0.035, 1.0];
                    self.begin_shot(s, 0);
                }
                false
            }
            Phase::Shot(_) => self.step_shot(s),
        }
    }

    fn begin_shot(&mut self, s: &mut AppState, index: usize) {
        let mut index = index;
        while index < self.shots.len() {
            let shot = &self.shots[index];
            if shot.requires_rt && !self.rt_hardware {
                eprintln!("[Capture] skip {}: no ray-query hardware", shot.name);
                self.index.push(
                    serde_json::json!({"name": shot.name, "skipped": "no ray-query hardware"}),
                );
                index += 1;
                continue;
            }
            if let Some(v) = &shot.debug_view {
                if resolve_view(v, &s.debug_views).is_none() {
                    eprintln!(
                        "[Capture] skip {}: debug view {v:?} is not available on this device",
                        shot.name
                    );
                    self.index.push(serde_json::json!({"name": shot.name, "skipped": format!("debug view {v} unavailable")}));
                    index += 1;
                    continue;
                }
            }
            break;
        }
        if index >= self.shots.len() {
            self.finish();
            return;
        }
        let shot = self.shots[index].clone();
        let size = shot
            .resolution
            .map(|r| (r[0], r[1]))
            .unwrap_or(self.options.size);
        let frames = shot.camera.frame_count();
        self.moving = s.bench.is_some() && shot.name.contains("-move");
        self.samples.clear();
        self.bvh_modes = [0; 3];
        self.derive_baseline = s
            .targets
            .lock()
            .map_or((0, 0), |t| (t.derive_calls, t.derive_rebuilds));
        println!(
            "[Capture] shot {}/{}: {} ({} frame{}, {}x{})",
            index + 1,
            self.shots.len(),
            shot.name,
            frames,
            if frames == 1 { "" } else { "s" },
            size.0,
            size.1
        );
        self.phase = Phase::Shot(ShotRun {
            index,
            frame: 0,
            frames,
            settle: shot.settle_frames.unwrap_or(90),
            gpu: BTreeMap::new(),
            last_generation: 0,
            started: std::time::Instant::now(),
            applied: false,
            size,
        });
    }

    fn apply(&mut self, s: &mut AppState, shot: &Shot, size: (u32, u32)) {
        // View.
        let mode = shot
            .debug_view
            .as_deref()
            .and_then(|v| resolve_view(v, &s.debug_views))
            .unwrap_or(0);
        s.renderer.set_debug_mode(mode);
        // VG.
        let current = s.runtime.settings();
        let vg_mode = match shot.mode.as_deref() {
            Some("hw") => helio_vg_format::VgMode::ForceHwRaster,
            Some("nonrt") => helio_vg_format::VgMode::ForceNonRtFallback,
            Some("off") => helio_vg_format::VgMode::Off,
            _ => helio_vg_format::VgMode::Auto,
        };
        s.tau = shot.tau_px.unwrap_or(1.0).clamp(TAU_RANGE.0, TAU_RANGE.1);
        s.runtime.set_settings(VgRuntimeSettings {
            mode: vg_mode,
            tau_px: s.tau,
            tier3: matches!(shot.mode.as_deref(), Some("nonrt") | Some("auto")),
            ..current
        });
        // Lighting.
        let sun = shot.sun.clone().unwrap_or_else(|| SunSpec {
            azimuth_deg: 215.0,
            elevation_deg: 48.0,
            ..SunSpec::default()
        });
        let mut light = super::directional_light(sun.direction(), sun.color, sun.intensity);
        if shot.fog.as_ref().is_some_and(|f| f.enabled && f.shafts) {
            light = super::v3_demo_common::volumetric_light(light, 0);
        }
        if let Some(id) = s.sun_light {
            super::v3_demo_common::update_light(&mut s.scene_db.world, id, light);
        }
        let ambient = shot
            .ambient
            .clone()
            .unwrap_or_else(|| self.default_ambient.clone());
        s.renderer.set_feature_live(helio::AmbientLight {
            color: ambient.color,
            intensity: ambient.intensity,
        });
        let clear = shot
            .clear_color
            .map_or(self.default_clear, |c| [c[0], c[1], c[2], 1.0]);
        s.renderer.set_clear_color(clear);
        if let Some(post) = &self.post {
            post.set_on(&mut s.renderer, shot.post);
        }
        if shot.ray_tracing {
            eprintln!(
                "[Capture] {}: ray_tracing requested but not implemented in this example; rendering rasterised",
                shot.name
            );
        }
        // Resolution.
        if size != (s.renderer.output_width(), s.renderer.output_height()) {
            s.renderer.set_render_size(size.0, size.1);
        }
        self.ensure_target(&s.device, size);
    }

    /// Record the shot's lens for this frame (focus computed from the pose)
    /// and return the pose with the field of view the lens implies.
    fn lensed(&mut self, _s: &mut AppState, shot: &Shot, pose: Pose, size: (u32, u32)) -> Pose {
        let aspect = size.0 as f32 / size.1 as f32;
        let (lens, fov) = match &shot.lens {
            Some(spec) => lens_component(spec, &pose, aspect),
            None => (lens_component(&LensSpec::default(), &pose, aspect).0, pose.fov_deg),
        };
        self.last_lens = Some((
            lens.focal_length_mm,
            lens.f_number,
            lens.focus_distance_m,
            shot.lens.is_some(),
        ));
        Pose {
            fov_deg: fov,
            ..pose
        }
    }

    fn render(&mut self, s: &mut AppState, pose: Pose, size: (u32, u32)) {
        s.cam_pos = pose.position;
        let camera = Camera::perspective_look_at(
            pose.position,
            pose.target,
            glam::Vec3::Y,
            pose.fov_deg.to_radians(),
            size.0 as f32 / size.1 as f32,
            0.1,
            500.0,
        );
        s.renderer.set_frame_delta_override(Some(DT));
        if self.moving {
            let frame = s.frames as u32;
            if let Some(b) = s.bench.as_mut() {
                b.move_slice(&mut s.scene_db.world, frame);
            }
        }
        super::v3_demo_common::flush_scene_db(&s.scene_db, &s.queue);
        s.rays.publish(&s.scene_db.world, &mut s.renderer);
        let view = &self.target.as_ref().expect("capture target").1;
        let started = std::time::Instant::now();
        if let Err(e) = s.renderer.render(&camera, view) {
            log::error!("Capture render: {e:?}");
            s.render_errors += 1;
        }
        let cpu_ms = started.elapsed().as_secs_f32() * 1000.0;
        s.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let wall_ms = started.elapsed().as_secs_f32() * 1000.0;
        s.frames += 1;
        if let Some(bench) = &s.bench {
            self.samples.push((cpu_ms, wall_ms));
            if let Some(bvh) = s.renderer.find_pass::<helio_pass_vg_bvh::VgBvhPass>() {
                let slot = match bvh.mode() {
                    helio_pass_vg_bvh::BvhMode::Skip => 0,
                    helio_pass_vg_bvh::BvhMode::Refit => 1,
                    helio_pass_vg_bvh::BvhMode::Rebuild => 2,
                };
                self.bvh_modes[slot] += 1;
            }
            let private = super::vg_stress_bench::private_bytes();
            if private as f64 > bench.cfg.mem_limit_gb * 1e9 {
                eprintln!(
                    "[Bench] private memory {:.2} GB exceeds the {} GB guard; stopping",
                    private as f64 / 1e9,
                    bench.cfg.mem_limit_gb
                );
                std::process::exit(3);
            }
        }
    }

    fn step_shot(&mut self, s: &mut AppState) -> bool {
        let Phase::Shot(run) = &mut self.phase else {
            return false;
        };
        let index = run.index;
        let shot = self.shots[index].clone();
        let size = run.size;
        if !run.applied {
            run.applied = true;
            self.apply(s, &shot, size);
        }
        let Phase::Shot(run) = &mut self.phase else {
            return false;
        };
        let (frame_now, settle_now) = (run.frame, run.settle);
        let pose = shot.camera.pose(frame_now);
        let pose = self.lensed(s, &shot, pose, size);
        let total_settle = if frame_now == 0 { settle_now } else { 2 };
        // Settle frames (the first frame of a sequence only; later frames follow the path).
        let mut gpu_window: BTreeMap<&'static str, (f64, u32)> = BTreeMap::new();
        let mut last_generation = match &self.phase {
            Phase::Shot(r) => r.last_generation,
            _ => 0,
        };
        for i in 0..total_settle.max(1) {
            self.render(s, pose, size);
            if i + 30 >= total_settle {
                let snap = s.renderer.timing_snapshot();
                if snap.generation != last_generation {
                    last_generation = snap.generation;
                    for p in &snap.passes {
                        if let Some(ms) = p.gpu_ms {
                            let e = gpu_window.entry(p.name).or_insert((0.0, 0));
                            e.0 += f64::from(ms);
                            e.1 += 1;
                        }
                    }
                    if let Some(total) = snap.total_gpu_ms {
                        let e = gpu_window.entry("TOTAL").or_insert((0.0, 0));
                        e.0 += f64::from(total);
                        e.1 += 1;
                    }
                }
            }
        }
        let Phase::Shot(run) = &mut self.phase else {
            return false;
        };
        run.last_generation = last_generation;
        for (k, v) in gpu_window {
            let e = run.gpu.entry(k).or_insert((0.0, 0));
            e.0 += v.0;
            e.1 += v.1;
        }
        let frame = run.frame;
        let frames = run.frames;
        // Write the image.
        let jpg = shot.format.as_deref() == Some("jpg");
        let ext = if jpg { "jpg" } else { "png" };
        let path = if frames == 1 {
            self.options.dir.join(format!("{}.{ext}", shot.name))
        } else {
            self.options
                .dir
                .join(&shot.name)
                .join(format!("{frame:04}.{ext}"))
        };
        let quality = shot.jpeg_quality.unwrap_or(92).clamp(30, 100);
        match self.save_image(s, &path, size, jpg.then_some(quality)) {
            Ok(bytes) => self.bytes_written += bytes,
            Err(e) => {
                eprintln!("[Capture] {}: {e}", shot.name);
                s.loading_errors += 1;
            }
        }
        let Phase::Shot(run) = &mut self.phase else {
            return false;
        };
        run.frame += 1;
        if frames > 1 && run.frame % 30 == 0 {
            println!("[Capture]   {} frame {}/{}", shot.name, run.frame, frames);
        }
        if run.frame < frames {
            return false;
        }
        // Shot done: stats.
        let gpu: BTreeMap<&str, f64> = run
            .gpu
            .iter()
            .filter(|(_, v)| v.1 > 0)
            .map(|(k, v)| (*k, (v.0 / f64::from(v.1) * 1000.0).round() / 1000.0))
            .collect();
        let elapsed = run.started.elapsed().as_secs_f32();
        let stats = self.stats_json(s, &shot, pose_of(&shot, frames), size, frames, gpu, elapsed);
        let json_path = self.options.dir.join(format!("{}.json", shot.name));
        if let Err(e) = std::fs::write(
            &json_path,
            serde_json::to_vec_pretty(&stats).unwrap_or_default(),
        ) {
            eprintln!("[Capture] {}: {e}", json_path.display());
        }
        self.index.push(serde_json::json!({
            "name": shot.name,
            "image": if frames == 1 { format!("{}.{}", shot.name, if shot.format.as_deref() == Some("jpg") { "jpg" } else { "png" }) } else { format!("{}/", shot.name) },
            "stats": format!("{}.json", shot.name),
            "frames": frames,
            "selected_triangles": stats["vg"]["selected_triangles"],
            "gpu_ms": stats["gpu_ms"]["TOTAL"],
        }));
        self.begin_shot(s, index + 1);
        matches!(self.phase, Phase::Done)
    }

    #[allow(clippy::too_many_arguments)]
    fn stats_json(
        &self,
        s: &mut AppState,
        shot: &Shot,
        pose: Pose,
        size: (u32, u32),
        frames: u32,
        gpu: BTreeMap<&str, f64>,
        seconds: f32,
    ) -> serde_json::Value {
        let cull = s
            .renderer
            .find_pass::<helio_pass_vg_cull::VgCullEarlyPass>();
        let stats = cull.and_then(|p| p.stats_snapshot());
        let (hw, sw, dense) = stats.map_or((0, 0, 0), |st| st.tier_split());
        let pool = s.runtime.with_store(|store| store.stats());
        let tier3 = s.runtime.tier3_stats();
        let settings = s.runtime.settings();
        let tile_density = tile_density_json(s);
        serde_json::json!({
            "name": shot.name,
            "adapter": s.adapter_name,
            "resolution": [size.0, size.1],
            "frames": frames,
            "debug_view": shot.debug_view.clone().unwrap_or_else(|| "normal".into()),
            "camera": {
                "position": pose.position.to_array(),
                "look_at": pose.target.to_array(),
                "fov_deg": pose.fov_deg,
                "path": shot.camera.is_path(),
            },
            "tau_px": settings.tau_px,
            "vg_mode": format!("{:?}", settings.mode),
            "fog": shot.fog.clone().filter(|f| f.enabled),
            "sun": shot.sun,
            "lens": self.last_lens.map(|(f, n, d, on)| serde_json::json!({"enabled": on, "focal_length_mm": f, "f_number": n, "focus_distance_m": d})),
            "post": shot.post,
            "tier3_on": settings.tier3,
            "ray_tracing_requested": shot.ray_tracing,
            "ray_tracing_rendered": false,
            "scene": {
                "source_triangles": s.scene_triangles,
                "vg_assets": s.vg_meshes.len(),
                "vg_instances": s.vg_objects,
                "classic_instances": s.classic_objects,
            },
            "vg": {
                "selected_triangles": stats.map(|st| st.triangles),
                "clusters_tested": stats.map(|st| st.clusters_tested),
                "jobs": stats.map(|st| st.jobs),
                "root_shortcuts": stats.map(|st| st.root_shortcuts),
                "clusters_emitted": stats.map(|st| st.emitted),
                "tier_clusters": {"hw": hw, "sw": sw, "dense": dense},
                "tier_bins": stats.map(|st| st.tier_clusters.to_vec()),
                "culled": stats.map(|st| serde_json::json!({
                    "frustum": st.culled_frustum, "occlusion": st.culled_occlusion,
                    "cone": st.culled_cone, "error": st.culled_sse, "deferred": st.deferred,
                })),
                "overflow": stats.map(|st| serde_json::json!({
                    "jobs": st.job_overflow, "visible": st.visible_overflow, "peak_queue": st.peak_queue,
                })),
                "pool": pool.map(|p| serde_json::json!({
                    "resident_pages": p.resident_pages, "root_pages_resident": p.root_pages_resident,
                    "slot_capacity": p.slot_capacity, "slot_limit": p.slot_limit,
                    "resident_bytes": p.resident_bytes, "uploads": p.uploads, "evictions": p.evictions,
                    "failures": p.failures, "feedback_overflows": p.feedback_overflows,
                })),
                "tier3": {
                    "frames_nonrt": tier3.frames_nonrt, "frames_rt": tier3.frames_rt,
                    "rt_hardware": tier3.rt_hardware, "dense_tiles": tier3.dense_tiles,
                    "dense_tiles_processed": tier3.dense_tiles_processed, "peel_layers": tier3.peel_layers,
                    "list_total": tier3.list_total, "list_max": tier3.list_max, "tris_total": tier3.tris_total, "tris_max": tier3.tris_max,
                    "masked_tiles": tier3.masked_tiles, "masked_tris": tier3.masked_tris, "masked_tris_max": tier3.masked_tris_max,
                    "bin_overflow": tier3.bin_overflow, "slot_overflow": tier3.slot_overflow,
                    "clamped_tiles": tier3.clamped_tiles, "clamped_tris": tier3.clamped_tris,
                    "tlas_rebuilds": tier3.tlas_rebuilds, "tlas_reuses": tier3.tlas_reuses, "blas_builds": tier3.blas_builds,
                    "tlas_instances": tier3.tlas_instances, "candidates": tier3.candidates,
                    "rt_incomplete_frames": tier3.rt_incomplete_frames, "uncovered_readbacks": tier3.uncovered_readbacks,
                    "proxy_assets": tier3.proxy_assets, "proxy_bytes": tier3.proxy_bytes,
                    "rt_prepare_cpu_us": tier3.rt_prepare_cpu_us,
                },
            },
            "tile_density": tile_density,
            "bench": self.bench_json(s),
            "gpu_ms": gpu,
            "gpu_ms_note": "mean of the last 30 settle frames (asynchronous timestamp readback)",
            "capture_seconds": seconds,
        })
    }

    /// The instance-benchmark section of the stats JSON (`null` outside the benchmark).
    fn bench_json(&self, s: &AppState) -> serde_json::Value {
        let Some(b) = &s.bench else {
            return serde_json::Value::Null;
        };
        let tail = |pick: fn(&(f32, f32)) -> f32| {
            let n = self.samples.len();
            let from = n.saturating_sub(40);
            self.samples[from..].iter().map(pick).collect::<Vec<_>>()
        };
        let (calls, rebuilds, visible, derive_gpu_bytes) =
            s.targets.lock().map_or((0, 0, 0, 0), |t| {
                (t.derive_calls, t.derive_rebuilds, t.visible, t.gpu_bytes)
            });
        let instances = b.cfg.instances.max(1) as f64;
        let objects = b.spawned_objects.max(1) as f64;
        let bvh_scratch = s
            .renderer
            .find_pass::<helio_pass_vg_bvh::VgBvhPass>()
            .map_or(0, |p| p.scratch_bytes());
        let private = super::vg_stress_bench::private_bytes();
        serde_json::json!({
            "instances": b.cfg.instances,
            "asset": format!("{:?}", b.cfg.asset),
            "object_rows": b.spawned_objects,
            "classic_objects": b.classic_objects,
            "spawn": {
                "ms": b.spawn_ms,
                "us_per_instance": b.spawn_ms * 1000.0 / instances,
                "private_bytes_delta": b.private_after_spawn.saturating_sub(b.private_before_spawn),
                "private_bytes_per_instance": b.private_after_spawn.saturating_sub(b.private_before_spawn) as f64 / instances,
            },
            "cpu_frame_ms": super::vg_stress_bench::summarize(&tail(|x| x.0)),
            "wall_frame_ms": super::vg_stress_bench::summarize(&tail(|x| x.1)),
            "frames_in_shot": self.samples.len(),
            "bvh_frames": {"skip": self.bvh_modes[0], "refit": self.bvh_modes[1], "rebuild": self.bvh_modes[2]},
            "derivation": {
                "calls": calls - self.derive_baseline.0,
                "rebuilds": rebuilds - self.derive_baseline.1,
                "visible_instances": visible,
            },
            "memory": {
                "private_bytes": private,
                "private_bytes_before_assets": b.private_before_assets,
                "per_instance_total": private.saturating_sub(b.private_before_assets) as f64 / instances,
            },
            "gpu_bytes": {
                "static_objects_rows_x252": (objects * 252.0) as u64,
                "vg_instances_etc": derive_gpu_bytes,
                "bvh_scratch": bvh_scratch,
                "bvh_per_instance": bvh_scratch as f64 / instances,
                "instance_total_per_instance": ((objects * 252.0) + derive_gpu_bytes as f64 + bvh_scratch as f64) / instances,
            },
        })
    }

    fn save_image(
        &mut self,
        s: &AppState,
        path: &Path,
        size: (u32, u32),
        jpeg_quality: Option<u8>,
    ) -> Result<u64, String> {
        let (texture, _, _) = self.target.as_ref().ok_or("no target")?;
        let (buffer, _) = self.readback.as_ref().ok_or("no readback buffer")?;
        let row = padded_row(size.0);
        let mut encoder = s.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        s.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        s.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let rgb = {
            let mapped = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("{e:?}"))?;
            rgba_rows_to_rgb(&mapped, size, row as usize)
        };
        buffer.unmap();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = Vec::new();
        match jpeg_quality {
            Some(q) => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q).encode(
                &rgb,
                size.0,
                size.1,
                image::ExtendedColorType::Rgb8,
            ),
            None => {
                use image::ImageEncoder;
                image::codecs::png::PngEncoder::new_with_quality(
                    &mut out,
                    image::codecs::png::CompressionType::Best,
                    image::codecs::png::FilterType::Adaptive,
                )
                .write_image(&rgb, size.0, size.1, image::ExtendedColorType::Rgb8)
            }
        }
        .map_err(|e| e.to_string())?;
        std::fs::write(path, &out).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(out.len() as u64)
    }

    fn finish(&mut self) {
        // The index covers every shot in the directory (partial re-runs merge), skips included.
        let mut shots = scan_stats(&self.options.dir);
        for skipped in self.index.iter().filter(|e| e.get("skipped").is_some()) {
            shots.push(skipped.clone());
        }
        let index = serde_json::json!({
            "shots": shots,
            "directory_bytes": dir_bytes(&self.options.dir),
            "this_run_bytes": self.bytes_written,
        });
        let path = self.options.dir.join("index.json");
        let _ = std::fs::write(&path, serde_json::to_vec_pretty(&index).unwrap_or_default());
        println!(
            "[Capture] done: {} shots, {:.1} MB of images, index at {}",
            self.index.len(),
            self.bytes_written as f64 / 1e6,
            path.display()
        );
        self.phase = Phase::Done;
    }
}

fn pose_of(shot: &Shot, frames: u32) -> Pose {
    shot.camera.pose(frames.saturating_sub(1))
}

pub fn padded_row(width: u32) -> u32 {
    (width * 4).div_ceil(256) * 256
}

pub fn rgba_rows_to_rgb(mapped: &[u8], size: (u32, u32), row: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((size.0 * size.1 * 3) as usize);
    for y in 0..size.1 as usize {
        for px in mapped[y * row..y * row + size.0 as usize * 4].chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
        }
    }
    rgb
}

/// Resolve a shot's `debug_view` to a renderer debug mode among the available ones.
pub fn resolve_view(name: &str, views: &[helio_core::DebugViewDescriptor]) -> Option<u32> {
    let lower = name.trim().to_ascii_lowercase();
    let id = match lower.as_str() {
        "normal" | "beauty" | "none" => Some(0),
        "albedo" => Some(4),
        "lighting-only" | "lighting_only" | "lighting" => Some(22),
        "vis_normal" | "vis-normal" => Some(debug_view::VgDebugView::VisNormal.id()),
        "wireframe" => views
            .iter()
            .find(|v| v.name.to_ascii_lowercase().contains("wire"))
            .map(|v| v.debug_mode),
        other => debug_view::parse(other).map(|v| v.id()).or_else(|| {
            views
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(other))
                .map(|v| v.debug_mode)
        }),
    }?;
    (id == 0 || views.iter().any(|v| v.debug_mode == id)).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn views() -> Vec<helio_core::DebugViewDescriptor> {
        [0, 4, 22, 20, 21, 100, 102, 112]
            .map(|m| helio_core::DebugViewDescriptor {
                name: "x",
                debug_mode: m,
                description: "",
            })
            .to_vec()
    }

    #[test]
    fn default_list_has_every_blog_name_exactly_once() {
        let shots = default_shots();
        let names: Vec<&str> = shots.iter().map(|s| s.name.as_str()).collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "duplicate names");
        for needed in [
            "hero-temple-fog",
            "hero-courtyard-statues",
            "hero-forest-dusk",
            "statue-lucy-closeup",
            "statue-thai-closeup",
            "trees-canopy-closeup",
            "dbg-normal",
            "dbg-albedo",
            "dbg-lighting-only",
            "dbg-triangle-id",
            "dbg-cluster-id",
            "dbg-lod-level",
            "dbg-detail-vs-root",
            "dbg-triangle-size",
            "dbg-raster-tier",
            "dbg-sw-overdraw",
            "dbg-dense-overlap",
            "dbg-dense-mask",
            "dbg-page-residency",
            "dbg-culling-survival",
            "dbg-error-vs-tau",
            "dbg-instance-id",
            "dbg-barycentrics",
            "dbg-material-bin",
            "dbg-classic-vs-vg-mask",
            "hero-colonnade-sunshafts",
            "hero-lucy-portrait",
            "hero-thai-pedestal",
            "hero-dragon-court-rack-focus-a",
            "hero-dragon-court-rack-focus-b",
            "hero-pine-bokeh",
            "hero-interior-hall-dust",
            "lucy-3m-normal",
            "lucy-3m-triangles",
            "lucy-3m-clusters",
            "lucy-12m-normal",
            "lucy-48m-clusters",
            "lucy-150m-triangles",
            "tau-0p5",
            "tau-1",
            "tau-4",
            "tau-16",
            "tau-64",
            "tau-4-triangles",
            "tau-64-triangles",
            "tier3-off",
            "tier3-on-peel",
            "tier3-on-rt",
            "flythrough",
            "flythrough-debug",
        ] {
            assert!(names.contains(&needed), "missing {needed}");
        }
        // The debug gallery shares ONE pose so the images line up.
        let dbg: Vec<_> = shots
            .iter()
            .filter(|s| s.name.starts_with("dbg-"))
            .collect();
        assert!(dbg.iter().all(|s| s.camera == dbg[0].camera));
        // Every default shot is valid and every view resolves.
        let text = serde_json::to_string(&shots).unwrap();
        assert_eq!(parse_shots(&text).unwrap(), shots);
        // Debug views stay clean: no lens, no fog, no grading.
        for shot in shots.iter().filter(|s| s.debug_view.is_some()) {
            assert!(
                shot.lens.is_none() && shot.fog.is_none() && !shot.post,
                "{}",
                shot.name
            );
        }
        // Hero shots use the full stack and focus on a subject.
        for shot in shots.iter().filter(|s| s.name.starts_with("hero-")) {
            assert!(
                shot.lens
                    .as_ref()
                    .is_some_and(|l| l.focus_target.is_some() && l.f_number <= 4.0)
                    && shot.post,
                "{}",
                shot.name
            );
        }
    }

    #[test]
    fn committed_shots_json_matches_the_built_in_list() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/vg_stress/shots.json");
        let text = std::fs::read_to_string(&path).expect("shots.json is committed");
        assert_eq!(
            parse_shots(&text).unwrap(),
            default_shots(),
            "regenerate with the ignored write_shots_json test"
        );
    }

    #[test]
    #[ignore = "regenerates assets/vg_stress/shots.json from the built-in list"]
    fn write_shots_json() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/vg_stress/shots.json");
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({ "shots": default_shots() })).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn parsing_accepts_both_forms_and_rejects_bad_shots() {
        let one = r#"{"name":"a","camera":{"position":[0,1,2],"yaw_deg":90}}"#;
        assert_eq!(parse_shots(&format!("[{one}]")).unwrap().len(), 1);
        assert_eq!(
            parse_shots(&format!("{{\"shots\":[{one}]}}"))
                .unwrap()
                .len(),
            1
        );
        assert!(parse_shots(&format!("[{one},{one}]")).is_err(), "duplicate");
        assert!(
            parse_shots(r#"[{"name":"a/b","camera":{"position":[0,0,0]}}]"#).is_err(),
            "path in name"
        );
        assert!(parse_shots(r#"[{"name":"a"}]"#).is_err(), "no camera");
        assert!(parse_shots(r#"[{"name":"a","camera":{"keyframes":[{"t":1,"position":[0,0,0],"look_at":[0,0,1]},{"t":1,"position":[0,0,0],"look_at":[0,0,1]}]}}]"#).is_err());
        assert!(parse_shots("not json").is_err());
    }

    #[test]
    fn fixed_camera_yaw_zero_looks_down_negative_z() {
        let c = CameraSpec {
            position: Some([1.0, 2.0, 3.0]),
            yaw_deg: Some(0.0),
            pitch_deg: Some(0.0),
            ..Default::default()
        };
        let p = c.pose(0);
        assert!((p.target - glam::Vec3::new(1.0, 2.0, 2.0)).length() < 1e-5);
        let c = CameraSpec {
            position: Some([0.0; 3]),
            yaw_deg: Some(90.0),
            ..Default::default()
        };
        assert!((c.pose(0).target - glam::Vec3::X).length() < 1e-5);
        assert_eq!(c.frame_count(), 1);
    }

    fn path() -> CameraSpec {
        let k = |t, x| Keyframe {
            t,
            position: [x, 0.0, 0.0],
            look_at: [x, 0.0, -1.0],
            fov_deg: Some(40.0 + x),
        };
        CameraSpec {
            keyframes: vec![k(0.0, 0.0), k(1.0, 10.0), k(2.0, 20.0), k(3.0, 40.0)],
            duration_s: Some(3.0),
            fps: Some(10.0),
            ..Default::default()
        }
    }

    #[test]
    fn path_interpolation_hits_keys_is_monotonic_and_smooth() {
        let c = path();
        assert_eq!(c.frame_count(), 30);
        for (t, x) in [(0.0, 0.0), (1.0, 10.0), (2.0, 20.0), (3.0, 40.0)] {
            assert!((c.pose_at(t).position.x - x).abs() < 1e-4, "key at {t}");
        }
        let xs: Vec<f32> = (0..30).map(|f| c.pose(f).position.x).collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "monotonic: {xs:?}");
        // No velocity jump across a key: per-frame steps change gradually.
        let steps: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            steps.windows(2).all(|w| (w[1] - w[0]).abs() < 0.5),
            "{steps:?}"
        );
        // FOV interpolates with the keys; beyond the range it clamps.
        assert!((c.pose_at(0.5).fov_deg - 45.0).abs() < 1e-3);
        assert_eq!(c.pose_at(99.0).position.x, 40.0);
    }

    #[test]
    fn resolution_and_options_parse() {
        assert_eq!(parse_resolution("1440p"), Some((2560, 1440)));
        assert_eq!(parse_resolution("4K"), Some((3840, 2160)));
        assert_eq!(parse_resolution("1280x720"), Some((1280, 720)));
        assert_eq!(parse_resolution("0x5"), None);
        assert_eq!(parse_resolution("huge"), None);
        let none = |_: &str| None;
        assert!(Options::from_sources(&[], &none).is_none());
        let o = Options::from_sources(&["--capture".into(), "out".into()], &|k| {
            (k == "VG_STRESS_CAPTURE_RES").then(|| "720p".to_string())
        })
        .unwrap();
        assert_eq!((o.dir, o.size), (PathBuf::from("out"), (1280, 720)));
        let o = Options::from_sources(&[], &|k| match k {
            "VG_STRESS_CAPTURE" => Some("d".into()),
            "VG_STRESS_CAPTURE_ONLY" => Some("a, b".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(o.size, (1600, 900));
        assert_eq!(o.only, ["a", "b"]);
    }

    #[test]
    fn debug_views_resolve_by_key_alias_and_availability() {
        let v = views();
        assert_eq!(resolve_view("normal", &v), Some(0));
        assert_eq!(resolve_view("cluster", &v), Some(21));
        assert_eq!(resolve_view("tri_size", &v), Some(102));
        assert_eq!(resolve_view("vis_normal", &v), Some(112));
        assert_eq!(resolve_view("albedo", &v), Some(4));
        // Registered on the device or not at all.
        assert_eq!(resolve_view("tier", &v), None);
        assert_eq!(resolve_view("nonsense", &v), None);
    }

    #[test]
    fn rows_are_unpadded_and_sun_points_down() {
        assert_eq!(padded_row(1920), 7680);
        assert_eq!(padded_row(100), 512);
        let mapped = [
            1u8, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, 7, 8, 9, 255, 1, 1, 1, 255, 9, 9, 9, 9,
        ];
        let rgb = rgba_rows_to_rgb(&mapped, (2, 2), 12);
        assert_eq!(rgb, [1, 2, 3, 4, 5, 6, 7, 8, 9, 1, 1, 1]);
        let d = SunSpec {
            elevation_deg: 30.0,
            azimuth_deg: 90.0,
            ..Default::default()
        }
        .direction();
        assert!(
            d[1] < 0.0 && d[0] < 0.0,
            "sun in +X travels toward -X: {d:?}"
        );
    }
}

#[cfg(test)]
mod lens_tests {
    use super::*;

    #[test]
    fn focus_distance_and_field_of_view_follow_the_shot() {
        let pose = Pose {
            position: glam::Vec3::new(0.0, 0.0, 10.0),
            target: glam::Vec3::ZERO,
            fov_deg: 50.0,
        };
        let spec = LensSpec {
            focus_target: Some([0.0, 0.0, 4.0]),
            ..Default::default()
        };
        let (lens, fov) = lens_component(&spec, &pose, 16.0 / 9.0);
        assert!((lens.focus_distance_m - 6.0).abs() < 1e-4);
        assert!(
            (fov - 50.0).abs() < 1e-4,
            "no focal override keeps the camera fov"
        );
        // Without a target the look_at distance is the focus; the focal length matches the fov.
        let (lens, _) = lens_component(&LensSpec::default(), &pose, 16.0 / 9.0);
        assert!((lens.focus_distance_m - 10.0).abs() < 1e-4);
        let sensor_h = 36.0 / (16.0 / 9.0);
        assert!(
            ((2.0 * (sensor_h / (2.0 * lens.focal_length_mm)).atan()).to_degrees() - 50.0).abs()
                < 1e-3
        );
        // A focal-length override sets the field of view: 135 mm is much narrower than 35 mm.
        let long = LensSpec {
            focal_length_mm: Some(135.0),
            ..Default::default()
        };
        let wide = LensSpec {
            focal_length_mm: Some(35.0),
            ..Default::default()
        };
        let (_, f_long) = lens_component(&long, &pose, 16.0 / 9.0);
        let (_, f_wide) = lens_component(&wide, &pose, 16.0 / 9.0);
        assert!(f_long < 10.0 && f_wide > 30.0, "{f_long} {f_wide}");
    }
}

#[cfg(test)]
mod placement_dump {
    #[test]
    #[ignore]
    fn print_statue_placements() {
        use crate::vg_stress_data::{dims::Dims, layout::museum_layout_with};
        let layout = museum_layout_with(
            &Dims::load(),
            &crate::vg_stress_prep::catalog::Config::default(),
        );
        for p in &layout.placements {
            if p.virtual_geometry
                && !p.asset.contains('#')
                && (p.asset.contains("lucy")
                    || p.asset.contains("thai")
                    || p.asset.contains("dragon"))
            {
                eprintln!(
                    "{} pos {:.1?} rot {:.0?} scale {:.2?}",
                    p.asset.rsplit('/').next().unwrap(),
                    p.position,
                    p.rotation,
                    p.scale
                );
            }
        }
    }
}

/// The interactive post stack (`P`): 0 off, 1 standard (grading, bloom,
/// vignette), 2 cinematic (the same here: depth of field is the lens
/// plugin's, haze the fog plugin's). Installed lazily; the title shows the
/// level.
pub fn set_interactive_post(s: &mut AppState, level: u8) {
    if s.post_stack.is_none() {
        s.post_stack = Some(PostStack::install(&mut s.renderer));
    }
    if let Some(stack) = &s.post_stack {
        stack.set_on(&mut s.renderer, level >= 1);
    }
    s.post_level = level;
    println!(
        "[Museum] post stack: {}",
        [
            "off",
            "standard (grading, bloom, vignette)",
            "cinematic (grading, bloom, vignette)"
        ][level as usize]
    );
}

/// One index entry per `<name>.json` stats file in `dir`.
fn scan_stats(dir: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json")
            || path.file_stem().is_some_and(|s| s == "index")
        {
            continue;
        }
        let Some(stats) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            continue;
        };
        let Some(name) = stats["name"].as_str() else {
            continue;
        };
        let frames = stats["frames"].as_u64().unwrap_or(1);
        let ext = ["png", "jpg"]
            .into_iter()
            .find(|e| dir.join(format!("{name}.{e}")).exists());
        out.push(serde_json::json!({
            "name": name,
            "image": ext.map_or_else(|| format!("{name}/"), |e| format!("{name}.{e}")),
            "stats": format!("{name}.json"),
            "frames": frames,
            "selected_triangles": stats["vg"]["selected_triangles"],
            "gpu_ms": stats["gpu_ms"]["TOTAL"],
            "tier3_on": stats["tier3_on"],
        }));
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Distribution of the Tier 2 per-tile counters of the last frame (calibration).
/// Blocks on the GPU; capture mode only.
fn tile_density_json(s: &AppState) -> serde_json::Value {
    let Some(pass) = s
        .renderer
        .find_pass::<helio_pass_vg_raster::VgRasterEarlyPass>()
    else {
        return serde_json::Value::Null;
    };
    let Some((counters, tiles)) = pass.shared().tile_counters_blocking(&s.device, &s.queue) else {
        return serde_json::Value::Null;
    };
    let n = counters.len().max(1);
    let mut tris: Vec<u32> = counters.iter().map(|c| c[0]).collect();
    let mut hits: Vec<u32> = counters.iter().map(|c| c[1]).collect();
    let total_tris: u64 = tris.iter().map(|&v| u64::from(v)).sum();
    let total_hits: u64 = hits.iter().map(|&v| u64::from(v)).sum();
    tris.sort_unstable();
    hits.sort_unstable();
    let pct = |v: &[u32], p: f64| v[((v.len() as f64 - 1.0) * p) as usize];
    let over = |v: &[u32], t: u32| v.iter().filter(|&&x| x >= t).count();
    let ladder = [256u32, 512, 768, 1536, 3072, 6144, 12288, 24576, 49152];
    serde_json::json!({
        "tiles": [tiles[0], tiles[1]],
        "tris_total": total_tris, "hits_total": total_hits,
        "tris_max": tris[n - 1], "hits_max": hits[n - 1],
        "tris_p50": pct(&tris, 0.5), "tris_p99": pct(&tris, 0.99), "hits_p99": pct(&hits, 0.99),
        "tiles_with_tris_ge": ladder.iter().map(|&t| serde_json::json!({"threshold": t, "tiles": over(&tris, t)})).collect::<Vec<_>>(),
        "tiles_with_hits_ge": ladder.iter().map(|&t| serde_json::json!({"threshold": t, "tiles": over(&hits, t)})).collect::<Vec<_>>(),
    })
}
