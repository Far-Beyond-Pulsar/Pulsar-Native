# Plugin API refactor: audit and plan

Tracking issue: Far-Beyond-Pulsar/Pulsar-Native#1608 (design source: Tristan's
2026-11-07 plan plus the coordinator's review; decisions recorded in the issue).
This file is the one plan. When it lands it **replaces**
`.agents/PLUGIN_API_V2.md` and `.agents/PLUGIN_API_V2_REMAINING.md` (see
"Documents to delete" at the end; they are not deleted by the audit change).

Status: Phase 0 (audit) is this document's sections 1 to 5. Sections 6 and 7 are
the plan. Every row below is based on code read on `main` at 4327973b2 (SceneDB
at the pinned rev `017252c`, read from the local cargo checkout). Anything not
read is marked **UNVERIFIED**.

Legend for "gate": **G** = reader goes through `pulsar_plugins::registered` (the
registration has an `origin` and implements `pulsar_plugins::Registration`);
**G\*** = gated by another mechanism (named in the row); **none** = raw
`inventory::iter`, so a disabled plugin's submission still counts.

## 0. Surprises (read these first)

1. **Every descriptor has `requires = []`** (all 26 `plugins/vendor/*/pulsar-plugin.toml`),
   yet the plugins' crates depend on each other (section 3). Disabling
   `pulsar.meshes` while `pulsar.lights` stays enabled is allowed by `resolve`,
   although `lights_component` and the meshes feature crates depend on each
   other's api crates. The descriptor `requires` is today documentation nobody
   checks against Cargo.
2. **Many plugin submissions are not gated.** `AssetComponentRegistration`,
   `ThumbnailRendererRegistration`, `LinkedScriptLanguage` (Blueprint plugin),
   every reflection and SceneDB registration (`EngineClassRegistration`,
   `ComponentMethodRegistration`, `UiPropertyEditorHint`,
   `GpuMirrorRegistration`, `HandleLedgerRegistration`, ...) have no `origin`.
   Only `WorldComponentRegistration` (and, by class name, its ticks, events and
   unfinished reports) and the asset loaders are gated among the derive-made
   ones.
3. **The derive and attribute macros submit most plugin registrations**, not
   hand-written `inventory::submit!`. `#[register_world_component]`,
   `#[derive(SceneStore)]`, `engine_class`, `component_methods` emit
   `EngineClassRegistration`, `WorldComponentRegistration`, `GpuMirror/Clear`,
   `HandleLedger`, `ComponentTick/Event` in the plugin's crate. "Delete the
   scattered `inventory::submit!` sites" therefore only removes the
   hand-written ones (about 170 textual sites in plugins, section 1); the macro
   output needs a design decision (open question Q1).
4. **Unknown origins count as enabled.** `pulsar_plugins::origin_enabled`
   returns `true` when the origin package is in no descriptor
   (`crates/core/pulsar_plugins/src/lib.rs:468`), and `plugin_of` returns `None`.
   A typo, a helper crate, or `plugins/trait_editor` / `crates/editor/plugin_typescript`
   (no descriptors) is silently "the engine's". The only guard is the textual
   test `a_plugin_registers_only_from_its_modules`
   (`plugins/installed/tests/installed_set.rs:205`), which checks files under
   `plugins/vendor` that mention `origin!()`.
5. **`FeatureRegistrar::consumes` is used by exactly one feature (`post`).**
   `ContractUse` (`helio-feature-api/src/feature.rs`) is collected by
   `FeatureSet::register_into`, but only `post/feature/src/feature.rs:21-22` and
   Helio's own test call `consumes`, and nothing in `crates/core` or
   `crates/editor` reads the returned list. Phase 3 "link Helio buffer contracts to
   their provider plugin" has no data to link today.
6. **An api crate carries statics and `inventory::submit!`.**
   `plugins/vendor/materials/api` (`helio-mats`) submits `HandleLedgerRegistration`,
   `GpuMirrorRegistration`, `GpuClearRegistration` (`src/instance.rs:295,364,371`)
   and holds `static ACTIVE_GRAPHS: OnceLock<RwLock<..>>`
   (`src/radiant/graph_registry.rs:60`). `lights/api/src/settings.rs:18` has
   `pub static SHADOWS: Group`. The planned "no statics in api crates" test
   fails on today's tree, so it starts with an allow list that only shrinks.
7. **Core still names plugin crates.** Cargo edges from core to plugin crates:
   `engine`, `level_migrate`, `pulsar_package`, `scene_inventory` -> `installed_plugins`;
   `pulsar_game` -> `input_plugin`; `pulsar_package` -> `blueprint_editor_plugin`;
   `scene_inventory` -> `physics_plugin`, `input_plugin`; `ui_common` and
   `pulsar_physics` -> `skeletal_assets` (a Plugin_SKM crate). This is #1605's
   scope, but the catalog layering (section 4) must not make it worse.
8. **Four plugin repos are outside the workspace** (`exclude` in the root
   `Cargo.toml`: `blueprint_editor`, `code_editor`, `matter_editor`,
   `table_editor`) but are linked through path dependencies from
   `plugins/installed`. The issue's migration of an editor-only plugin
   ("suggest `table_editor`, which also brings it into the workspace") means
   removing it from `exclude` and adding `workspace = true` dependencies.
9. **`decals` is a poor "object kind with editor" proof.** Its editor crate
   (`plugins/vendor/decals/editor/src/lib.rs`) is identity only and submits
   nothing; all 7 direct submissions are in `decals_component`. Prefer `post`
   (component + feature + editor with `CreateEntry` and `EnvironmentListing`)
   as the Phase 1 object-kind pilot; `decals` becomes the second migration.
10. **Four plugin submodules are pinned to a non-`main` ref** per
    `git submodule status`: `input_plugin`, `matter_editor`, `physics_plugin`,
    `ui_editor` (all at `origin/rewrite/p17-cleanup`). `just check-submodule-pins`
    will reject them on main unless listed in `.github/submodule-pin-exceptions.txt`
    (not read: **UNVERIFIED**). Phase 1 pilots must not touch them first.
11. **`std::fs` is more widespread than the issue says.** My count
    (`grep -rn "std::fs::"` over `crates/` and `plugins/` excluding `ui`, `renderer`,
    `external`, `third-party`) is 1333 lines in 275 files, against the issue's
    663 lines in 148. The scoping differs (mine includes tests and counts every
    textual match); the A5 worker must define the count it shrinks.
    Pulsar has no root `clippy.toml` (Helio has one).
12. **`pulsar_plugins` is already close to dependency-light** (deps: `inventory`,
    `serde`, `toml`, `tracing`). Keep it that way: the catalog cannot live there
    because every registry it must read sits above it (section 4).

## 1. Registration matrix

"Submitters" are the crates that submit, from `grep -rn "submit!"` over `crates/`
and `plugins/` (textual sites; macros add more). "Readers" are the files that
iterate. "Summary?" asks whether a stable id, label, category and origin are
available now without reading plugin-private data: **yes** (a field exists),
**derive** (computable from fields), **no** (needs a new field).

### 1.1 Plugin identity

| Type | Owner | Submitters | Readers | Gate | Summary? |
|---|---|---|---|---|---|
| `InstalledPlugin { descriptor }` | `pulsar_plugins` (lib.rs:148) | `installed!` macro, one per plugin's first runtime (or editor) module | `pulsar_plugins::installed()` (raw iter, lib.rs:215) | n/a (it is the gate's source) | yes (descriptor id, name, description) |

### 1.2 Plugin-gated registrations (implement `Registration`, read via `registered`)

Submitting plugins from the textual counts; `lights` etc. are plugin dirs under
`plugins/vendor`. The origin is the submitting crate's `CARGO_PKG_NAME`.

| Type | Owner crate | Submitters (count) | Readers | Summary? |
|---|---|---|---|---|
| `RenderFeatureRegistration { features, configure }` | `engine_backend` (`.../helio_renderer/render_features.rs:10`) | decals, fog, foliage, gi, lens, lights, materials, meshes, particles, portals, post, sky, sprites, voxel, water (15, one each) | `render_features.rs:32,40` (`plugin_features`, `configure`) | derive (feature ids from `FeatureSet::ids`, no label or category) |
| `ViewOverscanRegistration` | `engine_backend` (same file:49) | lens | `render_features.rs:68` | no |
| `EditorLinesRegistration { name }` | `engine_backend` (same file:83) | splines | `render_features.rs:101` | yes (`name`) |
| `SceneRowsRegistration { register_columns, derivation }` | `engine_backend/src/scene/helio_bridge.rs:185` | decals, fog, foliage, lens, particles, post, sky, sprites, water (9) | `helio_bridge.rs:201,272` | no (no name) |
| `ObjectJoinRegistration { name }` | `helio_bridge.rs:83` | lights, meshes | `helio_bridge.rs:119,253` | yes (`name`) |
| `SpaceProducerRegistration { name }` | `helio_bridge.rs:460` | portals | `helio_bridge.rs:507` | yes (`name`) |
| `ContentGeometryRegistration` | `engine_backend/.../interaction.rs:786` | lights, meshes (plus one engine-owned) | `interaction.rs:775,820,1025` | no |
| `ViewIconRegistration` | `interaction.rs:974` | lights | `interaction.rs:985` | no |
| `RenderHostPartRegistration` | `engine_backend/.../host_parts.rs:74` | meshes | `host_parts.rs:85` | no |
| `AnimatedContentRegistration` | `engine_backend/.../animated_content.rs:30` | foliage x2, meshes x2, particles, water | `animated_content.rs:64,97` | no |
| `LevelSingletonRegistration { class_name }` | `engine_backend/src/scene/level_rules.rs:53` | foliage, sky | `level_rules.rs:62` | yes (`class_name`) |
| `ComponentEventsRegistration` | `engine_backend/src/scene/component_events.rs:38` | skm_editor (animation) | `component_events.rs:53` | no |
| `TickPhaseRegistration` | `engine_backend/src/scene/game_hooks.rs:35` | skm_editor | `game_hooks.rs:45` | no |
| `ContentReportRegistration` | `game_hooks.rs:59` | meshes | `game_hooks.rs:69` | no |
| `ImporterRegistration` | `asset_import/src/importer.rs:97` | meshes (plus engine-owned in `asset_import`, `ui_file_manager`) | `importer.rs:106` | derive (importer id) |
| `ProjectSettingsRegistration` | `pulsar_settings/src/project/mod.rs:16` | meshes | `project/mod.rs:28` | no |
| `SettingsGroupRegistration` | `pulsar_settings/src/project/rendering.rs:26` | foliage, gi, lights, portals, voxel (5) | `rendering.rs:40` | derive (group key) |
| `RecordMigrationRegistration` | `pulsar_class/src/records.rs:58` | meshes x2, skm_editor | `records.rs:98,122,141,225` | no |
| `ScenePayloadRegistration` | `pulsar_world_registry/src/scene_payloads.rs:36` | voxel | `scene_payloads.rs:58,69` | no |
| `SceneSubsystemRegistration { name }` | `pulsar_world_registry/src/scene_subsystems.rs:13` | meshes, voxel | `scene_subsystems.rs:32` | yes (`name`) |
| `AssetLoaderRegistration { kind }` | `helio_component/src/asset_component.rs:46` | materials | `asset_component.rs:93`; installed with a per-load `origin_enabled` check (:60-66) | yes (`kind`) |
| `AssetReloadRegistration` | `asset_component.rs:112` | helio_component (engine), materials x2, meshes | `asset_component.rs:123` | no |
| `GroundMaterialNames` | `helio_component/src/components/ground_materials.rs:26` | voxel | `ground_materials.rs:36,41` | no |
| `MovabilityRegistration` | `helio_component/src/components/movability.rs:68` | helio_component, lights, meshes | `movability.rs:90` | no |
| `SkinnedMeshAccess` / `SkeletalPoseAccess` | `helio_component/src/skinning.rs:49,91` | meshes / skm_editor | `skinning.rs:59,101` (`.next()`: first wins) | no |
| `TextureBindingRegistration` | `helio_component/src/texture_assets.rs:265` | decals, sprites | `texture_assets.rs:288` | no |
| `PluginEditorProvider { create }` | `plugin_manager/src/builtin.rs:137` | asset_viewer, blueprint_editor, code_editor, input_plugin, materials, matter_editor, physics_plugin, render_bps, skm_editor, table_editor, ui_editor (11) | `builtin.rs:172` (`BuiltinEditorRegistry::register_linked`) | yes (`provider_id()` on the created provider) |
| Level-editor extension points (13 types via `extension_point!`): `ObjectIconRegistration`, `ToolModeRegistration`, `CreateEntry`, `EnvironmentPreset`, `EnvironmentListing`, `LevelComponentSection`, `SaveHook`, `EditorData`, `Interaction`, `SelectionFollowUp`, `PropertyFollowUp`, `CameraFraming`, `AiToolNamespace`; plus `DebugOverlay` (`extend/viewport.rs:53`) | `ui_level_editor/src/extend/mod.rs:465` | `CreateEntry` x21 (3 engine-owned in `ui_level_editor`, the rest in fog, foliage, lights x7, meshes x4, post, sky, sprites, water), `EnvironmentListing` x6, `EnvironmentPreset` x3 (lights), `ToolModeRegistration` x4 (foliage, splines, voxel x2), `ObjectIconRegistration` (lights, meshes), `LevelComponentSection` (foliage, sky), `SaveHook`/`Interaction`/`PropertyFollowUp`/`CameraFraming` (voxel), `SelectionFollowUp`/`AiToolNamespace` (splines), `EditorData` (foliage), `DebugOverlay` (portals) | `extend/mod.rs` (`sorted`, `create_entry`, `environment_preset`, ...), `extend/viewport.rs:57` | `CreateEntry`: yes (`id`, labels); the rest: partial (`order`) |

### 1.3 Gated by another mechanism (G\*)

| Type | Owner | Submitters | Readers | How gated |
|---|---|---|---|---|
| `WorldComponentRegistration { class_name, origin, ... }` | `pulsar_world_registry/src/lib.rs:439` | macro `#[register_world_component]` / `engine_class` in every component crate; engine-owned in `engine_backend`, `pulsar_game`, `pulsar_script_object_model` | `pulsar_world_registry/src/runtime.rs:106-110` (OWN list, `Gated`, `origin_enabled(r.origin)`) | origin filter inside the runtime table. Summary: yes (`class_name`, category from `EngineClassRegistration`) |
| `ComponentTickRegistration`, `ComponentEventRegistration`, `UnfinishedComponentRegistration` | `pulsar_world_registry/src/lib.rs:103,112`, `unfinished.rs:21` | macros; input_plugin, physics_plugin | `runtime.rs:111-125`, gated by `class_enabled(class_name)` (all registrations of the class must be enabled) | by class name through the world registration's origin. Summary: derive (class name) |

### 1.4 Reflection and SceneDB registrations (macro-submitted; no origin; ungated)

These are registered per type by derives. They cannot name the submitting plugin
today. Readers are the shared registries (reflection, SceneDB), which are also
copy-attached across DLLs (`runtime.rs` modules, `PLUGIN_SYSTEM.md`).

| Type | Owner | Submitters (textual) | Readers | Summary? |
|---|---|---|---|---|
| `EngineClassRegistration { name, category }` | `pulsar_reflection/src/registry.rs:26` | macro, every `EngineClass` | `registry.rs`, `runtime.rs:105` | yes (name, category) but no origin |
| `ComponentMethodRegistration { class_name }` | `registry.rs:37` | decals, lens, splines + macro, `pulsar_game`, `pulsar_world_registry`, `pulsar_script_object_model` | `engine_class_derive/src/lib.rs:437` (raw iter inside generated code) | derive |
| `RuntimeTypeRegistration` | `pulsar_reflection/src/runtime_registry.rs:25` | 14 (reflection x6, `pulsar_script_object_model` x5, `pulsar_world_registry`, `ui_level_editor`, voxel) | `runtime_registry.rs` | derive |
| `TypeMethodRegistration`, `DynMethodRegistration`, `ConversionRegistration`, `EnumVariantDocs`, `ScenePropsApplierRegistration`, `TypeRendererRegistration` | `pulsar_reflection` (`methods.rs:276`, `dyn_registry.rs:65`, `conversions.rs:28`, `lib.rs:303,353`, `type_renderer.rs:101`) | reflection and derives | `runtime.rs` `Registrations` (attach list) | no |
| `UiPropertyEditorHint` | `pulsar_reflection/src/lib.rs:291` | `ui_common` x3, `helio_component`, voxel x2 | `helio_component/src/components/reflected_list.rs:25` (raw), `ui_common` | no |
| `GpuMirrorRegistration`, `GpuClearRegistration`, `VarLenReleaseRegistration`, `HandleLedgerRegistration`, `WorldMethodRegistration` | **SceneDB** (git dep, rev `017252c`: `gpu/world_mirror.rs:706,868,891`, `handle_ledger.rs:193`, `component_methods.rs:65`) | 19 each of Mirror/Clear (derive and hand-written in decals, fog, foliage, lens, lights, materials, meshes x2, particles, post, sky, sprites, water, `pulsar_scene_model` x4, `pulsar_physics`); `HandleLedger` x4 (decals, materials, skm_editor, sprites) | SceneDB `world_mirror.rs:727-771`, `handle_ledger.rs:200` | no (key is a component id) |

### 1.5 Ungated plugin submissions into engine-defined types (must be gated)

| Type | Owner | Plugin submitters | Reader | Why it matters |
|---|---|---|---|---|
| `AssetComponentRegistration` | `helio_component/src/asset_component.rs:20` | meshes | `asset_component.rs:24` (raw) | disabled meshes still maps asset kinds to components |
| `ThumbnailRendererRegistration { extension }` | `ui_common/src/asset_thumbnails.rs:14` | meshes x2 (`meshes/editor/src/thumbnails.rs`) | `asset_thumbnails.rs:20` (raw) | disabled meshes still renders `.mesh` thumbnails |
| `LinkedScriptLanguage { create }` | `plugin_editor_api/src/scripting.rs:155` | blueprint_editor, plus engine-owned `plugin_editor_api` and `plugin_typescript` | `scripting.rs:162` (raw), used by `pulsar_package` and the language registry | disabled Blueprints still compile |
| `PhysicsBehavior`, `EventSpec` | `physics_plugin/src/behavior.rs:84`, `physics_events.rs:45` | physics_plugin only (6 behaviors) | `behavior.rs:90`, `physics_events.rs:121` (raw, `LazyLock`) | plugin-internal extension points; fine until another plugin extends physics, then they need the gate |

### 1.6 Engine-owned or non-plugin registrations

| Type | Owner | Submitters | Readers | Note |
|---|---|---|---|---|
| `LinkedEditorProvider` | `plugin_manager/src/builtin.rs:125` | `plugin_typescript`, `plugins/trait_editor` (and docs in `plugin_manager`) | `builtin.rs:169` (raw) | two "plugins" with no descriptor; they must get one or stay engine-owned, explicitly |
| `SpaceDefiner`, `MotionGate` | `pulsar_scene_model/src/frames.rs:72`, `motion.rs:24` | `engine_backend` x2, `pulsar_scene_model` x3 / `helio_component`, `pulsar_world_registry` | `frames.rs:100,127,140`, `runtime.rs:54,118` | engine-owned; listed in `scene_inventory::WORLD_STATE_SITES` |
| `NativeRegistration`, `NativeProvider`, `GenericProvider`, `ComponentRegistration`, `ComponentProvider`, `ValueTypeRegistration`, `ValueOpsRegistration`, `EventValueCodecRegistration` | `pulsar_script_vm/src/native.rs:425-441`, `types.rs:556-1120` | `pulsar_game` x6, `pulsar_macros` x2 (the `#[blueprint]` macro, so every `pulsar_std` node), `pulsar_script_math`, `pulsar_world_registry`, `pulsar_std` | `native.rs:692-704`, `types.rs:878-926` | engine-owned until #1605(f) moves Blueprint nodes into plugins |
| `WindowRegistrant` | `window_manager/src/registry.rs:13` | `ui_gen_macros` | `window_manager/src/lib.rs:37` | engine-owned |
| `InventoryEntry` (AI tools) | `toolbelt/crates/tool_registry` | `tool_registry_macros`; consumers `ui_level_editor/src/ai/tools`, `blueprint_editor/src/ai_tools.rs`, code_editor, `agent_chat_tools` | `inventory_support.rs:100,111` | plugin AI tools are partly through this ungated list and partly `AiToolNamespace` (gated) |
| `MacroActionBuilder` | `wgpui/src/action.rs:285` | gpui macros | wgpui | UI framework; out of scope |

### 1.7 Registries that are not `inventory`

| Registry | Owner | Fed by | Note |
|---|---|---|---|
| `NodeRegistry` / `FeatureSet` / `FeatureRegistrar` | `helio-feature-api` (`registry.rs:464`, `feature.rs:34,125`) | `RenderFeatureRegistration::features()` through `plugin_features()` (flattens to one `FeatureSet`) | `NodeRegistry` stores `NodeDescriptor`s keyed by `kind`; **no feature id on a node** (`FeatureRegistrar::node` calls `registry.register(descriptor)`). `Feature::id()` is the stable feature id. |
| `BufferContract<R>` | `helio-core/src/scene_input.rs:24` | consts in plugin api and feature crates | key + row type only; no version field. Keys found in plugins: decals `decals`; fog `global_fog_media, local_fog_media, volumetric_fog_settings`; foliage `foliage_interactors, foliage_layers, foliage_types, foliage_wind`; lens `camera_lenses, lens_elements`; lights `billboard_instances`; materials `materials`; meshes `render_groups, static_objects`; particles `corona_emitters`; portals `portal_projection_counts, portal_views, portals`; post `camera_postprocess, post_process_volumes`; sky `atmospheres`; sprites `sprite_instances`; water `water_hitboxes, water_volumes` (grep of `BufferContract::new`; may miss derive-generated keys, **UNVERIFIED**). |
| `plugin_editor_api::registrar` (`Plugin`, `PluginBuilder`, `export_plugin_v2!`) | `plugin_editor_api/src/registrar.rs` | no vendored plugin; only `plugin_manager/tests/fixtures/dependency_plugins/*` | sections: files, editors, components, statusbar, subsystems. This is the "v2 builder" the issue says nothing uses. |
| `EditorPlugin*` traits, `export_plugin!`, `PermanentLibrary` | `plugin_editor_api`, `plugin_manager` | DLL plugins and the fixtures | `PluginManager::plugin_details()` (`plugin_manager/src/lib.rs:1007`) aggregates only these |
| `BuiltinEditorRegistry`, `EditorRegistry`, `FileTypeRegistry`, panel restorers (`panel_restorers()`), `asset_import::register_companion`, `thumbnails::register_thumbnail_renderer` | `plugin_manager`, `asset_import`, `engine_fs` | runtime calls | host-owned registries; ungated except through the providers that feed them |

**Summarizability conclusion.** Of about 60 registration types, none has
all four of stable id, label, category and origin without change. Types with a usable id
field: `CreateEntry`, `LevelSingletonRegistration`, `SceneSubsystemRegistration`,
`ObjectJoinRegistration`, `SpaceProducerRegistration`, `EditorLinesRegistration`,
`AssetLoaderRegistration`, `WorldComponentRegistration`, and (through `Feature::id`)
`RenderFeatureRegistration`. The `Contribution` trait (Phase 1b) therefore adds
`summary()` as a required method, and each owner decides its id; types with no
natural id get one added field (`id: &'static str`), which is a breaking change at
the submit sites that Phase 1 rewrites anyway.

## 2. Classification of raw `inventory::iter` paths

Every `inventory::iter` call in `crates/` and `plugins/` (excluding `crates/ui`,
tests, and the `pulsar_plugins::registered` wrapper itself at lib.rs:204).

| Site | Type | Class | Action |
|---|---|---|---|
| `pulsar_plugins/src/lib.rs:215` | `InstalledPlugin` | engine-owned | keep (becomes the catalog's descriptor source) |
| `pulsar_world_registry/src/runtime.rs:90-101,107-122` | WorldComponent, Tick, Event, Unfinished | engine-owned **runtime attachment** (collects each copy's list for the DLL runtime table; the gate is applied downstream in `OWN`) | keep; summary through the table, never through raw iter |
| `pulsar_scene_model/src/runtime.rs:54,118` | `MotionGate` | engine-owned, runtime attachment | keep |
| `pulsar_scene_model/src/frames.rs:100,127,140` | `SpaceDefiner` | engine-owned | keep ("statically linked plugins only") |
| `pulsar_reflection/src/runtime.rs:106` (`collected`) | 10 reflection types | runtime attachment | keep |
| SceneDB `world_mirror.rs:727-771`, `handle_ledger.rs:200`, `component_methods.rs:139` | Gpu*, HandleLedger, WorldMethod | runtime attachment (SceneDB, external repo) | keep; origin would need a SceneDB change (Q1) |
| `pulsar_script_vm/src/native.rs:692-704`, `types.rs:878-926` | natives, components, value types | engine-owned | keep until #1605(f); then gate through plugin build |
| `window_manager/src/lib.rs:37` | `WindowRegistrant` | engine-owned | keep |
| `toolbelt .../inventory_support.rs:100,111`, test `macro_tests.rs:82` | `InventoryEntry` | engine-owned reader of plugin AI tools | **must be gated** for plugin entries (needs an origin on the entry, a toolbelt change, **UNVERIFIED** how `#[tool]` expands) |
| `plugin_manager/src/builtin.rs:169` | `LinkedEditorProvider` | engine-owned (typescript, trait editor) | keep, but give both a descriptor or label them engine |
| `plugin_editor_api/src/scripting.rs:162` | `LinkedScriptLanguage` | **plugin registration that must be gated** (blueprint_editor) | add `origin`, read through `registered` |
| `helio_component/src/asset_component.rs:24` | `AssetComponentRegistration` | **plugin registration that must be gated** (meshes) | add `origin`, read through `registered` |
| `helio_component/src/asset_component.rs:60` | `AssetLoaderRegistration` | plugin registration, gate applied per load (:63) | keep, or move to `registered` at install time and re-check |
| `ui_common/src/asset_thumbnails.rs:20` | `ThumbnailRendererRegistration` | **plugin registration that must be gated** (meshes) | add `origin`; note: registered once into `engine_fs::thumbnails`, so re-gating on project switch needs the generation counter |
| `helio_component/src/components/reflected_list.rs:25` | `UiPropertyEditorHint` | engine-owned lookup (reflection) | keep |
| `engine_class_derive/src/lib.rs:437` | `ComponentMethodRegistration` | macro-generated, class-scoped | keep |
| `physics_plugin/src/behavior.rs:90`, `physics_events.rs:121` | `PhysicsBehavior`, `EventSpec` | plugin-internal extension point | keep until a second plugin registers; then gate |
| `scene_inventory/src/linked.rs:61` | Gpu*, VarLen | test-only | keep |
| `plugins/vendor/foliage/component/src/lib.rs:382`, `lights/component/src/lib.rs:189,194`, `portals/component/src/lib.rs:107`, `sky/component/src/lib.rs:262`, `splines/component/src/lib.rs:181`, `lens/component/src/component.rs:3881`, `voxel/component/tests/voxel_component_schema.rs:104` | Level singleton, ObjectJoin, SpaceProducer, EditorLines, UiPropertyEditorHint | **test-only** (all inside `#[cfg(test)]`; checked at the cited lines: assertions that the plugin registered itself) | keep; these tests also pin the `origin`-less-registration contract and move to catalog assertions in Phase 1/1b |
| `pulsar_reflection/tests`, `engine_backend/tests/script_ref_survival.rs`, `pulsar_world_registry/tests/*`, `pulsar_script_object_model/src/test_support.rs` | various | test-only | keep |

Raw sites that matter for Phase 1b: the four "must be gated" rows and `InventoryEntry`.

## 3. Plugin-to-plugin dependencies and contracts

Method: parsed every `Cargo.toml` under `plugins/vendor/<plugin>/` (excluding
examples, web/android demos, snapshot, fixtures), mapped package names to their
plugin directory, and listed edges between plugins in `[dependencies]` (N),
`[dev-dependencies]` (D). Descriptor `requires` for all of them: `[]`. Crate names
are package names (`helio-light-api` is in `lights`, `helio-mats` in `materials`,
`helio-mesh-api` in `meshes`).
"Contract" is what the crate edge appears to consume; I read the crate names,
not each use site, so the column is **crate-level evidence** and the buffer-key
level is **UNVERIFIED** (only `post` declares `consumes`; see surprise 5).

### 3.1 Normal-dependency edges (plugin level, a DAG)

| Consumer plugin | Provider plugin | Via (consumer crate => provider crate) | Contract consumed (from crate role) |
|---|---|---|---|
| asset_viewer | materials | `asset_viewer_plugin` => `helio-mats`, `materials_component` | material component types |
| asset_viewer | meshes | `asset_viewer_plugin` => `meshes_component` | mesh component types |
| decals | materials | `helio-pass-decal` => `helio-mats` | material table buffer |
| gi | lights | `helio-pass-gi` => `helio-light-api` | light rows |
| gi | materials | `helio-pass-gi` => `helio-mats` | material table |
| gi | meshes | `helio-pass-gi` => `helio-mesh-api` | static object rows |
| fog | lights | `helio-pass-volumetric-fog` => `helio-light-api` | light rows |
| lens | lights | `helio-pass-flare` => `helio-light-api` | light rows |
| lights | materials | `helio-pass-deferred-light` => `helio-mats` | material table |
| meshes | lights | `helio-asset-compat`, `helio-mesh-host`, forward-lit, gbuffer, object-batch, shadow, shadow-dirty, transparent, vg-shade => `helio-light-api` | light rows |
| meshes | materials | asset-compat, mesh-host, passes, `meshes_component` => `helio-mats` | material table, material instance |
| meshes | skm_editor | `meshes_component` => `skeletal_assets` | skeleton and skinning asset formats |
| particles | meshes | `helio-pass-corona` => `helio-mesh-api` | mesh rows |
| physics_plugin | skm_editor | `physics_plugin` => `skeletal_assets` | skeleton asset format |
| portals | materials, meshes | `helio-pass-portal-instances` => `helio-mats`, `helio-mesh-api`, `helio-pass-gbuffer`; `helio-pass-portal-cull` => `helio-mesh-api` | material table, mesh rows, gbuffer pass |
| sky | lights | `helio-pass-sky` => `helio-light-api` | light rows |
| sprites | materials | `helio-pass-sprite-batch` => `helio-mats` | material table |
| voxel | foliage, gi, lights, materials, water | `helio-pass-voxel-planet` => `helio-foliage-api`, `helio-gi-api`, `helio-light-api`, `helio-mats`, `helio-water-api` | foliage ground, GI probe interface, lights, materials, water |
| water | foliage, lights, materials, meshes | `helio-pass-water-sim` => `helio-foliage-api`, `helio-light-api`, `helio-pass-deferred-light`, `helio-mats`, `helio-pass-gbuffer`, `-indirect-dispatch`, `-object-batch`, `-occlusion-cull`; `water_component` => `helio-foliage-api` | foliage ground, lights, materials, mesh/culling passes |

Reading from the graph: `materials`, `lights` (-> materials), `meshes` (-> lights,
materials, skm) and `skm_editor` form the lower layers; `voxel` and `water` are
the highest. `water` and `voxel` depend on another plugin's **pass crates**
(`helio-pass-deferred-light`, `-gbuffer`, `-object-batch`) and not only on api
crates, which contradicts the issue's rule "other plugins depend only on a
plugin's `api` crate" (the pass crates are `feature` role crates). The root-crate
leaf rule should be written as: other plugins depend only on a plugin's `api`
crate; today `water -> lights/meshes` and `voxel -> fog/foliage/post/sky` break it
(section 3.2).

### 3.2 Dev-dependency edges and cycles

`[dev-dependencies]` add the reverse direction and make Cargo-level cycles:
`lights_component` => `meshes_component` (D), `materials_component` =>
`meshes_component` (D), `sprites_component` => `meshes_component` (D),
`foliage_component` => `helio-pass-sky` (D), `water_component` => `foliage_component`,
`lights_component`, `meshes_component` (D), `voxel_component` => `fog_component`,
`foliage_component`, `lights_component`, `post_component`, `sky_component` (D),
and the pass crates' dev edges (`helio-pass-gi` => `helio-mesh-host`, etc.).
Normal edges alone form a DAG; the dev edges are test-only. A root crate that
re-exports its api and component crates will turn some of these dev edges into
normal edges if a root imports another plugin's root; the rule "roots are leaves"
prevents that, so the plugin tests that use another plugin's component move to a
workspace-level test crate (`plugins/installed/tests` already does this).

### 3.3 Edges not captured by crates

- **Registry edges.** `voxel_component` uses `engine_backend` registration types
  (`RenderFeatureRegistration` etc.), as do all component crates; these are
  plugin-to-engine edges and stay (they are what `build` sections replace).
- **Class-name edges.** Components call each other by class name through the
  registries (e.g. `CreateEntry.id` is "so a plugin can use another's entry
  without linking it", `ui_level_editor/src/extend/mod.rs:115`). These are
  runtime contracts with no Cargo edge. **UNVERIFIED** how many exist; Phase 3
  should enumerate them with a grep for `create_entry(` and class-name string
  constants.
- **Settings groups** (`SettingsGroupRegistration`) are read across plugins by
  key (`pulsar_settings::project::rendering::groups()`); **UNVERIFIED** per key.

### 3.4 Declared vs actual

`requires` is empty everywhere, so `resolve`'s requirement pull-in and drop
logic (`pulsar_plugins/src/lib.rs:308`) never fires for real plugins; its tests
(`pulsar_plugins/src/tests.rs`, 148 lines) use synthetic descriptors. Phase 3's
`provides` and `requires` therefore must be **seeded from this table**, and a CI
test must compare descriptor `requires` against the Cargo plugin graph (normal
edges) so they cannot drift again.

## 4. Crate and layer map; where the catalog lives

### 4.1 Dependencies that decide it (read from the Cargo manifests)

```
pulsar_plugins   (leaf: inventory, serde, toml, tracing)
   ^
   | used by: engine_backend, helio_component, pulsar_world_registry, pulsar_settings,
   |          pulsar_class, asset_import, engine_state, plugin_manager,
   |          ui_level_editor, every plugin component/editor crate
plugin_editor_api (SDK; gpui)    <- plugin_manager, ui_level_editor, ui_plugin_manager,
                                    ui_core, asset_import, pulsar_package, engine
plugin_manager (host; gpui, plugin_editor_api, pulsar_plugins, pulsar_world_registry)
engine_backend -> helio_component, pulsar_world_registry, pulsar_settings, pulsar_class,
                  engine_fs, pulsar_plugins        (does NOT depend on plugin_manager)
ui_level_editor -> engine_backend, plugin_manager, plugin_editor_api, ...
ui_plugin_manager -> engine_backend, plugin_manager, plugin_editor_api
ui_core -> ui_level_editor, ui_plugin_manager, plugin_manager, asset_import, ...
engine (binary) -> ui_core, installed_plugins, plugin_manager, engine_backend
```

### 4.2 Why not in `plugin_manager`

The catalog must read registries owned by `engine_backend` (features, scene rows,
level rules), `helio_component`, `pulsar_world_registry`, `pulsar_settings`,
`pulsar_class`, `asset_import` and, for the editor, **`ui_level_editor`**
(13 extension points). `ui_level_editor` depends on `plugin_manager`, so the
catalog in `plugin_manager` is a dependency cycle for the editor half, and for
the runtime half it would make the DLL loader depend on the renderer crate
(`plugin_manager` today depends on neither `engine_backend` nor Helio).

### 4.3 Recommendation

1. **`pulsar_plugins` (stays a leaf, grows by data only).** Add:
   - `trait Contribution: Registration { fn summary(&self) -> Summary }`,
     `struct Summary { id: Cow<'static, str>, label: Cow<'static, str>, category: &'static str }`
     (the contribution kind: `"feature"`, `"create_entry"`, ...), and `origin()`.
     No registry types.
   - `enum OriginKind { Plugin(&'static PluginDescriptor), Engine, Unknown }` and
     `classify(origin)`, replacing the silent `true` in `origin_enabled` for the
     catalog (the gate's behaviour for `Unknown` stays "counts", but the catalog
     reports it with provenance; a strict-mode test fails on `Unknown`).
   - `provides` / `requires` contract ids and optionality in `PluginDescriptor`
     (Phase 3), with pure resolution and named errors. Still serde/toml only.
   - the root-crate descriptor fields (Phase 1).
2. **Owner crates export their own summaries.** Each registry owner adds a
   function `fn contributions() -> Vec<Entry>` next to its registry (`engine_backend`,
   `helio_component`, `pulsar_world_registry`, `pulsar_settings`, `pulsar_class`,
   `asset_import`, `plugin_manager` for editor providers and DLL plugins,
   `ui_level_editor::extend` for extension points, `helio-feature-api` for the
   feature-to-node map). Typed capability registries stay with their owners; the
   owners depend on `pulsar_plugins` only.
3. **New host crate `plugin_catalog`** (suggest `crates/core/plugin_catalog`, a
   workspace member beside `installed_plugins`' pattern): depends on
   `pulsar_plugins`, the runtime owners above, and with an `editor` feature on
   `plugin_manager` and `ui_level_editor`. It joins descriptors, enabled state
   (`Resolution`), modules, and the owners' `Entry`s into one `CatalogSnapshot`
   with provenance for unknown origins. A game build links it without `editor`
   (named activation errors need only `pulsar_plugins`; the diagnostics view
   needs the snapshot). `ui_plugin_manager` consumes the snapshot (Extensions
   viewer); `engine` and tests build it. The catalog depends on no plugin crate.
4. **`plugin_manager`** keeps DLL loading, `PluginDetails`, editor providers and
   dependency load order; it contributes DLL plugins to the catalog through the
   same entry type. `plugin_features()` stays in `engine_backend` for graph
   building (as the issue says).

Alternative considered: put the catalog in `ui_plugin_manager`. It already
depends on `engine_backend` and `plugin_manager` (but not `ui_level_editor`), so
it would add that edge; rejected because the runtime/game half needs the catalog
without gpui and tests need it headless.

### 4.4 Plugin root crate vs `plugin_editor_api::PluginBuilder`

`plugin_editor_api` pulls in gpui (`Cargo.toml` deps via `ui`), so a
runtime-only root crate cannot import it. Recommended split, to settle in Phase 1:

- `pulsar_plugins::PluginBuilder` (dependency-light): holds the plugin id,
  descriptor, and a typed store (`TypeId -> Vec<Box<dyn Any + Send + Sync>>`) of
  contributions, each tagged with the plugin id (origin = plugin id, not package).
  Owners add **extension traits** on it (`engine_backend` adds `render()`,
  `scene()`, ...; `pulsar_settings` adds `settings()`; `ui_level_editor` adds
  `level_editor()` under `editor`). Typed registries stay with their owners; the
  builder only stores; the catalog counts.
- The existing `plugin_editor_api::PluginBuilder` becomes the **DLL-facing**
  editor builder (`files`, `editors`, `components`, `statusbar`, `subsystems`),
  fed from the same root `build_editor` behind the `editor` feature. Either rename
  it or make it an extension on the dependency-light builder (decision Q2).

## 5. Open questions (need a decision before Phase 1)

- **Q1 (macro-submitted registrations).** Options: (a) keep derive output as
  link-time `inventory` but have it carry `origin!()` (needs `engine_class_derive`,
  `pulsar_reflection` and SceneDB (external repo) changes) and let `build` assert
  that the listed types are registered; (b) derive emits a `register(&mut Builder)`
  fn that `build` calls (no link-time discovery; the registries in SceneDB and
  reflection get fed by builder calls, which breaks the DLL attach path unless
  rebuilt). The audit recommends (a) for Phase 1 and (b) after Phase 4. The issue's
  goal "no `use installed_plugins as _`" is only met for hand-written registrations
  under (a); the derive-made ones still need the crate linked (it is, via the
  root).
- **Q2.** One `PluginBuilder` or two (section 4.4)?
- **Q3.** Is `origin` still a per-registration field after the root crate exists?
  The builder can stamp it, so the field disappears from submit sites and
  `registered::<T>()` becomes `catalog::contributions::<T>()`. Recommend yes,
  removed in Phase 1 for migrated plugins; the gate keeps accepting the field for
  unmigrated ones until Phase 1 completes for all 26.
- **Q4.** Plugins without descriptors that submit registrations:
  `plugins/trait_editor`, `crates/editor/plugin_typescript`. Give them
  descriptors (they are editor tools like `table_editor`) or declare them engine.

## 6. The plan

Principles carried from the issue: one stable plugin identity (descriptor id);
summaries derived from real registrations; typed registries stay with their
owners; static and dynamic meet at the catalog; validate before publishing;
`.trait.json` roles stay distinct from the plugin contract. Process rules are the
backlog rules in `.agents/BACKLOG_DISPATCH_PLAN.md` (branch `backlog/<id>`, draft
PR, plugin repos first, update docs, shrink-only lists).

Workers are named as in the dispatch plan. Each phase lists write scope,
tasks, acceptance and what it deletes.

### Phase 0 (this document, A0)

Done when this file merges. It is the audit the later phases cite.

### Phase 1: root crates and ownership (A1)

Write scope: `pulsar_plugins`, `plugin_editor_api`, `plugin_manager`
(builder wiring), `plugins/installed`, two plugin repos (their own PRs first),
`.agents/PLUGIN_SYSTEM.md`.

1. **Root crate shape.** One crate per plugin named by the descriptor
   (`modules.root = "<crate>"`; runtime `modules` stay for game linking). Contents:
   `pub fn build(&mut PluginBuilder)` (runtime phase) and, behind feature
   `editor`, `pub fn build_editor(&mut EditorBuilder)`. The root imports the
   plugin's `api`, `component`, `feature`, `editor` crates; the editor crate is an
   optional dependency of the root (`editor = ["dep:<plugin>_editor"]`). Roots are
   leaves: no other crate depends on a root; other plugins import only `api`.
2. **Builder.** Implement `pulsar_plugins::PluginBuilder` (typed store, plugin
   id stamped) and the owner extension traits for the registrations the two pilots
   need (section 4.4). The first cut ships sections for: render features, scene
   rows, settings groups, level-editor entries (`editor`), editor providers
   (`editor`). Other types keep their `inventory` path and the gate until their
   plugin migrates.
3. **Descriptor.** Add `root` module to `[modules]` (parse, validate, keep
   `deny_unknown_fields`); `installing_module` becomes the root. Update the
   `installed_plugins` generator
   (`UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set`)
   so `installed_plugins` depends on each **root** and a new generated
   `build_all(&mut Host)` calls each enabled root's `build` in dependency order.
   `engine` drops `use installed_plugins as _` only after all 26 are migrated
   (until then both paths exist; it is the end of Phase 1, not the start).
4. **Pilots.** Editor-only: `table_editor` (also brought into the workspace:
   remove it from root `Cargo.toml` `exclude`, switch its manifest to
   `workspace = true` dependencies; PR in Plugin_DBTable first). Object kind:
   `post` (component, feature, editor with `CreateEntry` and
   `EnvironmentListing`, 6 direct submissions) instead of `decals` (section 0.9);
   `decals` follows as the third migration to prove a no-editor-registration kind.
   Each pilot's `build` lists every registration; the scattered `inventory::submit!`
   sites are deleted in the same change (hand-written ones; derive output per Q1).
5. **Docs.** `.agents/PLUGIN_SYSTEM.md` gets a "Plugin root crate" section with
   one fully commented example (the `post` root), and `.agents/CRATES.md`
   "plugins/vendor" gets the root column.

Acceptance: `cargo test -p pulsar_plugins -p installed_plugins` (the generator
check, `a_plugin_registers_only_from_its_modules` updated to roots, `absent_plugins`
for post and table_editor: disabled plugin has no features, entries or editor);
`cargo check -p post_plugin` style per touched crate; a runtime-only check that
`cargo tree -p <pilot root> --no-default-features` has no `gpui`.

### Phase 1b: the catalog (A2)

Write scope: `pulsar_plugins`, `helio-feature-api` (Helio PR first), the owner
crates listed in 4.3, new `plugin_catalog`.

1. **Feature-to-node map.** `FeatureRegistrar::node` records the feature id on
   every node it adds; `NodeRegistry` exposes `nodes_of(feature)` and
   `feature_of(kind)`. Same for `prepare`/`finish`/`scene_derivation` counts.
2. **`Contribution` trait** (section 4.3) implemented for every registration type
   in the 1.2 table, adding the missing `id` fields; `summary()` is derived from
   the fields, never a second list. Types in 1.4 get summaries through the
   catalog's origin join only after Q1 (a) lands; until then they are counted as
   "unattributed reflection registrations" with a number, not hidden.
3. **Gate the four ungated rows** of section 2 (`AssetComponentRegistration`,
   `ThumbnailRendererRegistration`, `LinkedScriptLanguage`, plugin `InventoryEntry`)
   with `origin` and `registered`.
4. **`plugin_catalog` crate** with `CatalogSnapshot`: descriptors, enabled state,
   modules, contributions by owner plugin, engine-owned contributions labelled
   as the engine's, and unknown origins reported with provenance
   (`classify` -> `OriginKind::Unknown { package }`), not guessed from names.
   `plugin_features()` stays for graph building; the UI reads the snapshot.
5. **Strict mode test:** with every plugin linked, no registration has
   `OriginKind::Unknown` unless listed in a shrink-only allow list
   (`plugin_catalog/allowed_unknown_origins.txt`: `plugin_typescript`, `trait_editor`
   at first, per Q4).

Acceptance: renderer fixture from the issue: one plugin with several features and
nodes, the catalog reports the feature-to-node map and the graph builds with the
same node ids; disabled-plugin fixture: every registration from its modules
filtered, project data diagnosable. Commands: `cargo test -p plugin_catalog -p
helio-feature-api -p pulsar_plugins`; the rendering rule applies
(`helio-architecture` skill, four-part report) because `helio-feature-api` and
renderer registration change.

### Phase 2: Extensions viewer on the catalog (A8, first half)

Write scope: `ui_plugin_manager`. Extensions reads `CatalogSnapshot`: features
with their nodes and passes, settings, editors, importers and the rest under the
owning plugin. Delete the Renderer tab (`RegistryView::Renderer`,
`load_renderer_features`, `screen.rs:19,68,304,348`) and `render_renderer_workspace`.
Engine-owned contributions are labelled. Counts, filters, dependencies and enabled
state come from one snapshot. Acceptance: a headless snapshot test (the crate
already builds a headless suite in `plugin_manager` dev-deps; **UNVERIFIED** for
`ui_plugin_manager`) and a manual check recorded in the PR.

### Phase 3: provided and required contracts (A8, second half)

Write scope: `pulsar_plugins` (descriptors, resolution), the 26 descriptors (each
in its plugin repo PR), `plugin_catalog`, `helio-feature-api`, feature crates.

1. Descriptor fields: `provides = [{ id, kind }]`, `requires = [{ id, optional }]`
   with contract ids such as `pulsar.lights.light_rows`, `pulsar.materials.table`
   (names follow section 3.1; the buffer keys in 1.7 are the first provided
   contracts). **No version ranges** until Phase 4 (review change 1).
2. Activation resolves required contracts against enabled providers; failure
   names consumer, contract and provider
   (`plugin pulsar.meshes needs contract pulsar.lights.light_rows, provided by
   pulsar.lights, which the project disables`). An absent optional contract is
   recorded as such, not an error.
3. Fix surprise 5: every feature calls `consumes` (or the registrar derives it
   from the pass `scene_buffers`), and the catalog links each `ContractUse` to the
   provider plugin through `provides`.
4. Seed `requires` from section 3.1 (normal edges); add the CI test that descriptor
   `requires` equals the Cargo plugin graph (normal edges), so they cannot drift.
5. Fixtures from the issue: missing, disabled, and (stub) incompatible provider
   each produce a named activation error.

### Phase 4: binary plugins (A10)

Write scope: `plugin_editor_api`, `plugin_manager`, `pulsar_package`, CI.
From #1547: contract versions and automatic layout fingerprints (size, alignment,
field hash) checked at load; DLL plugins expose the same root `build` over the FFI
(the existing `_plugin_manifest` export carries the descriptor; `_plugin_create`
returns the builder result); dual build (cdylib and static through the same
`build`; `physics_plugin` has a `builtin` feature); bundling first-party plugins
into a few DLLs; `replaces =` manifest decision (logged); CI running
`plugin_dependencies` and `world_component_plugins`; and the **no-statics test for
api crates and `helio-feature-api`** with the allow list from surprise 6
(`materials/api`, `lights/api`), shrink-only. Known limits (no unload, matching
rustc) stay documented.

### Phase 5: component class API (A9)

Write scope: `plugin_editor_api`, `pulsar_scene_model` attachments,
`pulsar_world_registry`, `ui_level_editor` inspector, `pulsar.character` plugin
(new repo). From #1534: `p.components().class::<T>(ClassSpec { requires, systems,
gpu, presentation })`; `requires` auto-attach as component-instance entities
(`pulsar_scene_model::attachments`) with the loader checking the required class
comes from a plugin in `requires`; `Facade` presentation; system registration with
ordering labels replacing per-crate `ComponentTickRegistration` wiring;
`UnresolvedComponent` round-trips through the catalog. Proof: `pulsar.character`
(skeletal, physics, optional input) places in a level, plays, and loads with data
preserved when removed. Note the V2 Phase 2 items already done elsewhere
(per-plugin ledger rows is #1605(e), physics move is #1605(a)); do not redo them.

### Traits, VFS and role work (decided in the issue)

Mapped onto the dispatch waves; each is independent of Phases 1 to 3 except where
noted.

| Step | Wave | Scope | Depends on |
|---|---|---|---|
| **T1: read-only VFS mounts** (A4) | 3 (quiet window) | `engine_fs::virtual_fs`: mounts composed with the active provider (`virtual_fs/mod.rs`, `providers/*`); write to a mount fails with a named error; `plugins/<plugin id>/` mount per enabled plugin; the file drawer shows it read-only (`ui_file_manager`); built-in traits served from a read-only `pulsar` mount, deleting `builtin_traits::ensure`/`sync_one`/`by_path`-driven re-sync and the drift warnings (`engine_fs/src/builtin_traits.rs`, call sites `blueprint_trait_index.rs:143,261,568,661`, `trait_db.rs:131`); an on-disk `plugins/` folder in a project is reported as a problem | Phase 1b (mount list comes from enabled plugins) |
| **T2: all file IO through `engine_fs`** (A5) | 3, zero other writers | complete missing ops; migrate `std::fs` call sites (count first, section 0.11); add a root `clippy.toml` with `disallowed-methods` for `std::fs` and a shrink-only allow list (the VFS local provider, build scripts, tests); split mechanically by crate group, no two groups share a crate | T1 |
| **T3: role registry** (A6) | 4 | replace `engine_fs::world_roles::ROLES` (4 entries, `world_roles.rs:54`), `WorldSettingsData` role fields (`ui_level_editor/src/core/world_settings_data.rs`), `EngineRole` match arms (`pulsar_game/src/roles.rs`, `scripting/driver.rs`); World Settings picker via level-editor extension points; role lifecycle events (spawned, possession changed, game begin/end) on the Gamma hub | Phase 1b (registry entries are catalog contributions) |
| **T4: implementer sources** (A7) | 4 | `TraitImplementerSource` registration (kind id, scan from VFS paths, binder to callables), origin-gated like the others; Blueprints first (registered by the Blueprint plugin), native components second (from reflection); `BlueprintTraitIndex` becomes one trait index and `TraitImplementer::kind` is the source id; native trait bindings onto `ComponentMethodRegistration`/`MethodMetadata`, checked by `engine_fs::trait_conformance`; one dispatcher (`RuntimeTraits`, today `call_trait_hook` at `pulsar_game/src/scripting/driver.rs:2224`) for Blueprint and native implementers; plugin trait ids namespaced `<plugin id>.<Trait>` with `contractVersion`, the catalog rejecting a trait outside its namespace | T3, Phase 1 |
| **T5: catalog entries and proof** (A8) | 4 | trait definitions, roles, implementer sources and implementers as catalog contributions; cross-plugin trait use declared as `requires` (Phase 3); fixture plugin defines a trait and a role, implements natively; a Blueprint implements it; both dispatch in a game; disabling the plugin leaves "Missing trait" diagnostics | T1, T3, T4, Phase 3 |

Plugin trait definitions are registered from the root `build` and served from the
plugin's `plugins/<id>/` mount (so T1 precedes the trait registration), and the
fixture plugin is the proof for the statics, contract, renderer, disabled-plugin
and trait+role fixtures listed under "Proof" in the issue.

### Order, conflicts and cost

- A0 (this) -> A1 -> A2 -> A3 (#1605, kernel items) -> [quiet] T1 -> T2 -> T3 ->
  T4 -> Phase 2/3/T5 (A8) -> Phase 5 (A9) -> Phase 4 (A10). Phases 1/1b write
  `pulsar_plugins`, `plugin_manager`, `engine_backend` registration files and the
  plugin repos that A3's items also touch; run serially as the dispatch plan says.
- Plugin repo changes go in the plugin repo first (dependency-first merges, pin
  update, `.github/submodule-pin-exceptions.txt`). Do not start with
  `input_plugin`, `matter_editor`, `physics_plugin`, `ui_editor` (section 0.10).
- Per-phase checks name touched crates only; no workspace builds until the
  Phase 1 end-state removal of `use installed_plugins as _`
  (`cargo check --workspace` once, by the coordinator).
- Rendering-adjacent work (Phase 1b `helio-feature-api`, plugin feature crates)
  loads `helio-architecture` and ends with Design, AAA comparison, Deleted,
  Verified.

## 7. Documents to delete

Not deleted by this change; the merge that lands this plan deletes them and
updates links.

| File | What survives | Where |
|---|---|---|
| `.agents/PLUGIN_API_V2.md` | the "sharing types between plugins" analysis (component ids follow type name and layout, not `TypeId`; fallback by-name access) and the dual-build rationale | Phase 4 here; the move-out estimates and pass-crate tiers are superseded by the plugin repo split (p18) and #1605 and are dropped. The "registrar" design (`p.files()`, `p.render()`, ...) is superseded by the root-crate `build` (Phase 1). The Phase 0/1 "as built" notes (manifests, load order, `catch_unwind`) remain true and are recorded in `PLUGIN_SYSTEM.md` ("Manifests and plugin dependencies"). |
| `.agents/PLUGIN_API_V2_REMAINING.md` | open items map to Phases 1 to 5 and the dispatch plan; "Plugin Manager window details page" is done; "stop `ui_core`/`scene_inventory` naming plugin crates" is #1605(b) | Phase 5/#1605 |

References to update when deleting: `.agents/PLUGIN_SYSTEM.md` (last paragraph
cites `PLUGIN_API_V2.md`), `AGENTS.md` quick index (no row today; add one for this
plan), the `#1532` family of issues (closed into #1608).

## 8. Unverified or not read

- Whether the 26 `plugins/vendor` descriptors and the `plugins/trait_editor` /
  `plugin_typescript` crates are the complete plugin set (no descriptor under
  `plugins/vendor/*` was missing: all 26 directories have one).
- Buffer-key-level contracts across each plugin edge (section 3.1 is crate
  evidence). Class-name and settings-key edges (3.3) are not enumerated.
- Which registrations the derive macros emit exactly, beyond the textual
  `submit!` sites and the names in `engine_class_derive/src/lib.rs` (read only at
  the cited lines). The textual count (about 170 hand-written sites in plugins) is
  a grep, not a parse; macro expansion adds more.
- SceneDB (external git dep) was read at the local checkout of the pinned rev
  only for its `collect!`/`iter` sites.
- `.github/submodule-pin-exceptions.txt`, CI workflows, and whether
  `ui_plugin_manager` has a headless test harness.
- The `std::fs` count methodology (section 0.11).
- How `toolbelt`'s `#[tool]` macro expands and whether it can carry an origin.
- DLL-plugin behaviour (origin gating "statically linked plugins only" per
  `scene_inventory::WORLD_STATE_SITES`); the Phase 4 design for DLLs is not
  audited beyond `PLUGIN_SYSTEM.md`.
- No `cargo` command was run (per the brief); no claim here is a build result.
