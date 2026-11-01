# Plugin API v2: what must be done

Companion to `PLUGIN_API_V2.md` (design, audit). Phases 0 and 1 are done
(branch `plugin-api-v2-phase1`). This file lists the rest as work items, in
order, each with the proof that it is finished. Tick items as they land and log
problems under "Issues" at the bottom; clean those up after Phase 6.

## Done

- [x] Phase 0: measured audit (`PLUGIN_API_V2.md`, "Phase 0 audit").
- [x] Phase 1: manifests, `requires`/`optional`, dependency-ordered loading,
      registrar API (`Plugin`, `PluginBuilder`, `export_plugin_v2!`),
      `catch_unwind` on entry points, fixtures and tests.
- [x] Plugin Manager window: click a plugin for a details page (back button)
      with dependencies, what requires it, and what it provides.

## Phase 1 leftovers

- [ ] Rebuild the excluded plugin repos (`blueprint_editor`, `code_editor`,
      `shader_editor`, `matter_editor`, `table_editor`) against the new macro and
      confirm they load.
- [ ] Give `physics_plugin` and `skeletal_animation_plugin` real manifests
      (`skeletal_animation_plugin` depends on `physics`; `physics` on `skeletal`
      assets) so the details page shows real edges.
- [ ] Decide `catch_unwind` coverage for the other `extern "C"` entry points
      (`_plugin_init_globals`, the attach functions).
- [ ] Stop `ui_core` and `scene_inventory` naming `physics_plugin` and
      `skeletal_animation_plugin` crates directly (Phase 0 finding 4); they
      should reach plugins through the registrar.

## Phase 2: full component API

- [ ] `p.components().class::<T>(ClassSpec { requires, systems, gpu, presentation })`.
- [ ] `requires`: attach required classes as component-instance entities
      (`pulsar_scene_model::attachments`) when a class is placed; loader checks
      the required class comes from a plugin in `requires`.
- [ ] `Facade` presentation: one inspector card and one script-visible
      component over several real components.
- [ ] Systems: lifecycle and tick registration with ordering labels, replacing
      per-crate `ComponentTickRegistration` wiring.
- [ ] Per-plugin GPU schemas and namespaced buffer keys; split
      `scene_inventory/ledger.toml` so each plugin owns its rows and
      `cargo test -p scene_inventory` checks both.
- [ ] Cross-plugin typed access: layout fingerprint for each plugin API crate,
      "no statics in API crates" test, trait dispatch by registered tables or
      class name. If too fragile, fall back to by-name access and record it.
- [ ] Move `pulsar_physics` (2.1k lines) into `physics_plugin` as the first
      proof.
- Proof: plugin B places plugin A's component, reads it typed, and a level with
  both round-trips; removing A leaves B's data as `UnresolvedComponent`.

## Phase 3: render registration

- [ ] Split `helio-pass-api` (the `RenderPass` trait and its context types)
      out of `helio-core`; no statics in it.
- [ ] Shader hot-reload registry (`helio-core/src/shader/hot.rs`): shared
      runtime table or host-only behind the API.
- [ ] `p.render().pass_node(..)`, `.pipeline(..)`, host inputs: plugins add
      `NodeDescriptor`s to the process registry before the one-time probe.
- [ ] A graph naming a node from an absent plugin reports the plugin by name
      and falls back.
- [ ] Retire the legacy `add_pass` builders so plugin passes are not blocked.
- [ ] First users: voxel passes register nodes (closes the RENDER_GRAPH.md
      "voxel passes" gap).
- [ ] Decide tier C (core deferred set): stay core or become `helio` features.
- Proof: a fixture plugin adds a pass and a buffer; the node shows in
  `render_bps` with probed pins and renders.

## Phase 4: assets and tooling over the registrar

- [ ] Importers, thumbnail renderers and import companions for DLL plugins
      (currently link-time `inventory` only).
- [ ] Script natives and trait roles via `p.scripting()`.
- [ ] AI tools via `p.ai()`.
- [ ] Settings and events sections.
- Proof: the shader plugin's material import companion works as a DLL.

## Phase 5: move-outs (one domain at a time, tree green after each)

Order by difficulty (numbers from the audit):

- [ ] Tier A passes, 24 crates, about 52k lines: move node registration next to
      each pass, then the crates into plugins.
- [ ] `helio_component` split by component. Needs `ui_level_editor` and
      `engine_backend` to stop naming component types first: edit components
      through reflection. Easy ones first (Decal, Sprite, ParticleEmitter,
      ReflectionCapture, CameraLens, Fog); Voxel, Water, Foliage, Atmosphere,
      Spline, PostProcess, Portal later.
- [ ] Tier B passes with their components (voxel-planet 48k, lens, water-sim,
      foliage-place, volumetric-fog, sky, corona, sprite-batch, decal).
- [ ] `engine_backend` voxel backend (`voxel_backend.rs`, `scene/voxel_*`),
      sublevels, portals, authored LOD, physics subsystem, networking.
- [ ] Level-editor component panels move with their components.
- [ ] `pulsar_std` per-domain Blueprint nodes ship from the owning plugin.
- Rule: each move deletes more central code than it adds.

## Phase 6: first product

- [ ] `pulsar.character`: a character component built from a skeletal mesh
      component, a controller and a camera rig, shown as one component, living
      in a standard-library plugin that depends on the others.
- Proof: place a character in a level, play it, remove the plugin and see the
  level load with the data preserved.

## Cross-cutting

- [ ] Dual build: every first-party plugin builds as a cdylib and links
      statically into a packaged game through the same `build` call
      (`physics_plugin` already has a `builtin` feature).
- [ ] Bundle first-party plugins into a few DLLs to limit disk and memory from
      per-plugin copies of the world crates.
- [ ] Decide whether a plugin may replace a core pass node (`replaces =` in the
      manifest, logged).
- [ ] CI: run `plugin_dependencies` and the world-component plugin tests; add a
      load-order test with real vendored plugins once they declare dependencies.
- [ ] Docs: keep `PLUGIN_SYSTEM.md`, `CRATES.md` and `RENDER_GRAPH.md` in step
      with each phase.

## Known limits (stay true)

- Plugins are never unloaded: no hot reload; iterating on a plugin means
  restarting the editor.
- Every plugin must match the engine's rustc.
- Plugin manager window shows state at open and on Refresh; it does not watch.

## Issues

(none logged yet)
