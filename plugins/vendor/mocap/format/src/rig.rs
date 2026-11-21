//! The channel schema of a take: which joints and signals it carries.

use crate::error::FormatError;
use serde::{Deserialize, Serialize};

/// What one channel measures; fixes how many `f32` columns it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    /// A position in metres: x, y, z.
    Position3,
    /// A unit quaternion: x, y, z, w.
    Rotation4,
    /// One value (a blendshape weight, a finger curl).
    Scalar,
    /// A 0..1 tracking confidence.
    Confidence,
}

impl ChannelKind {
    /// How many columns a channel of this kind occupies.
    pub fn width(self) -> usize {
        match self {
            Self::Position3 => 3,
            Self::Rotation4 => 4,
            Self::Scalar | Self::Confidence => 1,
        }
    }
}

/// How a channel's samples are stored in a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantisation {
    /// Bit-exact `f32`.
    #[default]
    Lossless,
    /// 16 bits over the block's range (about 1.5e-5 of the range): for
    /// confidences and weights the author accepts to round.
    Q16,
}

/// One joint of the rig's hierarchy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Joint {
    pub name: String,
    /// Index of the parent joint, `None` for a root.
    pub parent: Option<u16>,
}

/// One named channel of a take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelDesc {
    pub name: String,
    pub kind: ChannelKind,
    /// The joint it drives, when it is a joint channel.
    pub joint: Option<u16>,
    #[serde(default)]
    pub quantisation: Quantisation,
}

/// The schema of a take: its joints and channels, in column order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigProfile {
    pub name: String,
    pub joints: Vec<Joint>,
    pub channels: Vec<ChannelDesc>,
}

impl RigProfile {
    /// Total `f32` columns across every channel.
    pub fn columns(&self) -> usize {
        self.channels.iter().map(|c| c.kind.width()).sum()
    }

    /// The first column of each channel, in channel order.
    pub fn column_offsets(&self) -> Vec<usize> {
        let mut at = 0;
        self.channels
            .iter()
            .map(|c| {
                let start = at;
                at += c.kind.width();
                start
            })
            .collect()
    }

    /// Index of the channel called `name`.
    pub fn channel_index(&self, name: &str) -> Option<usize> {
        self.channels.iter().position(|c| c.name == name)
    }

    /// Checks the schema is self-consistent: parents precede children,
    /// channel joints exist, channel names are unique.
    pub fn validate(&self) -> Result<(), FormatError> {
        for (i, joint) in self.joints.iter().enumerate() {
            if joint.parent.is_some_and(|p| usize::from(p) >= i) {
                return Err(FormatError::InvalidRig(format!(
                    "joint {} has a parent that does not precede it",
                    joint.name
                )));
            }
        }
        for channel in &self.channels {
            if channel.joint.is_some_and(|j| usize::from(j) >= self.joints.len()) {
                return Err(FormatError::InvalidRig(format!(
                    "channel {} names a joint that does not exist",
                    channel.name
                )));
            }
        }
        let mut names: Vec<&str> = self.channels.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        match names.windows(2).find(|w| w[0] == w[1]) {
            Some(w) => Err(FormatError::InvalidRig(format!("channel {} appears twice", w[0]))),
            None => Ok(()),
        }
    }

    /// A rig of `joints` (name, parent) where every joint has a rotation
    /// channel and the first also a root position, plus named blendshapes.
    pub fn humanoid(name: &str, joints: &[(&str, Option<u16>)], blendshapes: &[&str]) -> Self {
        let joint_list: Vec<Joint> = joints
            .iter()
            .map(|(n, p)| Joint { name: (*n).to_owned(), parent: *p })
            .collect();
        let mut channels = Vec::new();
        if let Some(root) = joint_list.first() {
            channels.push(ChannelDesc {
                name: format!("{}.position", root.name),
                kind: ChannelKind::Position3,
                joint: Some(0),
                quantisation: Quantisation::Lossless,
            });
        }
        for (i, joint) in joint_list.iter().enumerate() {
            channels.push(ChannelDesc {
                name: format!("{}.rotation", joint.name),
                kind: ChannelKind::Rotation4,
                joint: u16::try_from(i).ok(),
                quantisation: Quantisation::Lossless,
            });
        }
        for shape in blendshapes {
            channels.push(ChannelDesc {
                name: format!("blend.{shape}"),
                kind: ChannelKind::Scalar,
                joint: None,
                quantisation: Quantisation::Q16,
            });
        }
        Self { name: name.to_owned(), joints: joint_list, channels }
    }
}
