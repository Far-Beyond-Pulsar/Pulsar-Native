//! The fused scene: all captured data as one subject in one world.
//!
//! For now the scene assumes a single subject (marking and labelling several
//! subjects comes later): every body-shaped source is fused into one
//! character, per joint.
//!
//! - **Body sources** (VMC, a suit, the synthetic figure): joint rotations (and
//!   root positions) are averaged by joint name across sources, then posed by
//!   forward kinematics on the richest source's skeleton.
//! - **Camera keypoints**: the best camera's keypoints (highest total
//!   confidence this frame; no geometry is invented between uncalibrated views)
//!   are lifted into the scene and fill the joints no body source observes. With
//!   no body source at all they are the character: a flat figure in the image's
//!   plane, not guessed into depth.
//! - **Sensors**: each orientation channel is an axis triad attached beside the
//!   character, listed as an unassigned sensor until subjects are labelled.
//! - **Cameras** are scene objects, not part of the subject.
//!
//! One metric space: +Y up, the subject facing +Z with its left at +X, cameras
//! in front looking at it.

use crate::keypoints::{COCO17, COCO17_EDGES};
use crate::skeleton::Skeleton;
use mocap_format::signal::{nlerp, normalise};
use mocap_format::{ChannelKind, RigProfile};
use std::ops::Range;

/// A torso (mid-shoulder to mid-hip) in metres, used to size lifted keypoints.
const TORSO_M: f32 = 0.5;
/// Height of the hips above the floor.
const HIPS_HEIGHT_M: f32 = 0.95;
/// Where cameras stand (in front of the stage, looking back at it).
const CAMERA_DEPTH: f32 = 3.0;
/// Sideways spacing of sensor triads beside the subject.
const SENSOR_SPACING: f32 = 0.35;

/// COCO keypoint index to the skeleton joint it stands for (the limb joints).
const KEYPOINT_JOINTS: [(usize, &str); 12] = [
    (5, "LeftUpperArm"),
    (7, "LeftLowerArm"),
    (9, "LeftHand"),
    (6, "RightUpperArm"),
    (8, "RightLowerArm"),
    (10, "RightHand"),
    (11, "LeftUpperLeg"),
    (13, "LeftLowerLeg"),
    (15, "LeftFoot"),
    (12, "RightUpperLeg"),
    (14, "RightLowerLeg"),
    (16, "RightFoot"),
];

/// What the character's geometry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FigureSource {
    /// A skeleton (possibly with limb joints filled from camera keypoints).
    Skeleton,
    /// Only one camera's keypoints: flat, in the image plane.
    Keypoints,
    /// Only keypoints triangulated from calibrated cameras: real 3D.
    Triangulated,
}

/// The character's geometry. A joint with a non-finite coordinate is absent.
#[derive(Clone, Debug)]
pub struct SceneFigure {
    pub source: FigureSource,
    pub joints: Vec<[f32; 3]>,
    pub bones: Vec<(usize, usize)>,
    /// Joint names (for colouring left and right).
    pub names: Vec<String>,
    /// Indices of joints filled from camera keypoints rather than a body source.
    pub from_keypoints: Vec<usize>,
}

/// One source's part in the subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contribution {
    pub source: String,
    pub what: &'static str,
}

/// The single subject: one character fed by every body-shaped source.
#[derive(Clone, Debug)]
pub struct Subject {
    pub label: String,
    pub figure: SceneFigure,
    pub contributions: Vec<Contribution>,
}

/// An orientation sensor: a triad (x, y, z axes) at `origin`.
#[derive(Clone, Debug)]
pub struct SceneAxes {
    pub label: String,
    pub origin: [f32; 3],
    /// The rotated x, y and z axes, each 0.25 m long.
    pub axes: [[f32; 3]; 3],
}

/// A running camera, with its solved pose (`position`, `forward`) when calibrated.
#[derive(Clone, Debug)]
pub struct CameraInput {
    pub label: String,
    pub pose: Option<([f32; 3], [f32; 3])>,
}

/// A camera standing in the scene.
#[derive(Clone, Debug)]
pub struct SceneCamera {
    pub label: String,
    pub position: [f32; 3],
    pub forward: [f32; 3],
    /// True when the pose came from a calibration rather than a default slot.
    pub calibrated: bool,
}

/// Everything in the scene.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub subject: Option<Subject>,
    pub axes: Vec<SceneAxes>,
    pub cameras: Vec<SceneCamera>,
}

impl Scene {
    /// True when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.subject.is_none() && self.axes.is_empty() && self.cameras.is_empty()
    }
}

/// One source's slice of a merged rig, with its own local names.
struct Group {
    label: String,
    rig: RigProfile,
    columns: Range<usize>,
}

fn strip(name: &str) -> String {
    name.split_once('/').map_or(name.to_owned(), |(_, rest)| rest.to_owned())
}

fn prefix(name: &str) -> String {
    name.split_once('/').map_or(String::new(), |(p, _)| p.to_owned())
}

/// Splits a (possibly merged) rig into its sources. Channels and joints of one
/// source are contiguous.
fn groups(rig: &RigProfile) -> Vec<Group> {
    let offsets = rig.column_offsets();
    let mut out: Vec<Group> = Vec::new();
    let (mut first_joint, mut i) = (0usize, 0usize);
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
        let local = |index: Option<u16>| index.and_then(|p| usize::from(p).checked_sub(base)).and_then(|p| u16::try_from(p).ok());
        let sub = RigProfile {
            name: label.clone(),
            joints: joints
                .iter()
                .map(|(_, joint)| mocap_format::Joint { name: strip(&joint.name), parent: local(joint.parent) })
                .collect(),
            channels: rig.channels[i..j]
                .iter()
                .map(|c| mocap_format::ChannelDesc { name: strip(&c.name), joint: local(c.joint), ..c.clone() })
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

fn is_world_keypoint_group(g: &Group) -> bool {
    g.rig.joints.is_empty() && g.rig.channels.iter().any(|c| c.name.starts_with("kp3d."))
}

fn is_keypoint_group(g: &Group) -> bool {
    g.rig.joints.is_empty() && g.rig.channels.iter().any(|c| c.name.starts_with("kp."))
}

/// Fuses the body groups' rotations and root positions by channel name onto the
/// richest group's rig: rotations are averaged on the shorter arc, positions
/// averaged. A channel no source observes stays a gap.
fn fuse_bodies(bodies: &[(&Group, &[f32])]) -> Option<(RigProfile, Vec<f32>)> {
    let (primary, _) = bodies.iter().max_by_key(|(g, _)| g.rig.joints.len())?;
    let mut fused = vec![f32::NAN; primary.rig.columns()];
    for (channel, at) in primary.rig.channels.iter().zip(primary.rig.column_offsets()) {
        let width = channel.kind.width();
        let mut seen: Vec<&[f32]> = Vec::new();
        for (group, slice) in bodies {
            let Some(index) = group.rig.channels.iter().position(|c| c.name == channel.name) else { continue };
            let from = group.rig.column_offsets()[index];
            if let Some(v) = slice.get(from..from + width).filter(|v| v.iter().all(|x| x.is_finite())) {
                seen.push(v);
            }
        }
        let Some(first) = seen.first() else { continue };
        let out = &mut fused[at..at + width];
        match channel.kind {
            ChannelKind::Rotation4 => {
                let mut mean = [first[0], first[1], first[2], first[3]];
                for (n, q) in seen.iter().enumerate().skip(1) {
                    mean = nlerp(mean, [q[0], q[1], q[2], q[3]], 1.0 / (n as f32 + 1.0));
                }
                out.copy_from_slice(&normalise(mean));
            }
            _ => {
                for (k, slot) in out.iter_mut().enumerate() {
                    *slot = seen.iter().map(|v| v[k]).sum::<f32>() / seen.len() as f32;
                }
            }
        }
    }
    Some((primary.rig.clone(), fused))
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
    let units = |p: [f32; 2]| [p[0] * aspect, p[1]];
    let (scale, pivot, lift) = match (mid(5, 6), mid(11, 12)) {
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

fn total_score(points: &[[f32; 3]]) -> f32 {
    points.iter().filter(|p| p[0].is_finite() && p[2].is_finite()).map(|p| p[2]).sum()
}

/// The keypoints of the camera that sees the subject best this frame.
fn best_keypoints(groups: &[(&Group, &[f32])]) -> Option<(String, Vec<[f32; 3]>)> {
    groups
        .iter()
        .map(|(g, slice)| (g.label.clone(), slice.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>()))
        .filter(|(_, points)| total_score(points) > 0.0)
        .max_by(|a, b| total_score(&a.1).total_cmp(&total_score(&b.1)))
}

fn mean_x(joints: &[[f32; 3]]) -> f32 {
    let finite: Vec<f32> = joints.iter().filter(|j| j.iter().all(|v| v.is_finite())).map(|j| j[0]).collect();
    if finite.is_empty() {
        0.0
    } else {
        finite.iter().sum::<f32>() / finite.len() as f32
    }
}

/// Poses the fused body and fills unobserved limb joints from keypoints.
/// Keypoints to fill a body with: triangulated (world metres) or one camera's image points.
enum Keypoints<'a> {
    World(&'a [[f32; 3]]),
    Image(&'a [[f32; 3]]),
}

fn body_figure(rig: &RigProfile, values: &[f32], keypoints: Option<Keypoints<'_>>, aspect: f32) -> SceneFigure {
    let skeleton = Skeleton::new(rig);
    let mut joints = skeleton.joints(values);
    let names: Vec<String> = rig.joints.iter().map(|j| j.name.clone()).collect();
    let mut from_keypoints = Vec::new();
    if let Some(points) = keypoints {
        let (lifted, aligned) = match points {
            Keypoints::World(p) => (p.to_vec(), true),
            Keypoints::Image(p) => (lift_keypoints(p, aspect, 0.0), false),
        };
        let hips = names.iter().position(|n| n == "Hips").map(|i| joints[i]);
        let mid = match (lifted.get(11), lifted.get(12)) {
            (Some(l), Some(r)) if l[0].is_finite() && r[0].is_finite() => Some([(l[0] + r[0]) * 0.5, (l[1] + r[1]) * 0.5]),
            _ => None,
        };
        // Triangulated points are already in the world; one camera's lifted points are moved to the hips.
        let shift = match (hips, mid) {
            (Some(h), Some(m)) if !aligned => [h[0] - m[0], h[1] - m[1], h[2]],
            _ => [0.0, 0.0, 0.0],
        };
        for (kp, name) in KEYPOINT_JOINTS {
            let Some(joint) = names.iter().position(|n| n == name) else { continue };
            let observed = rig
                .channel_index(&format!("{name}.rotation"))
                .map(|c| rig.column_offsets()[c])
                .is_some_and(|at| values[at..at + 4].iter().all(|v| v.is_finite()));
            if let Some(p) = lifted.get(kp).filter(|p| p[0].is_finite() && !observed) {
                joints[joint] = if aligned { [p[0], p[1], p[2]] } else { [p[0] + shift[0], p[1] + shift[1], shift[2]] };
                from_keypoints.push(joint);
            }
        }
    }
    SceneFigure { source: FigureSource::Skeleton, joints, bones: skeleton.bones(), names, from_keypoints }
}

/// The best-seen triangulated keypoints: the group with the most finite points.
fn world_keypoints(groups: &[(&Group, &[f32])]) -> Option<(String, Vec<[f32; 3]>)> {
    groups
        .iter()
        .map(|(g, slice)| (g.label.clone(), slice.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>()))
        .filter(|(_, points)| points.iter().any(finite_point))
        .max_by_key(|(_, points)| points.iter().filter(|p| finite_point(p)).count())
}

fn finite_point(p: &[f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite())
}

/// Builds the scene from a merged `rig`, one frame of its `values`, the cameras'
/// frame aspect ratio (`16/9` when unknown) and the running cameras. A calibrated
/// camera stands where the calibration put it; others are spread in front.
pub fn build_scene(rig: &RigProfile, values: &[f32], camera_aspect: f32, cameras: &[CameraInput]) -> Scene {
    let aspect = if camera_aspect > 0.0 { camera_aspect } else { 16.0 / 9.0 };
    let all = groups(rig);
    let slice_of = |g: &Group| values.get(g.columns.clone());
    let label = |g: &Group| if g.label.is_empty() { "scene".to_owned() } else { g.label.clone() };
    let bodies: Vec<(&Group, &[f32])> = all.iter().filter(|g| !g.rig.joints.is_empty()).filter_map(|g| Some((g, slice_of(g)?))).collect();
    let image_groups: Vec<(&Group, &[f32])> = all.iter().filter(|g| is_keypoint_group(g)).filter_map(|g| Some((g, slice_of(g)?))).collect();
    let world_groups: Vec<(&Group, &[f32])> = all.iter().filter(|g| is_world_keypoint_group(g)).filter_map(|g| Some((g, slice_of(g)?))).collect();
    // Triangulated points beat any one camera's flat points.
    let world = world_keypoints(&world_groups);
    let image = if world.is_some() { None } else { best_keypoints(&image_groups) };
    let mut scene = Scene::default();
    let mut contributions: Vec<Contribution> = Vec::new();
    let figure = match fuse_bodies(&bodies) {
        Some((body_rig, fused)) => {
            contributions.extend(bodies.iter().map(|(g, _)| Contribution { source: label(g), what: "body" }));
            let fill = match (&world, &image) {
                (Some((_, p)), _) => Some(Keypoints::World(p)),
                (None, Some((_, p))) => Some(Keypoints::Image(p)),
                _ => None,
            };
            let figure = body_figure(&body_rig, &fused, fill, aspect);
            if !figure.from_keypoints.is_empty() {
                if let Some((source, what)) = world.as_ref().map(|(s, _)| (s, "keypoints (3D)")).or(image.as_ref().map(|(s, _)| (s, "keypoints"))) {
                    contributions.push(Contribution { source: source.clone(), what });
                }
            }
            Some(figure)
        }
        None => match (world, image) {
            (Some((source, points)), _) => {
                contributions.push(Contribution { source, what: "keypoints (3D)" });
                Some(SceneFigure {
                    source: FigureSource::Triangulated,
                    joints: points.iter().map(|p| if finite_point(p) { *p } else { [f32::NAN; 3] }).collect(),
                    bones: COCO17_EDGES.to_vec(),
                    names: COCO17.iter().map(|n| (*n).to_owned()).collect(),
                    from_keypoints: Vec::new(),
                })
            }
            (None, Some((source, points))) => {
                contributions.push(Contribution { source, what: "keypoints" });
                Some(SceneFigure {
                    source: FigureSource::Keypoints,
                    joints: lift_keypoints(&points, aspect, 0.0),
                    bones: COCO17_EDGES.to_vec(),
                    names: COCO17.iter().map(|n| (*n).to_owned()).collect(),
                    from_keypoints: Vec::new(),
                })
            }
            (None, None) => None,
        },
    };
    let anchor = figure.as_ref().map_or(0.0, |f| mean_x(&f.joints));
    let mut sensors = 0usize;
    let sensor_groups = all.iter().filter(|g| g.rig.joints.is_empty() && !is_keypoint_group(g) && !is_world_keypoint_group(g));
    for group in sensor_groups {
        let Some(slice) = slice_of(group) else { continue };
        for (channel, at) in group.rig.channels.iter().zip(group.rig.column_offsets()) {
            let Some(q) = slice.get(at..at + 4).filter(|q| channel.kind == ChannelKind::Rotation4 && q.iter().all(|v| v.is_finite())) else {
                continue;
            };
            let q = [q[0], q[1], q[2], q[3]];
            let origin = [anchor + 0.6 + sensors as f32 * SENSOR_SPACING, 1.2, 0.0];
            let axes = [[0.25, 0.0, 0.0], [0.0, 0.25, 0.0], [0.0, 0.0, 0.25]].map(|a| rotate(q, a));
            scene.axes.push(SceneAxes { label: format!("{} {}", label(group), channel.name), origin, axes });
            sensors += 1;
        }
        contributions.push(Contribution { source: label(group), what: "sensor" });
    }
    scene.subject = figure.map(|figure| Subject { label: "Subject 1".to_owned(), figure, contributions });
    for (i, camera) in cameras.iter().enumerate() {
        let (position, forward) = camera.pose.unwrap_or_else(|| {
            let x = (i as f32 - (cameras.len() as f32 - 1.0) * 0.5) * 1.6;
            ([x, 1.4, CAMERA_DEPTH], [0.0, -0.05, -1.0])
        });
        scene.cameras.push(SceneCamera { label: camera.label.clone(), position, forward, calibrated: camera.pose.is_some() });
    }
    scene
}
