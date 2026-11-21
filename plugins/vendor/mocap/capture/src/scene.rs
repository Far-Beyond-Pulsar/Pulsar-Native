//! The fused scene: every source's data as one world.
//!
//! A pose frame in a merged rig holds several sources side by side (a VMC
//! performer, a camera's body keypoints, an IMU). [`build_scene`] turns it into
//! the things a viewer draws: **figures** (bodies with joints, and keypoint
//! figures lifted from a camera's image), **axes** (an orientation sensor) and
//! **cameras**. Everything is placed in one metric space: +Y up, figures facing
//! +Z, character-left at +X, bodies spread along X, sensors in a row in front,
//! cameras further out in front, looking at the stage.
//!
//! Keypoints from one camera are 2D: they are shown as a flat figure standing
//! in the image's plane (flagged `flat`), not guessed into depth; several
//! calibrated cameras can triangulate them later.

use crate::keypoints::{COCO17, COCO17_EDGES};
use crate::skeleton::Skeleton;
use mocap_format::{ChannelKind, RigProfile};
use std::ops::Range;

/// Metres between neighbouring bodies.
const BODY_SPACING: f32 = 1.3;
/// Metres between neighbouring sensors, and how far in front of the stage they sit.
const SENSOR_SPACING: f32 = 0.45;
const SENSOR_DEPTH: f32 = 1.2;
/// Where cameras stand (in front of the stage, looking back at it).
const CAMERA_DEPTH: f32 = 3.0;
/// A torso (mid-shoulder to mid-hip) in metres, used to size a keypoint figure.
const TORSO_M: f32 = 0.5;
/// Height of the hips above the floor.
const HIPS_HEIGHT_M: f32 = 0.95;

/// What a figure was made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FigureSource {
    /// Joint rotations (VMC, a suit, the synthetic figure).
    Skeleton,
    /// A camera's body keypoints: flat, in the image plane.
    Keypoints,
}

/// A body to draw. A joint with a non-finite coordinate is absent (a gap).
#[derive(Clone, Debug)]
pub struct SceneFigure {
    pub label: String,
    pub source: FigureSource,
    pub joints: Vec<[f32; 3]>,
    pub bones: Vec<(usize, usize)>,
    /// Joint names (for colouring left and right).
    pub names: Vec<String>,
}

/// An orientation sensor: a triad (x, y, z axes) at `origin`.
#[derive(Clone, Debug)]
pub struct SceneAxes {
    pub label: String,
    pub origin: [f32; 3],
    /// The rotated x, y and z axes, each of length 0.25 m.
    pub axes: [[f32; 3]; 3],
}

/// A camera standing in the scene.
#[derive(Clone, Debug)]
pub struct SceneCamera {
    pub label: String,
    pub position: [f32; 3],
    pub forward: [f32; 3],
}

/// Everything in the scene.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub figures: Vec<SceneFigure>,
    pub axes: Vec<SceneAxes>,
    pub cameras: Vec<SceneCamera>,
}

impl Scene {
    /// True when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.figures.is_empty() && self.axes.is_empty() && self.cameras.is_empty()
    }
}

/// One source's slice of a merged rig.
struct Group {
    /// The source id (`""` for an unmerged rig).
    label: String,
    rig: RigProfile,
    columns: Range<usize>,
}

/// Splits a (possibly merged) rig into its sources: channel names are
/// `<source>/<name>` when merged. Channels of one source are contiguous.
fn groups(rig: &RigProfile) -> Vec<Group> {
    let offsets = rig.column_offsets();
    let prefix = |name: &str| name.split_once('/').map_or(String::new(), |(p, _)| p.to_owned());
    let mut out: Vec<Group> = Vec::new();
    let mut first_joint = 0usize;
    let mut i = 0;
    while i < rig.channels.len() {
        let label = prefix(&rig.channels[i].name);
        let mut j = i;
        while j < rig.channels.len() && prefix(&rig.channels[j].name) == label {
            j += 1;
        }
        let joints: Vec<_> = rig
            .joints
            .iter()
            .enumerate()
            .skip(first_joint)
            .take_while(|(_, joint)| prefix(&joint.name) == label)
            .collect();
        let base = joints.first().map_or(0, |(k, _)| *k);
        first_joint = base + joints.len();
        let strip = |name: &str| name.split_once('/').map_or(name.to_owned(), |(_, rest)| rest.to_owned());
        let sub = RigProfile {
            name: label.clone(),
            joints: joints
                .iter()
                .map(|(_, joint)| mocap_format::Joint {
                    name: strip(&joint.name),
                    parent: joint.parent.and_then(|p| usize::from(p).checked_sub(base)).and_then(|p| u16::try_from(p).ok()),
                })
                .collect(),
            channels: rig.channels[i..j]
                .iter()
                .map(|c| mocap_format::ChannelDesc {
                    name: strip(&c.name),
                    joint: c.joint.and_then(|k| usize::from(k).checked_sub(base)).and_then(|k| u16::try_from(k).ok()),
                    ..c.clone()
                })
                .collect(),
        };
        let columns = offsets[i]..offsets.get(j).copied().unwrap_or_else(|| rig.columns());
        out.push(Group { label, rig: sub, columns });
        i = j;
    }
    out
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = cross(u, v).map(|c| 2.0 * c);
    let ut = cross(u, t);
    [v[0] + q[3] * t[0] + ut[0], v[1] + q[3] * t[1] + ut[1], v[2] + q[3] * t[2] + ut[2]]
}

/// Lifts COCO keypoints (`(x, y, score)` image fractions) into a flat figure in
/// metres, standing with its hips at hip height at x = `centre`. `aspect` is the
/// frame's width over height, so a pixel is square.
pub fn lift_keypoints(points: &[[f32; 3]], aspect: f32, centre: f32) -> Vec<[f32; 3]> {
    let at = |i: usize| points.get(i).filter(|p| p[0].is_finite() && p[1].is_finite()).copied();
    let mid = |a: usize, b: usize| match (at(a), at(b)) {
        (Some(p), Some(q)) => Some([(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5]),
        _ => None,
    };
    // Units: x in frame widths scaled by the aspect, y in frame heights.
    let units = |p: [f32; 2]| [p[0] * aspect, p[1]];
    let (shoulders, hips) = (mid(5, 6), mid(11, 12));
    let (scale, pivot, lift) = match (shoulders, hips) {
        (Some(s), Some(h)) => {
            let (s, h) = (units(s), units(h));
            let torso = ((s[0] - h[0]).powi(2) + (s[1] - h[1]).powi(2)).sqrt().max(1e-3);
            (TORSO_M / torso, h, HIPS_HEIGHT_M)
        }
        _ => (1.8, [0.5 * aspect, 1.0], 0.0),
    };
    points
        .iter()
        .map(|p| {
            if !(p[0].is_finite() && p[1].is_finite()) {
                return [f32::NAN; 3];
            }
            let u = units([p[0], p[1]]);
            [centre + (u[0] - pivot[0]) * scale, lift + (pivot[1] - u[1]) * scale, 0.0]
        })
        .collect()
}

/// Builds the scene from a merged `rig`, one frame of its `values`, the frame
/// aspect ratio of the cameras (`16/9` is assumed when unknown) and the running
/// cameras' `(id, label)`.
pub fn build_scene(rig: &RigProfile, values: &[f32], camera_aspect: f32, cameras: &[(String, String)]) -> Scene {
    let mut scene = Scene::default();
    let aspect = if camera_aspect > 0.0 { camera_aspect } else { 16.0 / 9.0 };
    let all = groups(rig);
    let bodies = all
        .iter()
        .filter(|g| !g.rig.joints.is_empty() || g.rig.channels.iter().any(|c| c.name.starts_with("kp.")))
        .count();
    let (mut body, mut sensor) = (0usize, 0usize);
    for group in &all {
        let Some(slice) = values.get(group.columns.clone()) else { continue };
        let label = if group.label.is_empty() { "scene".to_owned() } else { group.label.clone() };
        let slot = (body as f32 - (bodies as f32 - 1.0) * 0.5) * BODY_SPACING;
        if !group.rig.joints.is_empty() {
            let skeleton = Skeleton::new(&group.rig);
            let joints = skeleton.joints(slice).into_iter().map(|p| [p[0] + slot, p[1], p[2]]).collect();
            scene.figures.push(SceneFigure {
                label,
                source: FigureSource::Skeleton,
                joints,
                bones: skeleton.bones(),
                names: group.rig.joints.iter().map(|j| j.name.clone()).collect(),
            });
            body += 1;
        } else if group.rig.channels.iter().any(|c| c.name.starts_with("kp.")) {
            let points: Vec<[f32; 3]> = slice.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect();
            scene.figures.push(SceneFigure {
                label,
                source: FigureSource::Keypoints,
                joints: lift_keypoints(&points, aspect, slot),
                bones: COCO17_EDGES.to_vec(),
                names: COCO17.iter().map(|n| (*n).to_owned()).collect(),
            });
            body += 1;
        } else {
            let origin = [(sensor as f32) * SENSOR_SPACING - 0.5, 1.0, SENSOR_DEPTH];
            for (channel, at) in group.rig.channels.iter().zip(group.rig.column_offsets()) {
                if channel.kind != ChannelKind::Rotation4 {
                    continue;
                }
                let Some(q) = slice.get(at..at + 4).filter(|q| q.iter().all(|v| v.is_finite())) else { continue };
                let q = [q[0], q[1], q[2], q[3]];
                let axes = [[0.25, 0.0, 0.0], [0.0, 0.25, 0.0], [0.0, 0.0, 0.25]].map(|a| rotate(q, a));
                scene.axes.push(SceneAxes { label: format!("{label} {}", channel.name), origin, axes });
                sensor += 1;
            }
            if group.rig.channels.iter().all(|c| c.kind != ChannelKind::Rotation4) {
                sensor += 1;
            }
        }
    }
    for (i, (_, label)) in cameras.iter().enumerate() {
        let x = (i as f32 - (cameras.len() as f32 - 1.0) * 0.5) * 1.6;
        scene.cameras.push(SceneCamera { label: label.clone(), position: [x, 1.4, CAMERA_DEPTH], forward: [0.0, -0.05, -1.0] });
    }
    scene
}
