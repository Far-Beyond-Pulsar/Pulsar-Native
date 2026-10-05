# Contract proposal: persistence, JSON boundaries and undo

Status: **proposed — review required**

## Proposed decision

JSON is not the live scene model and is not used for internal cloning, property mutation, command execution, class expansion or undo. Scene persistence uses a versioned reflected archive with stable schema IDs and typed field values. JSON remains an optional human-readable interchange adapter and may remain in external protocols where it is useful.

The archive is not a memory dump. It records schema identity/version, stable object/component IDs, attachment metadata, field names or stable field IDs, typed values, resource references, and opaque unknown payloads. Each registered schema provides an explicit archive codec/migration. Never persist Rust `TypeId`, vtable pointers, raw `Entity`, raw struct memory, padding, or renderer-specific buffer indices.

## Archive shape and compatibility

Proposed archive records are:

- Scene header: format ID/version, scene ID, engine/schema manifest and optional content hashes.
- Object records: stable ID, name, parent stable ID, transform/visibility and object-level reflected data.
- Component records: stable instance ID, owner object ID, schema ID/version, order, enabled state, optional slot/provenance ID and typed fields.
- Resource references: stable asset/content IDs with import settings where authored.
- Unknown records: original schema ID/version plus opaque original bytes, preserved across load/save without pretending they are live editable values.

Binary is the proposed primary archive encoding to avoid a JSON object model in the canonical in-memory and storage path. It must be deterministic, inspectable with engine tooling, length/version checked, bounded against malformed input, and migratable. JSON adapters can encode the same logical archive for debugging/import/export. The format choice should not couple serialization to GPU layout or Rust ABI.

Load performs boundary decode -> schema migration -> typed value construction -> validated SceneDB transaction. Save performs a typed snapshot -> boundary archive encoding. Known invalid values produce actionable load errors; they are not silently defaulted and reported as attached. Unknown values remain opaque. Legacy flat LightComponent data and other observed shape mismatches are converted by versioned boundary migrations.

## In-memory classes and overrides

Class definitions/prefabs are decoded from files once into typed reflected defaults. Placed instances hold a stable class reference and typed override records keyed by slot ID and field ID/path. Removed slots are explicit typed tombstones. Changing class defaults recomputes instance views from current defaults plus typed overrides; it does not serialize components, recursively merge JSON values, and rehydrate the world on each edit.

If dynamic override storage is required, use a reflected `PropertyValue` representation or registered field codec, with schema validation. Do not make `serde_json::Value` the internal universal value solely because some scripts or classes are dynamic.

## Undo/redo

Undo stores typed before/after values or inverse patches in memory. A transaction captures the affected object/component entities and stable relationships. Deletion retains an owned snapshot until history releases it; restore reconstructs live entities while preserving stable IDs and remaps ephemeral `Entity` handles in references. Large immutable resources use shared handles; voxel/terrain edits can keep compact domain journals.

History operations are atomic and restore authoritative SceneDB state. GPU reflection and subscribers respond to the resulting commit through normal hooks. History must not clear and repopulate the whole world or rely on JSON as a clone mechanism. Durable crash recovery, if required, uses the same versioned archive/journal boundaries, not an assumption that in-memory `Any` values are persistent.

## Boundary classification

Every remaining JSON use must be classified as one of:

1. File/import/export adapter.
2. External AI, plugin, scripting VM, process, or network protocol adapter.
3. Debug/test fixture or diagnostic representation.
4. Untyped opaque unknown payload retained losslessly.

Any JSON conversion between two in-process operations on a known live component is a violation. API reviews should reject additions of internal `to_json -> from_json` clone paths.

## REVIEW: decisions to confirm or edit

- Is a versioned binary reflected archive the preferred canonical scene file, with JSON interchange supported?
- What inspectability/debugging tooling should accompany the archive?
- Should class overrides identify fields by stable field IDs, names, or both with migration aliases?
- Is crash-safe editor undo required, or is typed in-memory history sufficient for the current product?
- How long must unknown plugin component payloads remain byte-for-byte compatible?
