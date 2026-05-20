# Refactor Plan: Remove `Component` Enum, Unify on Reflection System

## Context

There are currently two parallel component systems:

1. **System 1 — typed `Component` enum** (`Vec<Component>` on `SceneObjectData` / `SceneObjectSnapshot`):
   `Material`, `Script`, `Collider`, `RigidBody`, `Light` variants.
   Rendered by `ComponentFieldsSection`.

2. **System 2 — reflection-based `ComponentInstance`** (`SceneMetadataDb`):
   Uses `#[derive(EngineClass)]` structs registered in `REGISTRY` at link time.
   Rendered by `ObjectTypeFieldsSection`.
   The "Add Component" dialog already uses this system.
   Real structs: `LightComponent`, `MaterialOverride`, `RigidBodyComponent`, etc. in `pulsar_rendering` / `pulsar_physics`.

**Goal:** Delete System 1 entirely. Everything goes through System 2.

---

## Step 1 — Delete `Component` enum from `engine_backend/src/scene/mod.rs`

Remove:
- `pub enum Component { Material { .. }, Script { .. }, Collider { .. }, RigidBody { .. }, Light { .. } }`
- The full `impl Component` block (`get_field_metadata`, `get_field_f32`, `set_field_f32`, `get_field_bool`, `set_field_bool`, `get_field_string`, `set_field_string`, `get_field_color`, `set_field_color`, `as_light`, `variant_name`)
- `pub enum ColliderShape` (if only used by `Component::Collider`)
- `pub enum ComponentFieldMetadata` and all its variants
- The `pub components: Vec<Component>` field from `SceneObjectSnapshot`
- The `pub components: Vec<Component>` field from `SceneEntryMeta`
- The `components: snap.components.clone()` line in `SceneEntry::new`
- The `props: meta.props.clone()` / `components: meta.components.clone()` lines in `SceneEntry::snapshot()`
- `SceneDb::set_props` method (only served the old system)

Also remove from `pub use` in `scene/mod.rs`:
- `Component` (no longer exported)

---

## Step 2 — Remove `components` from `SceneObjectData` and fix snapshot conversion (`scene_database.rs`)

- Delete `pub components: Vec<Component>` from `SceneObjectData`
- Remove `Component` from the `pub use engine_backend::scene::{...}` line
- In `SceneObjectData::from_snapshot`: remove `components: snap.components`
- In `SceneObjectData::into_snapshot`: remove `components: self.components`
- Fix the `load_from_file` loop — remove the `has_light_comp` / `Component::Light` auto-inject block (replaced by Step 5)

---

## Step 3 — Fix all struct literals that include `components: vec![]`

These files have `SceneObjectData { ..., components: vec![], ... }` literals that must have the field removed:

- `ui-crates/ui_level_editor/src/level_editor/ui/hierarchy.rs:229`
- `ui-crates/ui_level_editor/src/level_editor/ui/add_object_dialog.rs:195`
- `ui-crates/ui_level_editor/src/level_editor/ui/panel.rs:605, 642`
- `ui-crates/ui_level_editor/src/level_editor/ui/field_bindings.rs:499`
- `ui-crates/ui_level_editor/src/ai_tools.rs:802, 895`
- `ui-crates/ui_level_editor/src/level_editor/ui/viewport/helio_viewport.rs:156`
- `ui-crates/ui_level_editor/src/level_editor/scene_database.rs:385` (add_folder)

Also remove `props: Default::default()` lines added alongside `components: vec![]` if `props` is also removed from the struct (see Step 4).

---

## Step 4 — Populate `props` from `SceneMetadataDb` in `SceneDatabase`

`SceneObjectSnapshot.props` stays as-is — it is how the renderer reads light properties in `gpu_light_from_snap`.

Add a private helper `fn merge_component_props(object_id: &str, snap: &mut SceneObjectSnapshot, metadata_db: &SceneMetadataDb)` that:
1. Calls `metadata_db.get_components(object_id)`
2. For each `ComponentInstance { class_name, data }`, if `data` is a JSON object, merges all its key-value pairs into `snap.props` (last writer wins on key collision)

Call this helper at the end of `SceneDatabase::get_object()` and inside `get_all_objects()` (after `from_snapshot`).

This means:
- `LightComponent { color, intensity, range, ... }` stored as `{"color": [r,g,b,a], "intensity": 7.0, "range": 100.0, ...}` → all keys merged into `snap.props`
- `gpu_light_from_snap` already reads `snap.props["color"]`, `snap.props["intensity"]`, `snap.props["range"]` — **no change needed in the renderer**

---

## Step 5 — Auto-populate default components in `load_from_file` and `add_object`

In `SceneDatabase::load_from_file`, after inserting each object, call a new helper `fn ensure_default_components(object_id: &str, object_type: ObjectType, metadata_db: &SceneMetadataDb)`:

```
ObjectType::Light(_) → ensure "LightComponent" is present with defaults:
    color: [1.0, 1.0, 1.0, 1.0], intensity: 7.0, range: 100.0

ObjectType::Mesh(_) → ensure "MaterialOverride" is present with defaults
    (use REGISTRY.create_instance("MaterialOverride") to generate default JSON)
```

Pattern for generating default JSON (same as `add_component_dialog.rs:86`):
```rust
if let Some(mut instance) = REGISTRY.create_instance(class_name) {
    let props = instance.get_properties();
    let mut map = serde_json::Map::new();
    for prop in &props {
        let v = (prop.getter)(instance.as_ref());
        map.insert(prop.name.to_string(), property_value_to_json(&v));
    }
    metadata_db.add_component(object_id, class_name.to_string(), Value::Object(map));
}
```

Also call `ensure_default_components` from `SceneDatabase::add_object` so newly created objects get their defaults immediately.

---

## Step 6 — Delete `ComponentFieldsSection` entirely

- Delete `ui-crates/ui_level_editor/src/level_editor/ui/component_fields_section.rs`
- Remove `pub mod component_fields_section;` from `ui-crates/ui_level_editor/src/level_editor/ui/mod.rs`
- Remove `pub use component_fields_section::...` / `use super::ComponentFieldsSection` from any file
- In `workspace_panels.rs`:
  - Remove `component_sections: Vec<Entity<ComponentFieldsSection>>` field
  - Remove the block that creates `ComponentFieldsSection` for each `obj.components`
  - Remove `&self.component_sections` from the `properties_panel.render(...)` call
- In `properties_panel.rs`:
  - Remove `component_sections: &Vec<Entity<super::ComponentFieldsSection>>` parameter
  - Remove the `for section in component_sections { flex = flex.child(...) }` loop

---

## Step 7 — Fix `material_section.rs`

Currently reads `Component::Material { color, .. }` from `obj.components`.

Rewrite to read from `SceneMetadataDb`:
1. Call `scene_db.get_components(&object_id)` (returns `Vec<ComponentInstance>`)
2. Find the one with `class_name == "MaterialOverride"`
3. Parse its `data` JSON to get `color`, `metallic`, `roughness`
4. Write changes back via `scene_db.update_component_property(&object_id, "MaterialOverride", "color", json_val)`

---

## Step 8 — Update `default.level`

The current `default.level` has `"components": [{"Light": {...}}]` which is the old enum format.

Clear all `components` arrays to `[]` — `load_from_file` will auto-populate correct `LightComponent` defaults via Step 5.

Run:
```python
import json
with open('assets/default.level') as f: d = json.load(f)
for o in d['objects']: o['components'] = []; o['props'] = {}
with open('assets/default.level', 'w') as f: json.dump(d, f, indent=2)
```

---

## Step 9 — Verify renderer still works

`gpu_light_from_snap` in `sync_scene` already reads from `snap.props`:
```rust
let p = |k: &str, d: f32| snap.props.get(k).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(d);
```

After Step 4, `snap.props` is populated from `LightComponent`'s JSON (`color`, `intensity`, `range`).
**No change needed in the renderer.**

---

## Files Changed Summary

| File | Change |
|------|--------|
| `crates/engine_backend/src/scene/mod.rs` | Delete `Component` enum, `ColliderShape`, `ComponentFieldMetadata`, remove fields from `SceneObjectSnapshot`/`SceneEntryMeta` |
| `ui-crates/ui_level_editor/src/level_editor/scene_database.rs` | Remove `components` from `SceneObjectData`, add `merge_component_props`, add `ensure_default_components`, fix `load_from_file` and `add_object` |
| `ui-crates/ui_level_editor/src/level_editor/ui/component_fields_section.rs` | **Delete entirely** |
| `ui-crates/ui_level_editor/src/level_editor/ui/mod.rs` | Remove `component_fields_section` module |
| `ui-crates/ui_level_editor/src/level_editor/workspace_panels.rs` | Remove `component_sections` field and all creation logic |
| `ui-crates/ui_level_editor/src/level_editor/ui/properties_panel.rs` | Remove `component_sections` parameter and render loop |
| `ui-crates/ui_level_editor/src/level_editor/ui/material_section.rs` | Read from `SceneMetadataDb` instead of `Component::Material` |
| 7 files with `components: vec![]` literals | Remove that field from struct literal |
| `assets/default.level` | Clear all `components`/`props` arrays (Step 8 script) |

## Non-goals

- Do NOT change `ObjectTypeFieldsSection` — it already works correctly with the reflection system
- Do NOT change `add_component_dialog.rs` — already correct
- Do NOT change the renderer — already reads `snap.props`
- Do NOT remove `props: HashMap<String, serde_json::Value>` from `SceneObjectSnapshot` — the renderer needs it
