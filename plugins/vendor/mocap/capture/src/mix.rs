//! Combining any number of pose and sensor sources into one stream and one rig.
//!
//! Every source keeps its own rig and bus. For a take they are merged: with one
//! source the rig is that source's (names unchanged); with several, every joint
//! and channel is namespaced `"<source id>/<name>"` so nothing collides (two
//! VMC performers, a suit plus an IMU, a camera estimator plus a glove).

use crate::bus::PoseBus;
use crate::frame::PoseFrame;
use crate::recorder::MAX_HOLD_S;
use mocap_format::{ChannelDesc, Joint, RigProfile};
use std::ops::Range;
use std::sync::Arc;

/// One source's contribution to a mix.
pub struct MixInput {
    pub id: String,
    pub rig: RigProfile,
    pub bus: Arc<PoseBus>,
}

/// Merges rigs; returns the merged rig and each input's column range in it.
pub fn merge_rigs(inputs: &[(String, &RigProfile)]) -> (RigProfile, Vec<Range<usize>>) {
    if let [(_, only)] = inputs {
        return ((*only).clone(), vec![0..only.columns()]);
    }
    let mut merged = RigProfile { name: "mix".to_owned(), ..RigProfile::default() };
    let mut ranges = Vec::with_capacity(inputs.len());
    for (id, rig) in inputs {
        let joint_base = merged.joints.len() as u16;
        let start = merged.columns();
        merged.joints.extend(rig.joints.iter().map(|j| Joint {
            name: format!("{id}/{}", j.name),
            parent: j.parent.map(|p| p + joint_base),
        }));
        merged.channels.extend(rig.channels.iter().map(|c| ChannelDesc {
            name: format!("{id}/{}", c.name),
            joint: c.joint.map(|j| j + joint_base),
            ..c.clone()
        }));
        ranges.push(start..merged.columns());
    }
    (merged, ranges)
}

struct Track {
    bus: Arc<PoseBus>,
    range: Range<usize>,
    latest: Option<Arc<PoseFrame>>,
}

/// Turns the buses of several sources into one merged pose stream.
///
/// A merged frame is emitted for every frame any source delivers, holding the
/// other sources' latest values; a source silent for longer than
/// [`MAX_HOLD_S`] contributes gaps, never stale data.
pub struct Mixer {
    tracks: Vec<Track>,
    width: usize,
    sequence: u64,
}

impl Mixer {
    /// A mixer over `inputs`, and the merged rig its frames are in.
    pub fn new(inputs: Vec<MixInput>) -> (Self, RigProfile) {
        let rigs: Vec<(String, &RigProfile)> = inputs.iter().map(|i| (i.id.clone(), &i.rig)).collect();
        let (rig, ranges) = merge_rigs(&rigs);
        let tracks = inputs
            .into_iter()
            .zip(ranges)
            .map(|(input, range)| Track { bus: input.bus, range, latest: None })
            .collect();
        (Self { tracks, width: rig.columns(), sequence: 0 }, rig)
    }

    /// Drains every source and appends the merged frames, oldest first.
    pub fn pump(&mut self, out: &mut Vec<PoseFrame>) {
        let mut arrivals: Vec<(usize, Arc<PoseFrame>)> = Vec::new();
        let mut taken = Vec::new();
        for (i, track) in self.tracks.iter().enumerate() {
            taken.clear();
            track.bus.drain(&mut taken);
            arrivals.extend(taken.drain(..).map(|f| (i, f)));
        }
        arrivals.sort_by(|a, b| a.1.time_s.total_cmp(&b.1.time_s));
        for (index, frame) in arrivals {
            let now = frame.time_s;
            self.tracks[index].latest = Some(frame);
            self.sequence += 1;
            let mut values = vec![f32::NAN; self.width];
            for track in &self.tracks {
                if let Some(latest) = track.latest.as_ref().filter(|f| now - f.time_s <= MAX_HOLD_S) {
                    values[track.range.clone()].copy_from_slice(&latest.values);
                }
            }
            out.push(PoseFrame { sequence: self.sequence, time_s: now, values });
        }
    }
}
