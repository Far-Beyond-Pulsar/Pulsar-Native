//! Import of a rigged model into the skeletal asset set: one `.skeleton`, one
//! `.skmesh` skinned to it and one `.animseq` per animation, each with a fresh
//! id and linked to the others by those ids (never by path).
//!
//! Re-importing the same source keeps every id the earlier import gave, found
//! through [`Previous`], so assets in the project that refer to the skeleton or
//! mesh keep working. An output that has since been moved or renamed is found
//! again by its id and rewritten where it now is; one that was deleted is
//! recreated with the same id. Nothing an import did not write is touched, and
//! animations that left the source are kept, not deleted.
//!
//! The scan used to find a moved file is a plain walk of the project; it is the
//! kind of lookup the shared file index (Pulsar-Native#1322) will answer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
use helio_asset_compat::{ImportedClip, ImportedKey, ImportedRig};
use skeletal_assets::animation::{
    AnimationClip, BoneTrack, KeyInterpolation, Keyframe, KeyframeTangent,
};
use skeletal_assets::assets::{
    load_animseq_asset, load_skeleton_asset, save_animseq_asset, save_skeleton_asset, AnimSeqAsset,
    SkeletonAsset, ANIMSEQ_EXTENSION, SKELETON_EXTENSION,
};
use skeletal_assets::ids::{AnimSequenceId, SkeletalMeshId, SkeletonId};
use skeletal_assets::math::{Transform, Vec3};
use skeletal_assets::skeleton::{Bone, Skeleton};
use skeletal_assets::skmesh::{
    load_skmesh_asset, save_skmesh_asset, validate_mesh, Joint, MaterialSlot, MeshLod, MeshSection,
    SkeletalMeshAsset, SkinnedVertex, SKMESH_EXTENSION,
};

use crate::db::{relative_key, ImportOutput};
use crate::importer::{Imported, Previous};

/// [`ImportOutput::kind`] of the skeleton.
pub const KIND_SKELETON: &str = "skeleton";
/// [`ImportOutput::kind`] of the skeletal mesh.
pub const KIND_SKMESH: &str = "skmesh";
/// [`ImportOutput::kind`] of an animation sequence.
pub const KIND_ANIMSEQ: &str = "animseq";

/// Frame rate given to imported animations; the source keys are in seconds and
/// are kept exactly, the rate only sets the editor's timeline grid.
const IMPORT_FPS: f32 = 30.0;

/// Directories a search for a moved output skips.
const SKIPPED_DIRS: [&str; 5] = ["target", "node_modules", ".git", ".pulsar", ".cargo"];

/// Where one output goes, and the identity it carries over.
struct Placed {
    path: PathBuf,
    /// The id the earlier import gave it, if any.
    id: Option<String>,
    /// A file this import is replacing is already there.
    exists: bool,
}

/// Write the asset set for `rig` next to `asset_hint` (the `.mesh` path the
/// static importer would use: its directory and stem name the outputs).
pub fn import_rig(
    rig: &ImportedRig,
    asset_hint: &Path,
    previous: &Previous,
) -> Result<Imported, String> {
    let dir = asset_hint.parent().unwrap_or_else(|| Path::new("."));
    let stem = asset_hint
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rig".to_string());
    virtual_fs::create_dir_all(dir).map_err(|e| format!("create {}: {e:#}", dir.display()))?;

    let mut taken: HashSet<PathBuf> = HashSet::new();
    let skeleton_at = place(
        previous,
        KIND_SKELETON,
        None,
        dir,
        &stem,
        SKELETON_EXTENSION,
        &mut taken,
    );
    let mesh_at = place(
        previous,
        KIND_SKMESH,
        None,
        dir,
        &stem,
        SKMESH_EXTENSION,
        &mut taken,
    );

    // ── The assets, in memory ──────────────────────────────────────────────
    let bone_ids = bone_ids(rig);
    let mut skeleton = SkeletonAsset::new(Skeleton {
        bones: rig
            .bones
            .iter()
            .zip(&bone_ids)
            .map(|(bone, id)| Bone {
                id: id.clone(),
                name: if bone.name.trim().is_empty() {
                    id.clone()
                } else {
                    bone.name.clone()
                },
                parent: bone.parent.map(|p| bone_ids[p].clone()),
                bind_transform: Transform::from_trs(bone.translation, bone.rotation, bone.scale),
            })
            .collect(),
    });
    if let Some(id) = skeleton_at.id.as_deref().and_then(SkeletonId::parse) {
        skeleton.id = id;
    }

    let mut mesh = build_mesh(rig, &bone_ids, skeleton.id.clone());
    if let Some(id) = mesh_at.id.as_deref().and_then(SkeletalMeshId::parse) {
        mesh.id = id;
    }
    // A re-import keeps the materials that were assigned to slots by name.
    if mesh_at.exists {
        if let Ok(old) = load_skmesh_asset(&mesh_at.path) {
            for (index, lod) in mesh.lods.iter_mut().enumerate() {
                if let Some(old_lod) = old.lods.get(index) {
                    lod.screen_size_threshold = old_lod.screen_size_threshold;
                }
            }
            for slot in &mut mesh.material_slots {
                if let Some(old_slot) = old.material_slots.iter().find(|s| s.name == slot.name) {
                    slot.material = old_slot.material.clone();
                    slot.material_id = old_slot.material_id.clone();
                }
            }
        }
    }
    for issue in validate_mesh(&mesh, Some(&skeleton.skeleton())) {
        tracing::warn!("imported skeletal mesh: {}", issue.message);
    }

    // The editors draw the mesh with the skeleton and its animations unless
    // told otherwise; a re-import keeps a choice made since.
    if skeleton_at.exists {
        if let Ok(old) = load_skeleton_asset(&skeleton_at.path) {
            skeleton.preview_mesh = old.preview_mesh;
        }
    }
    skeleton.preview_mesh.get_or_insert_with(|| mesh.id.clone());

    // The skeleton goes first so a failure part-way leaves nothing pointing at
    // a file that was not written.
    save_skeleton_asset(&skeleton_at.path, &skeleton)
        .map_err(|e| format!("write {}: {e:#}", skeleton_at.path.display()))?;
    save_skmesh_asset(&mesh_at.path, &mesh)
        .map_err(|e| format!("write {}: {e:#}", mesh_at.path.display()))?;

    let root = previous.project_root;
    remember_asset_id(
        root,
        "skeleton",
        &skeleton.id.to_string(),
        &skeleton_at.path,
    );
    remember_asset_id(root, "skmesh", &mesh.id.to_string(), &mesh_at.path);
    let mut outputs = vec![
        ImportOutput {
            path: relative_key(root, &skeleton_at.path),
            kind: KIND_SKELETON.into(),
            id: Some(skeleton.id.to_string()),
            source_name: None,
        },
        ImportOutput {
            path: relative_key(root, &mesh_at.path),
            kind: KIND_SKMESH.into(),
            id: Some(mesh.id.to_string()),
            source_name: None,
        },
    ];

    // ── Animations ─────────────────────────────────────────────────────────
    let mut matched: HashSet<&str> = HashSet::new();
    for clip in &rig.animations {
        let base = format!("{stem}_{}", file_name_part(&clip.name));
        let at = place(
            previous,
            KIND_ANIMSEQ,
            Some(&clip.name),
            dir,
            &base,
            ANIMSEQ_EXTENSION,
            &mut taken,
        );
        let mut sequence =
            AnimSeqAsset::new(Some(skeleton.id.clone()), build_clip(clip, &bone_ids));
        let old_sequence = at
            .exists
            .then(|| load_animseq_asset(&at.path).ok())
            .flatten();
        if let Some(id) = at
            .id
            .as_deref()
            .and_then(AnimSequenceId::parse)
            .or_else(|| old_sequence.as_ref().map(|old| old.id.clone()))
        {
            sequence.id = id;
        }
        let old_preview = old_sequence
            .as_ref()
            .and_then(|old| old.preview_mesh.clone());
        sequence.preview_mesh = Some(old_preview.unwrap_or_else(|| mesh.id.clone()));
        save_animseq_asset(&at.path, &sequence)
            .map_err(|e| format!("write {}: {e:#}", at.path.display()))?;
        remember_asset_id(root, "animseq", &sequence.id.to_string(), &at.path);
        matched.insert(clip.name.as_str());
        outputs.push(ImportOutput {
            path: relative_key(root, &at.path),
            kind: KIND_ANIMSEQ.into(),
            id: Some(sequence.id.to_string()),
            source_name: Some(clip.name.clone()),
        });
    }
    // Animations the source no longer has stay on disk and in the record, so
    // they come back under the same file if the source gets them back.
    outputs.extend(
        previous
            .outputs
            .iter()
            .filter(|o| {
                o.kind == KIND_ANIMSEQ
                    && !o
                        .source_name
                        .as_deref()
                        .is_some_and(|n| matched.contains(n))
            })
            .cloned(),
    );

    Ok(Imported {
        asset: mesh_at.path,
        outputs,
    })
}

/// Unique, non-empty bone ids from the bones' names. The same source gives the
/// same ids every time, so tracks and joints keep finding their bones.
fn bone_ids(rig: &ImportedRig) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    rig.bones
        .iter()
        .enumerate()
        .map(|(i, bone)| {
            let base = match bone.name.trim() {
                "" => format!("bone_{i}"),
                name => name.to_string(),
            };
            let mut id = base.clone();
            let mut n = 2;
            while !used.insert(id.clone()) {
                id = format!("{base}_{n}");
                n += 1;
            }
            id
        })
        .collect()
}

fn build_mesh(rig: &ImportedRig, bone_ids: &[String], skeleton: SkeletonId) -> SkeletalMeshAsset {
    // One slot per material any LOD uses, in order of first use.
    let mut slot_materials: Vec<Option<usize>> = Vec::new();
    let lods = rig
        .lods
        .iter()
        .enumerate()
        .map(|(lod_index, source_lod)| {
            let mut indices = Vec::new();
            let mut sections = Vec::new();
            for section in &source_lod.sections {
                let slot = match slot_materials
                    .iter()
                    .position(|m| *m == section.material_index)
                {
                    Some(slot) => slot,
                    None => {
                        slot_materials.push(section.material_index);
                        slot_materials.len() - 1
                    }
                };
                sections.push(MeshSection {
                    first_index: indices.len() as u32,
                    index_count: section.indices.len() as u32,
                    material_slot: slot as u32,
                });
                indices.extend_from_slice(&section.indices);
            }
            MeshLod {
                screen_size_threshold: 1.0 / (lod_index as f32 + 1.0),
                vertices: source_lod
                    .vertices
                    .iter()
                    .map(|v| SkinnedVertex {
                        position: v.position,
                        normal: v.normal,
                        uv: v.uv,
                        joints: v.joints,
                        weights: v.weights,
                    })
                    .collect(),
                indices,
                sections,
            }
        })
        .collect();

    let mut mesh = SkeletalMeshAsset::new(Some(skeleton));
    mesh.joints = rig
        .joints
        .iter()
        .map(|joint| Joint {
            bone: bone_ids[joint.bone].clone(),
            inverse_bind: joint.inverse_bind,
        })
        .collect();
    mesh.material_slots = slot_materials
        .iter()
        .enumerate()
        .map(|(n, material)| MaterialSlot {
            name: material
                .and_then(|m| rig.material_names.get(m))
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
                .map_or_else(|| format!("Material {}", n + 1), str::to_string),
            material_id: None,
            material: None,
        })
        .collect();
    mesh.lods = lods;
    mesh
}

fn build_clip(clip: &ImportedClip, bone_ids: &[String]) -> AnimationClip {
    AnimationClip {
        name: clip.name.clone(),
        duration: clip.duration,
        fps: IMPORT_FPS,
        tracks: clip
            .tracks
            .iter()
            .map(|track| BoneTrack {
                bone_id: bone_ids[track.bone].clone(),
                keyframes: keyframes(&track.keys),
            })
            .collect(),
    }
}

/// The keys as poses. Euler angles jump by 360 degrees where the same
/// rotation has two spellings, which the editor would interpolate the long way
/// round, so each angle is moved to the spelling nearest the previous key's.
pub fn keyframes(keys: &[ImportedKey]) -> Vec<Keyframe> {
    let mut previous: Option<[f32; 3]> = None;
    keys.iter()
        .map(|key| {
            let mut transform = Transform::from_trs(key.translation, key.rotation, key.scale);
            let mut angles = [
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
            ];
            if let Some(before) = previous {
                for (angle, before) in angles.iter_mut().zip(before) {
                    while *angle - before > 180.0 {
                        *angle -= 360.0;
                    }
                    while *angle - before < -180.0 {
                        *angle += 360.0;
                    }
                }
            }
            previous = Some(angles);
            transform.rotation = Vec3::new(angles[0], angles[1], angles[2]);
            let interpolation = |mode| match mode {
                helio_asset_compat::ImportedInterpolation::Linear => KeyInterpolation::Linear,
                helio_asset_compat::ImportedInterpolation::Step => KeyInterpolation::Step,
                helio_asset_compat::ImportedInterpolation::CubicSpline => {
                    KeyInterpolation::CubicSpline
                }
            };
            Keyframe {
                time: key.time,
                transform,
                translation_interpolation: interpolation(key.translation_interpolation),
                rotation_interpolation: interpolation(key.rotation_interpolation),
                scale_interpolation: interpolation(key.scale_interpolation),
                in_tangent: KeyframeTangent {
                    translation: key.translation_in_tangent,
                    rotation: key.rotation_in_tangent,
                    scale: key.scale_in_tangent,
                },
                out_tangent: KeyframeTangent {
                    translation: key.translation_out_tangent,
                    rotation: key.rotation_out_tangent,
                    scale: key.scale_out_tangent,
                },
                rotation_quaternion: (key.rotation_interpolation
                    == helio_asset_compat::ImportedInterpolation::CubicSpline)
                    .then_some(key.rotation_quaternion),
            }
        })
        .collect()
}

/// `name` made safe to use inside a file name.
fn file_name_part(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    match cleaned.trim() {
        "" => "Animation".to_string(),
        trimmed => trimmed.to_string(),
    }
}

// ── Finding where an output goes ───────────────────────────────────────────

/// Decide the file for one output of kind `kind`. `source_name` pairs
/// animations with the earlier import's; the others have one file each.
fn place(
    previous: &Previous,
    kind: &str,
    source_name: Option<&str>,
    dir: &Path,
    base: &str,
    extension: &str,
    taken: &mut HashSet<PathBuf>,
) -> Placed {
    let earlier = previous
        .outputs
        .iter()
        .find(|o| o.kind == kind && o.source_name.as_deref() == source_name);
    let Some(earlier) = earlier else {
        let path = free_path(dir, base, extension, taken);
        return Placed {
            path,
            id: None,
            exists: false,
        };
    };

    let id = earlier.id.clone();
    let recorded = previous.project_root.join(&earlier.path);
    let moved_to = match &id {
        // Where it was, with the id it had?
        Some(id) if file_id(&recorded, extension).as_deref() != Some(id.as_str()) => {
            find_by_id(previous.project_root, extension, id)
        }
        // Kinds without an id are the file at the recorded path, if any.
        None if !recorded.is_file() => None,
        _ => Some(recorded.clone()),
    };
    match moved_to {
        Some(path) if !taken.contains(&path) => {
            taken.insert(path.clone());
            Placed {
                path,
                id,
                exists: true,
            }
        }
        // Gone: recreate it, with the same id, at the default place.
        _ => {
            let path = free_path(dir, base, extension, taken);
            Placed {
                path,
                id,
                exists: false,
            }
        }
    }
}

/// `dir/base.extension`, or `base 2`, `base 3`, ... if that name is in use
/// (on disk or by an earlier output of this import).
fn free_path(dir: &Path, base: &str, extension: &str, taken: &mut HashSet<PathBuf>) -> PathBuf {
    let path = (1..)
        .map(|n| {
            let name = if n == 1 {
                base.to_string()
            } else {
                format!("{base} {n}")
            };
            dir.join(format!("{name}.{extension}"))
        })
        .find(|p| !p.exists() && !taken.contains(p))
        .expect("an unbounded range always yields a free name");
    taken.insert(path.clone());
    path
}

/// The id stored in the skeleton or skeletal mesh file at `path`.
fn file_id(path: &Path, extension: &str) -> Option<String> {
    match extension {
        SKELETON_EXTENSION | ANIMSEQ_EXTENSION => {
            let bytes = virtual_fs::read_file(path).ok()?;
            let text = std::str::from_utf8(&bytes).ok()?;
            if extension == SKELETON_EXTENSION {
                SkeletonAsset::peek_id(text).map(|id| id.to_string())
            } else {
                AnimSeqAsset::peek_id(text).map(|id| id.to_string())
            }
        }
        _ => SkeletalMeshAsset::peek_id_at(path)
            .ok()
            .map(|id| id.to_string()),
    }
}

/// The project file with `extension` that carries `id`, wherever it was moved.
/// If several copies do, none is picked (the id is ambiguous).
fn find_by_id(root: &Path, extension: &str, id: &str) -> Option<PathBuf> {
    if let Ok(Some(index)) = engine_fs::open_project_file_index(root) {
        let kind = match extension {
            SKELETON_EXTENSION => "skeleton",
            ANIMSEQ_EXTENSION => "animseq",
            _ => "skmesh",
        };
        if let Ok(paths) = index.paths_for_asset_id_kind(kind, id) {
            let mut matching = paths.into_iter().filter(|path| {
                path.extension()
                    .is_some_and(|found| found.eq_ignore_ascii_case(extension))
            });
            if let Some(first) = matching.next() {
                return matching.next().is_none().then_some(first);
            }
        }
    }
    let entries = virtual_fs::manifest(root).ok()?;
    let mut found = entries
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter(|e| {
            let path = Path::new(&e.path);
            let skipped = path.components().any(|c| {
                let name = c.as_os_str().to_string_lossy();
                SKIPPED_DIRS.contains(&name.as_ref()) || name.starts_with('.')
            });
            !skipped
                && path
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case(extension))
        })
        .map(|e| root.join(e.path))
        .filter(|path| file_id(path, extension).as_deref() == Some(id));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn remember_asset_id(root: &Path, kind: &str, id: &str, path: &Path) {
    if let Ok(Some(index)) = engine_fs::open_project_file_index(root) {
        if let Err(error) = index
            .refresh_path(path)
            .and_then(|_| index.record_typed_asset_id(kind, id, path))
        {
            tracing::debug!("could not cache asset id for {}: {error:#}", path.display());
        }
    }
}
