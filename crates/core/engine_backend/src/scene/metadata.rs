//! Component-instance storage types for the level editor.
//!
//! `EditorObjectId`/`ComponentInstance` are the only types still live here --
//! `SceneObjectMetadata`/`HelioActorHandle`/`HelioObjectId`/`HelioLightId`/
//! `HelioVirtualObjectId`/`HelioWaterVolumeId` and this module's own
//! `ObjectType`/`LightType`/`MeshType` (shadow duplicates of the real,
//! actually-used ones in `scene::mod`) were deleted as confirmed-dead code:
//! zero callers outside `SceneMetadataDb`'s own now-removed object/
//! hierarchy surface. See `scene::mod`'s module doc.

/// Editor-side unique identifier for scene objects.
///
/// This is separate from Helio's internal IDs to allow for folders and other
/// organizational constructs that don't exist in Helio.
///
/// Re-exported from [`pulsar_scene_format`] since Pulsar-Native#557: the id
/// is part of the level file's wire format, so it is declared alongside it.
pub use pulsar_scene_format::ObjectId as EditorObjectId;

/// Component instance attached to a scene object.
///
/// Uses the reflection system for property inspection and editing.
///
/// Re-exported from [`pulsar_scene_format`] since Pulsar-Native#557 -- this
/// type IS the level file's `components` section entry, so it belongs to the
/// canonical schema rather than being declared a second time here.
pub use pulsar_scene_format::ComponentInstance;
