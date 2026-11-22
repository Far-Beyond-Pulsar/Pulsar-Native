# Legacy, deprecated and compat markers

An inventory of the places the source calls something "legacy", "deprecated"
or kept "for compatibility". It does not change code. Each row says what the
marker is, whether the old path still has callers, and what to do with it.

How it was made: a search over first-party Rust (the same scope as
`architecture/STYLE.md`) for `legacy`, `deprecated`, `back-compat` and
`compat` used as a marker found 249 lines in about 80 files, grouped into the
rows below. (The broader pattern in `READABILITY_REPORT.md` also catches
`compatible` / `compatibility` in the ordinary sense, 358 lines; those are
listed in the last section.) "Callers" were checked by `grep` over `crates/`
and `plugins/installed/` (and `plugins/vendor/` where noted); a name reached
through a macro or reflection would be missed.

Suggested action: **remove** (dead, or only a stale comment), **migrate**
(real work to retire it), **keep** (a reader of data that exists on disk;
remove when the old data cannot occur any more).

Not verified: about 25 of the 249 lines (rows marked "not traced"). The rest
are covered by a row.

## Dead or stale: remove

| Where | What | Callers | Action |
|---|---|---|---|
| `crates/editor/ui_common/src/generic_window.rs:82` | `drawer_window!`, an alias of `pulsar_drawer_window!` "for backwards compat during migration" | none in `crates/`, `plugins/installed`, `plugins/vendor` | remove |
| `crates/core/engine_backend/src/subsystems/render/mod.rs:18` | `pub struct WgpuRenderer;` "Stub for compatibility", re-exported at `engine_backend/src/lib.rs:35` | none outside the re-export (also none in `plugins/vendor`) | remove the stub and the re-export |
| same file, `Framebuffer` (line 22), re-exported as `RenderFramebuffer` | struct of the same stub module | only the re-export | remove with the stub |
| `.../helio_renderer/renderer.rs:223,678,1271`, `services/gpu_renderer.rs:536-542,548` | `RendererCommand`, `command_sender`/`command_receiver` ("Legacy feature commands", "Compatibility types retained for existing UI wiring"). The renderer drains and ignores the queue | `GpuRenderer::send_renderer_command` has no callers anywhere | remove the enum, the channel, the drain and `send_renderer_command`; fix the `Send` SAFETY comment at `gpu_renderer.rs:548` |
| `crates/editor/ui_level_editor/src/workspace/panels/properties.rs:40-42` | `editing_property` / `property_input` fields "DEPRECATED: Old manual property editing (will be removed)" | only constructed, then passed to `ui/properties/mod.rs:33-34` as `_editing_property` / `_property_input` (unused) | remove the fields and the two parameters |
| `crates/core/engine/src/init/graph.rs:121-123` | commented-out `window_tx` / `window_rx` "legacy channel fields (unused)" | none (comment) | remove the comment |
| `crates/editor/ui_level_editor/src/ui/properties/object_type_fields/mod.rs:13` | doc says the legacy "Object Type" card "has been removed" | n/a (history) | remove the sentence |
| `crates/core/pulsar_game/src/lib.rs:50` | `// Legacy tick loop` above `pub mod tick` | `pulsar_game::tick::TickLoop` is the live loop (`core_project_builder/templates.rs:20`, `embed`) | fix the comment; nothing to remove |
| `crates/core/engine_backend/src/services/pie_host.rs:215` | `scene_path` "kept for logging/legacy guests" | v1 guests are rejected by the version gate (`pulsar_pie_abi/src/lib.rs:225`) | reword: advisory, for logging |
| `crates/core/engine/src/assets.rs:96` | `pub use CombinedAssets as Assets` "for backward compatibility" | `crate::Assets` is the live name (`startup.rs:50`, `main.rs:71`) | keep the alias; drop the comment or rename `CombinedAssets` |
| `crates/core/pulsar_content/src/pak/writer.rs:22,321-327` | `LEGACY_PAK_FILE = "game.pak"`, removed from the output directory when a pack is written | used only inside this file | keep for one more release, then remove |

## Live migrations and compatibility readers: keep

These read data that older versions wrote. They have callers. Each can go
when the old data can no longer occur; none has a removal date today.

| Where | What | Callers | Action |
|---|---|---|---|
| `crates/core/asset_import/src/db.rs:56-232,322-348`, `service.rs:21-26`, `importer.rs:62` | import options kept in the legacy asset-keyed store (`.pulsar/import_options.json`) folded into `ImportDb` records | `ImportDb::load_with_migration` from `service.rs:21`; convert-in-place entries stay on the legacy store by design (`AGENTS.md`) | keep: not removable while convert-in-place imports exist |
| `crates/core/engine/src/steps/settings.rs:21,66` | migrates the old `allow_unsafe_process` preference out of the config file | runs at startup when the key is unsaved | keep |
| `crates/core/pulsar_scene_model/src/hierarchy_transform.rs:161,182,211,217,315` | world-space to parent-relative transform conversion for scene files older than 2.2 | `engine_backend/src/scene/runtime_level.rs:247`, `ui_level_editor/.../level_io.rs:418,545`, `ui/panel/lifecycle.rs:168` | keep until file versions below 2.2 are unsupported |
| `crates/core/engine_backend/src/scene/runtime_level.rs:8,197,276,307,413,656,763,768` | legacy loader notes, legacy `blueprint_bindings` read through the class migration, legacy component records | in the level load path | keep |
| `crates/subsystems/pulsar_scene/src/format.rs:48,49,285,340,396`, `loader.rs:2,5,19`, `lib.rs:8` | `blueprint_bindings` level-format section, legacy tagged kinds, `props["__component_instances"]` fallback | read at `runtime_level.rs:171,302,400-429`; the section is no longer written for migrated entries | keep; the one-class-per-object case still carries bindings over |
| `crates/editor/ui_level_editor/src/core/scene_edit/mod.rs:128,173-198`, `objects.rs:385,474-486`, `level_io.rs:162,237`, `classes.rs:26` | legacy Blueprint bindings and script-path adoption (`adopt_legacy_script_path`) | called from `add_object` when a script path is given | keep; migrate the callers that still pass a path, then remove |
| `crates/core/pulsar_class/src/migrate.rs:57,61,75,110,421,454`, `records.rs:11,35,55`, `lib.rs:21`, `registry.rs:339,450,652`, `prefab.rs:382-491` (a test fixture named `W6LegacyComponent`) | legacy `props.__component_instances`, record migrations, `script_asset` path resolution | level load, `cook.rs:5,200` | keep |
| `crates/core/pulsar_class/src/modules.rs:24,248-392` | `migrate_legacy_modules`: moves `events/.build` modules into the module store | `pulsar_package/src/compile.rs:76`, `ui_loading_screen/src/tasks.rs:163` | keep until no project has `events/.build` |
| `crates/editor/ui_core/src/app/layout/format.rs:44,168,214` | layout file version 1 (single window) | read on every restore | keep |
| `crates/editor/ui_core/src/custom_providers.rs:18-73` | moves the old custom-provider file into the provider store, then deletes it | `load_provider_instances` from `agent_chat_panel/builder.rs:76` | keep, then remove after a few releases |
| `.../helio_renderer/pipelines.rs:761-885`, `ui_level_editor/src/core/world_settings_data.rs:147-160` | `LEGACY_DEFAULT_PIPELINE = "helio.deferred"` for levels saved without a pipeline id; `migrate_pipeline_id` | level load; used by 10 tests under `plugins/installed/tests` | keep |
| `crates/core/engine_fs/src/virtual_fs/mounts.rs` (module docs, `ENGINE_FOLDER`), `mounts/projects.rs` (`resolve_segment`, `canonical_mount_path`), `mounts/tree.rs`, `mounts/tests.rs`, `pulsar_traits/src/db.rs:245` | `plugins/<plugin id>/` accepted as an alias of `plugins/Pulsar/` | mount resolution | keep |
| `crates/core/engine_fs/src/file_index/schema.rs:50-65` | SQL: renames the old `asset_ids` table while upgrading the index schema | on schema upgrade | keep (a one-off upgrade step, not a legacy code path) |
| `crates/core/pulsar_game/src/camera_selection.rs:265-374` | "legacy Camera object": an `ObjectType::Camera` with no `CameraComponent` still selects the camera, below component priority | live tier in `select_world_camera` | keep; rename "legacy" to what it is |
| `crates/core/pulsar_std/src/engine/nodes/events/mod.rs:46-49` | `emit_event`, a "retired" no-op Blueprint node | the compiler lists it in `RETIRED_EVENT_NODES` (`plugins/vendor/blueprint_editor/compiler/src/lib.rs:194`) so old graphs still load | keep while old graphs may exist |
| `crates/core/pulsar_std/src/registry.rs:26` | string-based type lookup "for compatibility" in `NodeParameter::get_type_info` | live fallback for non-`Reflectable` types | keep; reword (it is a fallback, not compat) |
| `crates/core/pulsar_studio_client/src/lib.rs:89` | falls back to the legacy lobby endpoint when there is no environment id | live | keep until the server drops it |
| `crates/core/friends_engine/src/types.rs:145` | friends file accepts legacy string entries | persisted data | keep |
| `crates/core/level_migrate/src/main.rs:140,213,335` | wording inside the repair tool | the tool itself is the migration | keep |
| `crates/editor/ui_core/src/app/construction.rs:320` | "(compat path)" when no splash-staged plugin manager exists | `create_plugin_manager` fallback | keep; reword |
| `crates/editor/ui_level_editor/src/core/scene_edit/default_level/mod.rs:181`, `pulsar_class/src/records.rs:11` | the demo level builder writes `legacy_material_override` into new mesh data; load folds it into the mesh component | live, and used by tests in `plugins/installed` | migrate: write the real material field, then retire the key |
| `agent_chat_tools/src/plugin_tools.rs:354` | `execute_plugin_tool`, a "back-compat wrapper" around `call_plugin_tool`, exposed as an AI tool | only `ui_core/src/app/manual_tool_panel.rs:329` (hides it) | migrate: check nothing prompts for the name, then remove both |
| `crates/core/pulsar-relay/src/rendezvous/sync_protocol.rs:93,209`, `pulsar_studio_client/src/lib.rs:223,342` | "Legacy file-based messages" in the sync protocol | `RequestFile` / `FileChunk` are used by `engine_fs/src/providers/p2p.rs`, so the label is wrong or the protocol move is unfinished | migrate (decide), then fix the label |

## Tests and fixtures that name legacy shapes

Keep: they test the migrations above. `plugins/installed/tests/` (`default_pipelines.rs`,
`level_editor.rs`, `render_acceptance.rs`, `runtime_render_parity.rs`, `skinned_frames.rs`,
`texture_heavy_level_probe.rs`, `virtual_geometry*_acceptance.rs`, `voxel_render_parity.rs`,
`absent_plugins.rs`) use `LEGACY_DEFAULT_PIPELINE` and `legacy_material_override`;
`pulsar_game/tests/component_event_routing.rs:127`, `pulsar_traits/src/records.rs:330`,
`pulsar_traits/src/index/tests.rs:162`, `ui_types_common/src/validation.rs:316`,
`level_migrate` tests, `pulsar_package/tests/cooked_level.rs:56`, `physics_playground.rs:129`.

## Not traced

These read as legacy markers but were not followed to their callers:
`engine_class_derive/src/properties.rs:115` (the single-string `#[category("X")]`
fallback; no use found in `plugins/vendor`, but `camera_component.rs` shows only the
multi-argument form, so it may be removable), `engine_class_derive/src/world_component.rs:13`,
`pulsar_world_registry/src/registration.rs:58` and `component_lifecycle/tests.rs:78`,
`ui_level_editor/.../property_renderer.rs:8`, the `installed_plugins::physics_plugin::legacy`
module used by `ui_level_editor/src/core/scene_edit/tests/components.rs:2,293` and
`render_acceptance.rs:2723,2846` (it lives in `plugins/vendor/physics_plugin`, outside this
audit), and `pulsar_package/src/cook.rs:5,200`.

## Not a legacy marker

Matched by the pattern but not legacy code: `agent_provider_openai` ("OpenAI compatible"
endpoints), `plugin_editor_api` / `plugin_manager` version compatibility, `pulsar-relay`
("post-quantum disabled for Windows compatibility", NAT candidate compatibility),
`core_project_builder.rs` dependency pins (`pin_compatible_dependencies`, "deprecated Linux
`ENOATTR`"), `pulsar_lsp/src/capabilities.rs` (LSP `deprecated` tag), `ui_keybindings`
(`deprecated_actions_to_preferred_actions` is a GPUI API), `ui_loading_screen/src/compatibility.rs`
(the version compatibility notice), `engine/src/dxc.rs:98` ("legacy shader compiler" = FXC),
`engine_backend/build.rs:5` (`helio-asset-compat` is a crate name), `engine/src/assets.rs:60,69`
("RustEmbed-compatible").
