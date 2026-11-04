//! The skeleton, skin weights and animations of a rigged source.
//!
//! [`extract_rigs`] reads what the static conversion ignores (the scene's
//! `Skin`, each vertex's `skin_weights`, the node animations) into plain
//! arrays, so a host can build its own skeletal assets without depending on
//! SolidRS or on this crate's math types. Nothing here knows an asset format.
//!
//! Bones are the skin's joint nodes plus every ancestor of a joint, so each
//! bone's world transform is the joint's world transform in the source and the
//! inverse bind matrices stay valid.

use std::collections::{HashMap, HashSet};

use solid_rs::scene::{AnimationTarget, Interpolation, NodeId};
use solid_rs::Scene;

/// A rigged model: one skeleton, the mesh skinned to it, and its animations.
#[derive(Clone, Debug)]
pub struct ImportedRig {
    /// Index in the source skin table; independent of names and node traversal order.
    pub source_skin: usize,
    pub name: String,
    /// Parents come before their children.
    pub bones: Vec<ImportedBone>,
    /// The skin's joint table: joint `n` follows `bones[joints[n].bone]`. The
    /// joint indices in [`RigVertex`] index this list.
    pub joints: Vec<ImportedJoint>,
    /// Geometry grouped into source LODs. Mesh names containing a `LOD<n>`
    /// token (for example `Body_LOD1`) are grouped together; untagged meshes
    /// belong to LOD 0. LODs are sorted by the numeric suffix.
    pub lods: Vec<ImportedLod>,
    /// Names of the scene's materials; a section's `material_index` indexes this.
    pub material_names: Vec<String>,
    pub animations: Vec<ImportedClip>,
}

#[derive(Clone, Debug)]
pub struct ImportedLod {
    pub vertices: Vec<RigVertex>,
    /// One section per source material, in order of first use.
    pub sections: Vec<RigSection>,
}

#[derive(Clone, Debug)]
pub struct ImportedBone {
    pub name: String,
    pub parent: Option<usize>,
    /// Local bind transform relative to the parent.
    pub translation: [f32; 3],
    /// Unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct ImportedJoint {
    pub bone: usize,
    /// Column-major inverse bind matrix.
    pub inverse_bind: [f32; 16],
}

#[derive(Clone, Copy, Debug)]
pub struct RigVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Indices into [`ImportedRig::joints`].
    pub joints: [u16; 4],
    /// Normalized: the weights sum to 1.
    pub weights: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct RigSection {
    /// Triangle list, absolute into [`ImportedLod::vertices`].
    pub indices: Vec<u32>,
    pub material_index: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ImportedClip {
    pub name: String,
    /// Length in seconds.
    pub duration: f32,
    /// Only bones the clip animates.
    pub tracks: Vec<ImportedTrack>,
}

#[derive(Clone, Debug)]
pub struct ImportedTrack {
    pub bone: usize,
    /// Every key carries a full transform; channels the source does not
    /// animate hold the bone's bind value.
    pub keys: Vec<ImportedKey>,
}

#[derive(Clone, Copy, Debug)]
pub struct ImportedKey {
    pub time: f32,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Per-channel interpolation and derivatives survive import into `.animseq`.
    pub translation_interpolation: ImportedInterpolation,
    pub rotation_interpolation: ImportedInterpolation,
    pub scale_interpolation: ImportedInterpolation,
    pub translation_in_tangent: [f32; 3],
    pub translation_out_tangent: [f32; 3],
    pub rotation_in_tangent: [f32; 4],
    pub rotation_out_tangent: [f32; 4],
    pub scale_in_tangent: [f32; 3],
    pub scale_out_tangent: [f32; 3],
    /// glTF source quaternion value, needed because the editor transform uses Euler angles.
    pub rotation_quaternion: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportedInterpolation {
    #[default]
    Linear,
    Step,
    CubicSpline,
}

/// One rig per skin used by a mesh node, in order of first use.
/// Geometry can be shared by nodes with the same mesh and skin, but never by
/// different skin palettes or inverse bind tables.
pub fn extract_rigs(scene: &Scene, config: &crate::LoadConfig) -> crate::Result<Vec<ImportedRig>> {
    let mut skins = Vec::new();
    for node in &scene.nodes {
        if let (Some(mesh), Some(skin)) = (node.mesh, node.skin) {
            if mesh >= scene.meshes.len() || skin >= scene.skins.len() {
                return Err(crate::AssetError::InvalidData(format!(
                    "mesh node '{}' has an invalid mesh or skin",
                    node.name
                )));
            }
            if !skins.contains(&skin) {
                skins.push(skin);
            }
        }
    }
    skins
        .into_iter()
        .map(|skin| extract_skin_rig(scene, config, skin))
        .collect()
}

fn extract_skin_rig(
    scene: &Scene,
    config: &crate::LoadConfig,
    skin_index: usize,
) -> crate::Result<ImportedRig> {
    let skin = &scene.skins[skin_index];
    if skin.joints.is_empty() {
        return Err(crate::AssetError::InvalidData(format!(
            "skin {skin_index} has an invalid joint count: {}",
            skin.joints.len()
        )));
    }
    let mut mesh_indices = Vec::new();
    for node in &scene.nodes {
        if node.skin == Some(skin_index) {
            if let Some(mesh) = node.mesh {
                if !mesh_indices.contains(&mesh) {
                    mesh_indices.push(mesh);
                }
            }
        }
    }

    // Unit scale is applied to positions and translations (see LoadConfig).
    let scale = config.import_scale.x;

    // ── Bones ──────────────────────────────────────────────────────────────
    let node_by_id: HashMap<NodeId, &solid_rs::scene::Node> =
        scene.nodes.iter().map(|n| (n.id, n)).collect();
    let mut parent_of: HashMap<NodeId, NodeId> = HashMap::new();
    for node in &scene.nodes {
        for child in &node.children {
            parent_of.insert(*child, node.id);
        }
    }
    let mut in_rig: HashSet<NodeId> = HashSet::new();
    for joint in &skin.joints {
        let mut current = Some(*joint);
        while let Some(id) = current {
            if !in_rig.insert(id) {
                break;
            }
            current = parent_of.get(&id).copied();
        }
    }

    let mut roots: Vec<NodeId> = in_rig
        .iter()
        .copied()
        .filter(|id| parent_of.get(id).is_none_or(|p| !in_rig.contains(p)))
        .collect();
    roots.sort();
    let mut bones: Vec<ImportedBone> = Vec::with_capacity(in_rig.len());
    let mut bone_of: HashMap<NodeId, usize> = HashMap::new();
    let mut stack: Vec<(NodeId, Option<usize>)> =
        roots.into_iter().rev().map(|id| (id, None)).collect();
    while let Some((id, parent)) = stack.pop() {
        let Some(node) = node_by_id.get(&id) else {
            continue;
        };
        let t = &node.transform;
        let index = bones.len();
        if bone_of.insert(id, index).is_some() {
            return Err(crate::AssetError::InvalidData(format!(
                "cycle or repeated node in skin {skin_index}"
            )));
        }
        bones.push(ImportedBone {
            name: node.name.clone(),
            parent,
            translation: [
                t.translation.x * scale,
                t.translation.y * scale,
                t.translation.z * scale,
            ],
            rotation: [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
            scale: [t.scale.x, t.scale.y, t.scale.z],
        });
        for child in node.children.iter().rev().filter(|c| in_rig.contains(c)) {
            stack.push((*child, Some(index)));
        }
    }

    if skin.joints.iter().any(|joint| !bone_of.contains_key(joint)) {
        return Err(crate::AssetError::InvalidData(format!(
            "skin {skin_index} references missing or cyclic joint nodes"
        )));
    }

    // ── Joints ─────────────────────────────────────────────────────────────
    let joints: Vec<ImportedJoint> = skin
        .joints
        .iter()
        .enumerate()
        .map(|(n, id)| {
            let mut m = skin.inverse_bind_matrix(n).to_cols_array();
            // S * IBM * S^-1 for a uniform scale S only changes the translation.
            for t in &mut m[12..15] {
                *t *= scale;
            }
            ImportedJoint {
                bone: bone_of[id],
                inverse_bind: m,
            }
        })
        .collect();

    let lods = extract_geometry(scene, config, mesh_indices, skin.joints.len())?;
    Ok(ImportedRig {
        source_skin: skin_index,
        name: skin.name.clone(),
        animations: extract_animations(scene, &bones, &bone_of, scale),
        bones,
        joints,
        lods,
        material_names: scene.materials.iter().map(|m| m.name.clone()).collect(),
    })
}

fn extract_geometry(
    scene: &Scene,
    config: &crate::LoadConfig,
    mesh_indices: Vec<usize>,
    joint_count: usize,
) -> crate::Result<Vec<ImportedLod>> {
    let scale = config.import_scale.x;
    let mut mesh_lods: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for mesh_index in mesh_indices {
        let name = &scene.meshes[mesh_index].name;
        mesh_lods
            .entry(lod_index(name).unwrap_or(0))
            .or_default()
            .push(mesh_index);
    }
    let mut lods = Vec::with_capacity(mesh_lods.len());
    let mut unweighted = 0usize;
    for (_, mesh_indices) in mesh_lods {
        let mut vertices: Vec<RigVertex> = Vec::new();
        let mut sections: Vec<RigSection> = Vec::new();
        for mesh_index in mesh_indices {
            let mesh = &scene.meshes[mesh_index];
            let base = u32::try_from(vertices.len())
                .map_err(|_| crate::AssetError::InvalidData("too many skinned vertices".into()))?;
            let count = u32::try_from(mesh.vertices.len())
                .map_err(|_| crate::AssetError::InvalidData("too many skinned vertices".into()))?;
            base.checked_add(count).ok_or_else(|| {
                crate::AssetError::InvalidData("too many skinned vertices".into())
            })?;
            for v in &mesh.vertices {
                let normal = v.normal.map_or([0.0, 1.0, 0.0], |n| [n.x, n.y, n.z]);
                let uv = v.uvs[0].map_or([0.0, 0.0], |uv| {
                    if config.flip_uv_y {
                        [uv.x, 1.0 - uv.y]
                    } else {
                        [uv.x, uv.y]
                    }
                });
                let (vertex_joints, weights) = v
                    .skin_weights
                    .as_ref()
                    .and_then(|sw| clean_weights(sw.joints, sw.weights, joint_count))
                    .unwrap_or_else(|| {
                        unweighted += 1;
                        ([0; 4], [1.0, 0.0, 0.0, 0.0])
                    });
                vertices.push(RigVertex {
                    position: [
                        v.position.x * scale,
                        v.position.y * scale,
                        v.position.z * scale,
                    ],
                    normal,
                    uv,
                    joints: vertex_joints,
                    weights,
                });
            }
            for primitive in &mesh.primitives {
                if primitive.topology != solid_rs::geometry::Topology::TriangleList {
                    log::warn!(
                        "Skinned mesh '{}' has a non-triangle primitive; skipped",
                        mesh.name
                    );
                    continue;
                }
                if primitive.indices.iter().any(|&index| index >= count) {
                    return Err(crate::AssetError::InvalidData(format!(
                        "mesh '{}' has an invalid vertex index",
                        mesh.name
                    )));
                }
                let indices = primitive.indices.iter().map(|i| base + i);
                match sections
                    .iter_mut()
                    .find(|s| s.material_index == primitive.material_index)
                {
                    Some(section) => section.indices.extend(indices),
                    None => sections.push(RigSection {
                        indices: indices.collect(),
                        material_index: primitive.material_index,
                    }),
                }
            }
        }
        lods.push(ImportedLod { vertices, sections });
    }
    if unweighted > 0 {
        log::warn!("{unweighted} skinned vertices had no usable weights and were bound to joint 0");
    }
    if lods.iter().all(|lod| lod.vertices.is_empty()) {
        return Err(crate::AssetError::InvalidData(
            "skin has no mesh vertices".into(),
        ));
    }

    Ok(lods)
}

/// Common DCC naming convention, case-insensitive. The first numeric `LOD`
/// token wins; a mesh without one is part of the highest-detail LOD.
fn lod_index(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if !bytes[i..i + 3].eq_ignore_ascii_case(b"lod") {
            continue;
        }
        let mut start = i + 3;
        while bytes
            .get(start)
            .is_some_and(|b| matches!(b, b'_' | b'-' | b' '))
        {
            start += 1;
        }
        let end = (start..bytes.len())
            .find(|n| !bytes[*n].is_ascii_digit())
            .unwrap_or(bytes.len());
        if end > start {
            if let Ok(index) = name[start..end].parse() {
                return Some(index);
            }
        }
    }
    None
}

/// Weights made safe: invalid or out-of-range influences dropped, the rest
/// normalized. `None` when nothing usable is left.
fn clean_weights(
    joints: [u16; 4],
    weights: [f32; 4],
    joint_count: usize,
) -> Option<([u16; 4], [f32; 4])> {
    let mut out_joints = [0u16; 4];
    let mut out_weights = [0.0f32; 4];
    for k in 0..4 {
        if weights[k].is_finite() && weights[k] > 0.0 && (joints[k] as usize) < joint_count {
            out_joints[k] = joints[k];
            out_weights[k] = weights[k];
        }
    }
    let sum: f64 = out_weights.iter().map(|&weight| f64::from(weight)).sum();
    if sum <= 0.0 {
        return None;
    }
    for w in &mut out_weights {
        *w = (f64::from(*w) / sum) as f32;
    }
    Some((out_joints, out_weights))
}

// ── Animation ──────────────────────────────────────────────────────────────

/// One animated property of a node, ready to sample.
struct Curve<'a> {
    interpolation: Interpolation,
    times: &'a [f32],
    values: &'a [f32],
    /// Components per key (3 or 4).
    stride: usize,
}

impl Curve<'_> {
    /// Value stored for key `k`: for a cubic spline, the middle element of its
    /// `[in tangent, value, out tangent]` triple.
    fn key(&self, k: usize) -> &[f32] {
        let width = if self.interpolation == Interpolation::CubicSpline {
            self.stride * 3
        } else {
            self.stride
        };
        let offset = if self.interpolation == Interpolation::CubicSpline {
            self.stride
        } else {
            0
        };
        let start = k * width + offset;
        self.values.get(start..start + self.stride).unwrap_or(&[])
    }

    fn tangent(&self, k: usize, incoming: bool) -> &[f32] {
        if self.interpolation != Interpolation::CubicSpline {
            return &[];
        }
        let start = k * self.stride * 3 + if incoming { 0 } else { self.stride * 2 };
        self.values.get(start..start + self.stride).unwrap_or(&[])
    }

    fn mode(&self) -> ImportedInterpolation {
        match self.interpolation {
            Interpolation::Step => ImportedInterpolation::Step,
            Interpolation::CubicSpline => ImportedInterpolation::CubicSpline,
            _ => ImportedInterpolation::Linear,
        }
    }

    /// Value and first derivative at `time`. Derivatives use source time units,
    /// as required by glTF Hermite interpolation.
    fn sample_with_derivative(&self, time: f32) -> Option<(Vec<f32>, Vec<f32>)> {
        let n = self.times.len();
        if n == 0 || self.key(0).len() != self.stride {
            return None;
        }
        if n == 1 || time < self.times[0] {
            return Some((self.key(0).to_vec(), vec![0.0; self.stride]));
        }
        if time > self.times[n - 1] {
            return Some((self.key(n - 1).to_vec(), vec![0.0; self.stride]));
        }
        if time == self.times[0] && self.interpolation == Interpolation::CubicSpline {
            return Some((self.key(0).to_vec(), self.tangent(0, false).to_vec()));
        }
        if time == self.times[n - 1] && self.interpolation == Interpolation::CubicSpline {
            return Some((self.key(n - 1).to_vec(), self.tangent(n - 1, true).to_vec()));
        }
        // Linear and step curves at exactly the last key: no key lies after
        // it to interpolate towards.
        if time == self.times[n - 1] {
            return Some((self.key(n - 1).to_vec(), vec![0.0; self.stride]));
        }
        let next = self.times.partition_point(|t| *t <= time);
        let (a, b) = (next - 1, next);
        let span = (self.times[b] - self.times[a]).max(1e-9);
        let t = (time - self.times[a]) / span;
        let (ka, kb) = (self.key(a), self.key(b));
        if ka.len() != self.stride || kb.len() != self.stride {
            return None;
        }
        if self.interpolation == Interpolation::Step {
            return Some((ka.to_vec(), vec![0.0; self.stride]));
        }
        if self.interpolation == Interpolation::CubicSpline {
            let out = self.tangent(a, false);
            let incoming = self.tangent(b, true);
            if out.len() != self.stride || incoming.len() != self.stride {
                return None;
            }
            let t2 = t * t;
            let t3 = t2 * t;
            let mut value = Vec::with_capacity(self.stride);
            let mut derivative = Vec::with_capacity(self.stride);
            for i in 0..self.stride {
                value.push(
                    (2.0 * t3 - 3.0 * t2 + 1.0) * ka[i]
                        + (t3 - 2.0 * t2 + t) * span * out[i]
                        + (-2.0 * t3 + 3.0 * t2) * kb[i]
                        + (t3 - t2) * span * incoming[i],
                );
                derivative.push(
                    ((6.0 * t2 - 6.0 * t) * ka[i]
                        + (3.0 * t2 - 4.0 * t + 1.0) * span * out[i]
                        + (-6.0 * t2 + 6.0 * t) * kb[i]
                        + (3.0 * t2 - 2.0 * t) * span * incoming[i])
                        / span,
                );
            }
            if self.stride == 4 {
                let length = value.iter().map(|v| v * v).sum::<f32>().sqrt();
                if length > 1e-8 {
                    let unit: Vec<f32> = value.iter().map(|v| *v / length).collect();
                    let radial = unit
                        .iter()
                        .zip(&derivative)
                        .map(|(q, d)| q * d)
                        .sum::<f32>();
                    for i in 0..4 {
                        derivative[i] = (derivative[i] - unit[i] * radial) / length;
                        value[i] = unit[i];
                    }
                }
            }
            return Some((value, derivative));
        }
        if self.stride == 4 {
            let value = slerp(ka, kb, t).to_vec();
            // The derivatives are unused for linear channels.
            Some((value, vec![0.0; self.stride]))
        } else {
            Some((
                ka.iter().zip(kb).map(|(x, y)| x + (y - x) * t).collect(),
                ka.iter().zip(kb).map(|(x, y)| (y - x) / span).collect(),
            ))
        }
    }

    fn sample(&self, time: f32) -> Option<Vec<f32>> {
        self.sample_with_derivative(time).map(|(value, _)| value)
    }

    fn tangents_at(&self, time: f32) -> Option<(Vec<f32>, Vec<f32>)> {
        if self.interpolation != Interpolation::CubicSpline {
            return Some((vec![0.0; self.stride], vec![0.0; self.stride]));
        }
        if let Some(index) = self.times.iter().position(|key_time| *key_time == time) {
            let value = self.key(index);
            let mut incoming = self.tangent(index, true).to_vec();
            let mut outgoing = self.tangent(index, false).to_vec();
            if value.len() != self.stride
                || incoming.len() != self.stride
                || outgoing.len() != self.stride
            {
                return None;
            }
            if self.stride == 4 {
                normalize_quaternion_derivative(value, &mut incoming);
                normalize_quaternion_derivative(value, &mut outgoing);
            }
            return Some((incoming, outgoing));
        }
        self.sample_with_derivative(time)
            .map(|(_, derivative)| (derivative.clone(), derivative))
    }
}

fn normalize_quaternion_derivative(value: &[f32], derivative: &mut [f32]) {
    let length = value.iter().map(|v| v * v).sum::<f32>().sqrt();
    if length <= 1e-8 || value.len() != 4 || derivative.len() != 4 {
        return;
    }
    let unit = [
        value[0] / length,
        value[1] / length,
        value[2] / length,
        value[3] / length,
    ];
    let radial = unit
        .iter()
        .zip(derivative.iter())
        .map(|(q, d)| q * d)
        .sum::<f32>();
    for i in 0..4 {
        derivative[i] = (derivative[i] - unit[i] * radial) / length;
    }
}

/// Shortest-path spherical interpolation of two quaternions (`[x, y, z, w]`).
fn slerp(a: &[f32], b: &[f32], t: f32) -> [f32; 4] {
    let mut b = [b[0], b[1], b[2], b[3]];
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        dot = -dot;
        for v in &mut b {
            *v = -*v;
        }
    }
    let (wa, wb) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    let mut out = [
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
        a[3] * wa + b[3] * wb,
    ];
    let len = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len > 1e-8 {
        for v in &mut out {
            *v /= len;
        }
    }
    out
}

#[derive(Default)]
struct BoneCurves<'a> {
    translation: Option<Curve<'a>>,
    rotation: Option<Curve<'a>>,
    scale: Option<Curve<'a>>,
}

fn extract_animations(
    scene: &Scene,
    bones: &[ImportedBone],
    bone_of: &HashMap<NodeId, usize>,
    scale: f32,
) -> Vec<ImportedClip> {
    let mut clips = Vec::new();
    for (n, animation) in scene.animations.iter().enumerate() {
        let mut curves: HashMap<usize, BoneCurves> = HashMap::new();
        for channel in &animation.channels {
            let (node, slot, stride) = match &channel.target {
                AnimationTarget::Translation(id) => (id, 0, 3),
                AnimationTarget::Rotation(id) => (id, 1, 4),
                AnimationTarget::Scale(id) => (id, 2, 3),
                AnimationTarget::MorphWeight { .. } => continue,
            };
            let Some(&bone) = bone_of.get(node) else {
                continue;
            };
            let curve = Curve {
                interpolation: channel.interpolation,
                times: &channel.times,
                values: &channel.values,
                stride,
            };
            let entry = curves.entry(bone).or_default();
            match slot {
                0 => entry.translation = Some(curve),
                1 => entry.rotation = Some(curve),
                _ => entry.scale = Some(curve),
            }
        }
        if curves.is_empty() {
            continue;
        }

        let mut tracks = Vec::new();
        let mut bone_order: Vec<usize> = curves.keys().copied().collect();
        bone_order.sort_unstable();
        for bone in bone_order {
            let bone_curves = &curves[&bone];
            let bind = &bones[bone];
            // Key at every time any of the bone's channels has a key.
            let mut times: Vec<f32> = [
                &bone_curves.translation,
                &bone_curves.rotation,
                &bone_curves.scale,
            ]
            .into_iter()
            .flatten()
            .flat_map(|c| c.times.iter().copied())
            .collect();
            times.sort_by(f32::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

            let keys = times
                .into_iter()
                .map(|time| {
                    let pick = |curve: &Option<Curve>, fallback: &[f32]| -> Vec<f32> {
                        curve
                            .as_ref()
                            .and_then(|c| c.sample(time))
                            .unwrap_or_else(|| fallback.to_vec())
                    };
                    // The bind translation is already scaled; sampled ones are not.
                    let t = match bone_curves
                        .translation
                        .as_ref()
                        .and_then(|c| c.sample(time))
                    {
                        Some(t) => [t[0] * scale, t[1] * scale, t[2] * scale],
                        None => bind.translation,
                    };
                    let r = pick(&bone_curves.rotation, &bind.rotation);
                    let s = pick(&bone_curves.scale, &bind.scale);
                    let (translation_in, translation_out) = bone_curves
                        .translation
                        .as_ref()
                        .and_then(|c| c.tangents_at(time))
                        .unwrap_or((vec![0.0; 3], vec![0.0; 3]));
                    let rotation_value = bone_curves
                        .rotation
                        .as_ref()
                        .and_then(|c| c.sample(time))
                        .unwrap_or_else(|| bind.rotation.to_vec());
                    let (rotation_in, rotation_out) = bone_curves
                        .rotation
                        .as_ref()
                        .and_then(|c| c.tangents_at(time))
                        .unwrap_or((vec![0.0; 4], vec![0.0; 4]));
                    let (scale_in, scale_out) = bone_curves
                        .scale
                        .as_ref()
                        .and_then(|c| c.tangents_at(time))
                        .unwrap_or((vec![0.0; 3], vec![0.0; 3]));
                    ImportedKey {
                        time,
                        translation: t,
                        rotation: [r[0], r[1], r[2], r[3]],
                        scale: [s[0], s[1], s[2]],
                        translation_interpolation: bone_curves
                            .translation
                            .as_ref()
                            .map_or(ImportedInterpolation::Linear, |c| c.mode()),
                        rotation_interpolation: bone_curves
                            .rotation
                            .as_ref()
                            .map_or(ImportedInterpolation::Linear, |c| c.mode()),
                        scale_interpolation: bone_curves
                            .scale
                            .as_ref()
                            .map_or(ImportedInterpolation::Linear, |c| c.mode()),
                        translation_in_tangent: [
                            translation_in[0] * scale,
                            translation_in[1] * scale,
                            translation_in[2] * scale,
                        ],
                        translation_out_tangent: [
                            translation_out[0] * scale,
                            translation_out[1] * scale,
                            translation_out[2] * scale,
                        ],
                        rotation_in_tangent: [
                            rotation_in[0],
                            rotation_in[1],
                            rotation_in[2],
                            rotation_in[3],
                        ],
                        rotation_out_tangent: [
                            rotation_out[0],
                            rotation_out[1],
                            rotation_out[2],
                            rotation_out[3],
                        ],
                        scale_in_tangent: [scale_in[0], scale_in[1], scale_in[2]],
                        scale_out_tangent: [scale_out[0], scale_out[1], scale_out[2]],
                        rotation_quaternion: [
                            rotation_value[0],
                            rotation_value[1],
                            rotation_value[2],
                            rotation_value[3],
                        ],
                    }
                })
                .collect();
            tracks.push(ImportedTrack { bone, keys });
        }
        clips.push(ImportedClip {
            name: if animation.name.is_empty() {
                format!("Animation {}", n + 1)
            } else {
                animation.name.clone()
            },
            duration: animation.duration(),
            tracks,
        });
    }
    clips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_cleaned_and_normalized() {
        let (j, w) = clean_weights([1, 2, 9, 0], [2.0, 2.0, 5.0, f32::NAN], 3).unwrap();
        assert_eq!((j[0], j[1]), (1, 2));
        assert_eq!(
            w,
            [0.5, 0.5, 0.0, 0.0],
            "the out-of-range and NaN influences are dropped"
        );
        assert!(clean_weights([0; 4], [0.0; 4], 3).is_none());
        assert!(clean_weights([7, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], 3).is_none());
    }

    #[test]
    fn rotation_curves_interpolate_by_the_shortest_arc() {
        let q0 = [0.0, 0.0, 0.0, 1.0];
        let half = std::f32::consts::FRAC_PI_4; // a 90 degree turn about Z
        let q1 = [0.0, 0.0, half.sin(), half.cos()];
        let mid = slerp(&q0, &q1, 0.5);
        let quarter = std::f32::consts::FRAC_PI_8; // 45 degrees
        assert!((mid[2] - quarter.sin()).abs() < 1e-5 && (mid[3] - quarter.cos()).abs() < 1e-5);
        // The negated quaternion is the same rotation: still the short way.
        let neg = [-q1[0], -q1[1], -q1[2], -q1[3]];
        let mid2 = slerp(&q0, &neg, 0.5);
        assert!((mid2[2] - quarter.sin()).abs() < 1e-5);
    }

    #[test]
    fn curves_hold_outside_their_keys_and_step_holds_between() {
        let times = [0.0, 1.0];
        let values = [0.0, 0.0, 0.0, 2.0, 4.0, 6.0];
        let linear = Curve {
            interpolation: Interpolation::Linear,
            times: &times,
            values: &values,
            stride: 3,
        };
        assert_eq!(linear.sample(-1.0).unwrap(), vec![0.0; 3]);
        assert_eq!(linear.sample(0.5).unwrap(), vec![1.0, 2.0, 3.0]);
        assert_eq!(linear.sample(2.0).unwrap(), vec![2.0, 4.0, 6.0]);
        assert_eq!(
            linear.sample(1.0).unwrap(),
            vec![2.0, 4.0, 6.0],
            "exactly at the last key"
        );
        let step = Curve {
            interpolation: Interpolation::Step,
            ..linear
        };
        assert_eq!(step.sample(0.9).unwrap(), vec![0.0; 3]);
        assert_eq!(step.sample(1.0).unwrap(), vec![2.0, 4.0, 6.0]);
    }
}
