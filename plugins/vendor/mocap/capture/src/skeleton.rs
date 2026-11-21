//! Forward kinematics for previews: joint positions from a pose frame.
//!
//! Rest offsets are a generic adult humanoid in a T-pose, metres, +Y up, the
//! character facing +Z. A pose's rotations are local to each joint (as VMC and
//! the VRM humanoid send them); a position channel on a joint replaces its
//! rest offset (the root and the hips carry one).

use mocap_format::{ChannelKind, RigProfile};

/// A joint's offset from its parent in the T-pose.
pub fn rest_offset(name: &str) -> [f32; 3] {
    // A merged rig namespaces joints as `<source>/<bone>`.
    let name = name.rsplit('/').next().unwrap_or(name);
    let (side, base) = match name.strip_prefix("Left") {
        Some(rest) => (-1.0, rest),
        None => match name.strip_prefix("Right") {
            Some(rest) => (1.0, rest),
            None => (0.0, name),
        },
    };
    // The character's left is +X: a "Left" joint sits at +X.
    let x = -side;
    match base {
        "Root" => [0.0; 3],
        "Hips" => [0.0, 0.95, 0.0],
        "Spine" => [0.0, 0.10, 0.0],
        "Chest" => [0.0, 0.13, 0.0],
        "UpperChest" => [0.0, 0.12, 0.0],
        "Neck" => [0.0, 0.14, 0.0],
        "Head" => [0.0, 0.10, 0.0],
        "Eye" => [x * 0.032, 0.08, 0.08],
        "Jaw" => [0.0, 0.02, 0.05],
        "Shoulder" => [x * 0.04, 0.10, 0.0],
        "UpperArm" => [x * 0.10, 0.0, 0.0],
        "LowerArm" => [x * 0.27, 0.0, 0.0],
        "Hand" => [x * 0.25, 0.0, 0.0],
        "UpperLeg" => [x * 0.09, -0.05, 0.0],
        "LowerLeg" => [0.0, -0.43, 0.0],
        "Foot" => [0.0, -0.42, 0.0],
        "Toes" => [0.0, -0.06, 0.13],
        _ => finger_offset(base, x),
    }
}

fn finger_offset(base: &str, x: f32) -> [f32; 3] {
    let lateral = |z: f32| [x * 0.035, 0.0, z];
    let step = [x * 0.03, 0.0, 0.0];
    match base {
        "ThumbProximal" => [x * 0.02, -0.005, 0.025],
        "IndexProximal" => lateral(0.03),
        "MiddleProximal" => lateral(0.012),
        "RingProximal" => lateral(-0.008),
        "LittleProximal" => lateral(-0.026),
        _ if base.ends_with("Intermediate") || base.ends_with("Distal") => step,
        _ => [0.0; 3],
    }
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let w = q[3];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    };
    let t = cross(u, v).map(|c| 2.0 * c);
    let ut = cross(u, t);
    [v[0] + w * t[0] + ut[0], v[1] + w * t[1] + ut[1], v[2] + w * t[2] + ut[2]]
}

fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    mocap_format::signal::quat_mul(a, b)
}

/// A rig's joints prepared for forward kinematics.
#[derive(Clone, Debug)]
pub struct Skeleton {
    parents: Vec<Option<usize>>,
    rest: Vec<[f32; 3]>,
    rotation_column: Vec<Option<usize>>,
    position_column: Vec<Option<usize>>,
    /// Sideways shift of each root joint so several figures stand apart.
    root_shift: Vec<f32>,
}

/// Metres between figures when a rig holds several.
const FIGURE_SPACING: f32 = 1.2;

impl Skeleton {
    /// Prepares `rig`.
    pub fn new(rig: &RigProfile) -> Self {
        let n = rig.joints.len();
        let mut skeleton = Self {
            parents: rig.joints.iter().map(|j| j.parent.map(usize::from)).collect(),
            rest: rig.joints.iter().map(|j| rest_offset(&j.name)).collect(),
            rotation_column: vec![None; n],
            position_column: vec![None; n],
            root_shift: Self::figure_spacing(rig),
        };
        for (channel, at) in rig.channels.iter().zip(rig.column_offsets()) {
            let Some(joint) = channel.joint.map(usize::from).filter(|j| *j < n) else { continue };
            match channel.kind {
                ChannelKind::Rotation4 => skeleton.rotation_column[joint] = Some(at),
                ChannelKind::Position3 => skeleton.position_column[joint] = Some(at),
                _ => {}
            }
        }
        skeleton
    }

    fn figure_spacing(rig: &RigProfile) -> Vec<f32> {
        let roots = rig.joints.iter().filter(|j| j.parent.is_none()).count();
        let mut seen = 0usize;
        rig.joints
            .iter()
            .map(|j| {
                if j.parent.is_some() || roots < 2 {
                    return 0.0;
                }
                seen += 1;
                (seen as f32 - 1.0 - (roots as f32 - 1.0) * 0.5) * FIGURE_SPACING
            })
            .collect()
    }

    /// Number of joints.
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    /// True for a rig with no joints.
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }

    /// `(parent, child)` pairs: the bones to draw.
    pub fn bones(&self) -> Vec<(usize, usize)> {
        self.parents.iter().enumerate().filter_map(|(j, p)| p.map(|p| (p, j))).collect()
    }

    /// World positions of every joint for `values` (a pose frame's columns).
    /// Gaps in a rotation read as identity and in a position as the rest offset.
    pub fn joints(&self, values: &[f32]) -> Vec<[f32; 3]> {
        let n = self.len();
        let mut positions = vec![[0.0f32; 3]; n];
        let mut rotations = vec![[0.0, 0.0, 0.0, 1.0f32]; n];
        for j in 0..n {
            let local_q = self.rotation_column[j]
                .and_then(|c| values.get(c..c + 4))
                .filter(|q| q.iter().all(|v| v.is_finite()))
                .map_or([0.0, 0.0, 0.0, 1.0], |q| [q[0], q[1], q[2], q[3]]);
            let offset = self.position_column[j]
                .and_then(|c| values.get(c..c + 3))
                .filter(|p| p.iter().all(|v| v.is_finite()))
                .map_or(self.rest[j], |p| [p[0], p[1], p[2]]);
            match self.parents[j] {
                Some(p) => {
                    let at = rotate(rotations[p], offset);
                    positions[j] = [positions[p][0] + at[0], positions[p][1] + at[1], positions[p][2] + at[2]];
                    rotations[j] = mul(rotations[p], local_q);
                }
                None => {
                    positions[j] = [offset[0] + self.root_shift[j], offset[1], offset[2]];
                    rotations[j] = local_q;
                }
            }
        }
        positions
    }
}
