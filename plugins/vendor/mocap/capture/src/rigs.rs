//! The rig profiles sources speak, and name-to-column lookup.

use mocap_format::{ChannelDesc, ChannelKind, Quantisation, RigProfile};
use std::collections::HashMap;

/// The VRM humanoid bones (the names VMC streams), each with its parent.
/// `Root` is the avatar's world transform; `Hips` hangs from it.
const VRM_BONES: &[(&str, Option<&str>)] = &[
    ("Root", None),
    ("Hips", Some("Root")),
    ("Spine", Some("Hips")),
    ("Chest", Some("Spine")),
    ("UpperChest", Some("Chest")),
    ("Neck", Some("UpperChest")),
    ("Head", Some("Neck")),
    ("LeftEye", Some("Head")),
    ("RightEye", Some("Head")),
    ("Jaw", Some("Head")),
    ("LeftShoulder", Some("UpperChest")),
    ("LeftUpperArm", Some("LeftShoulder")),
    ("LeftLowerArm", Some("LeftUpperArm")),
    ("LeftHand", Some("LeftLowerArm")),
    ("RightShoulder", Some("UpperChest")),
    ("RightUpperArm", Some("RightShoulder")),
    ("RightLowerArm", Some("RightUpperArm")),
    ("RightHand", Some("RightLowerArm")),
    ("LeftUpperLeg", Some("Hips")),
    ("LeftLowerLeg", Some("LeftUpperLeg")),
    ("LeftFoot", Some("LeftLowerLeg")),
    ("LeftToes", Some("LeftFoot")),
    ("RightUpperLeg", Some("Hips")),
    ("RightLowerLeg", Some("RightUpperLeg")),
    ("RightFoot", Some("RightLowerLeg")),
    ("RightToes", Some("RightFoot")),
];

const FINGERS: [&str; 5] = ["Thumb", "Index", "Middle", "Ring", "Little"];
const PHALANGES: [&str; 3] = ["Proximal", "Intermediate", "Distal"];

/// The VRM blendshape presets VMC streams.
pub const VRM_BLENDSHAPES: &[&str] = &[
    "Neutral", "A", "I", "U", "E", "O", "Blink", "Blink_L", "Blink_R", "Joy", "Angry", "Sorrow",
    "Fun", "LookUp", "LookDown", "LookLeft", "LookRight",
];

fn bone_list() -> Vec<(String, Option<String>)> {
    let mut bones: Vec<(String, Option<String>)> =
        VRM_BONES.iter().map(|(n, p)| ((*n).to_owned(), p.map(str::to_owned))).collect();
    for side in ["Left", "Right"] {
        for finger in FINGERS {
            let mut parent = format!("{side}Hand");
            for phalanx in PHALANGES {
                let name = format!("{side}{finger}{phalanx}");
                bones.push((name.clone(), Some(parent)));
                parent = name;
            }
        }
    }
    bones
}

/// The standard rig for VMC and the synthetic source: 55 bones plus `Root`,
/// a rotation per bone, positions for `Root` and `Hips`, and the VRM
/// blendshapes.
pub fn vrm_humanoid() -> RigProfile {
    let bones = bone_list();
    let index: HashMap<&str, u16> =
        bones.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i as u16)).collect();
    let joints: Vec<(&str, Option<u16>)> = bones
        .iter()
        .map(|(n, p)| (n.as_str(), p.as_deref().and_then(|p| index.get(p).copied())))
        .collect();
    let mut rig = RigProfile::humanoid("vrm-humanoid", &joints, VRM_BLENDSHAPES);
    rig.channels.push(ChannelDesc {
        name: "Hips.position".to_owned(),
        kind: ChannelKind::Position3,
        joint: index.get("Hips").copied(),
        quantisation: Quantisation::Lossless,
    });
    rig
}

/// Channel name to its first column, built once per rig.
#[derive(Clone, Debug, Default)]
pub struct RigIndex {
    columns: HashMap<String, usize>,
    rotations: Vec<usize>,
    width: usize,
}

impl RigIndex {
    /// Indexes `rig`.
    pub fn new(rig: &RigProfile) -> Self {
        let offsets = rig.column_offsets();
        let columns = rig.channels.iter().zip(&offsets).map(|(c, at)| (c.name.clone(), *at)).collect();
        let rotations = rig
            .channels
            .iter()
            .zip(&offsets)
            .filter(|(c, _)| c.kind == ChannelKind::Rotation4)
            .map(|(_, at)| *at)
            .collect();
        Self { columns, rotations, width: rig.columns() }
    }

    /// First column of every rotation channel.
    pub fn rotation_columns(&self) -> &[usize] {
        &self.rotations
    }

    /// First column of the channel called `name`.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns.get(name).copied()
    }

    /// Columns in a frame.
    pub fn width(&self) -> usize {
        self.width
    }
}
