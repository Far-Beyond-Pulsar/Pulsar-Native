Status: phases 1 to 6 built (2026-11-27), uncommitted; not yet checked in the running editor.

# One asset system: handles, processing and GPU residency

A component names an asset and holds only its handle. SceneDB loads it, runs
the owner's processor, shares the result in memory and on the GPU, and lets it
go when nothing uses it. This extends SceneDB's asset layer
(`pulsar_scenedb::assets`); it is not a second system.

## Rules

1. **Components never load, encode or upload.** They store an `AssetId`.
2. **SceneDB is type agnostic.** It runs what an owner registers. It never
   packs or decodes buffer data; the writer and reader of that data own its
   layout, and a shader still reads a plain buffer.
3. **One source, several GPU forms.** A GPU form is keyed by the asset and the
   *variant* the owner asks for (for an image: its use). The asset id stays the
   same.
4. **An asset may need assets** (a material its textures). Those are
   references like a component's: loaded, shared, reloaded and released by the
   same rules. No loader decodes another asset itself, and no plugin keeps a
   reverse map of who uses what.
5. **Lifetime follows use.** A GPU form no live asset or component needs is
   released.

## What exists and stays

`AssetId`, `declare`, per-world reference counts (`handle_ledger`), worker
loads, tiers, priorities and budgets, bindings, reload, the payload cache
budget, `Residency<T>` (CPU forms) and `derive_cached` (disk).

## Gaps (audit, 2026-11-27)

| Gap | Where |
|---|---|
| GPU upload and sharing live outside the asset system, in two process-wide tables | `helio_component/src/texture_assets.rs` (`RESIDENT`, `SHARED`) |
| Nothing releases a texture slot when its last user goes (only reload, or the store dropping) | same |
| An asset cannot declare the assets it needs | `pulsar_scenedb/src/assets.rs` |
| Materials decode their textures inside the material loader and keep a reverse map for reload | `plugins/vendor/materials/component/src/assets.rs` (`load_graph`, `texture_users`) |
| Other readers open image files themselves | sky `sky_texture.rs`, materials `vg_alpha.rs` and `preview_program.rs`, meshes `snapshot/src/renderer.rs` |

Not part of the model: images compiled into the binary (icons, the UV grid,
the moon map), editor-only previews and viewers, and render targets and
simulation buffers. They are not project assets shared between components.

## Phases

1. **GPU residency in SceneDB.** `pulsar_scenedb::gpu::shared_textures`: one
   table of GPU textures keyed by (texture store, asset, variant) and shared
   per device, staged or whole upload, slot reuse. `helio_component` supplies
   the processor (cook, format choice, detail bias) and its two tables go.
2. **Asset dependencies.** A loader may return the assets its payload needs.
   The world counts them while the payload is referenced, loads them through
   the same pump, and rebinds the parent when one changes.
3. **Materials.** A graph names its textures as dependencies; the material
   loader decodes none, and `texture_users` goes.
4. **Release.** GPU forms of assets with no references are released.
5. **Remaining readers** in the gap table move to the system, or are recorded
   here with the reason they stay.
6. **Delete and document.** `.agents/MEMORY.md`, `SCENEDB_MIGRATION.md`,
   `LEVEL_LOADING.md`; ledger rows.

Meshes and animation already use SceneDB loaders. Mesh geometry's GPU sharing
is the interned pool, keyed by content, which already follows the rules.

## As built: phase 1

`pulsar_scenedb::gpu::shared_textures` (`acquire`, `holds`, `release`,
`TextureRequest`, `TextureImage`), with its table in the asset registries so a
plugin library shares it (`runtime::ABI_VERSION` 6). `helio_component` keeps
the processor: format choice, cooking and the detail bias. `release` has tests
but no caller yet.

## As built: phases 2 and 3, dependencies and materials

`register_composer(kind, dependencies, compose)`. The world holds what a
payload needs as references, loads it like any asset, and composes the parent
on a loader thread once each need has loaded or failed, again whenever one
changes. A parent is `complete` when its needs are. Needs rank for loading as
what needs them does; a cycle fails; dropping a cached parent releases what it
held. `compose_asset_now` is the same outside a world.

The step is the *owner's*, so a need does not name a GPU form: the material's
compose step prepares each texture for the use the graph makes of it
(`LoadedTexture::prepare`). So the texture loader does not encode forms nobody
wants, `needed_texture` and `collect_texture` note the uses each file is wanted
for. That note is only a saving: whoever binds prepares what is missing.

Materials (`AssetLoaderRegistration::compose`): the loader reads the graph and
names its textures; it decodes none. Textures load coarse first (decided with
the user, 2026-11-27), so a material shows and then sharpens. Each texture's
GPU copy follows the texture's version, not the material's: one sharpening
re-uploads only itself, and a component and a material showing one file now
share one slot (they keyed it by different versions before, and replaced each
other's). Deleted: the loader's decoding, `texture_users`,
`graph_sources_sampling` and its reload registration.

## As built: phase 4, release

See "Release" in `assets.rs`. An asset is in use while a component references
it or an asset in use needs it. After `set_asset_release_delay` pumps (120)
out of use, the world's texture store gives back its slots
(`shared_textures::release`). **Whoever keeps a slot must reference the asset
it came from.**

## Phase 5: remaining readers

| Reader | Decision |
|---|---|
| `materials/api/src/vg_alpha.rs` | Its image reads are in tests |
| `materials/component/src/preview_program.rs` | The material editor's preview; editor-only |
| `meshes/snapshot/src/renderer.rs` | The offline snapshot tool |
| `sky/feature/src/sky_texture.rs` | Stays for now: one panorama owned and streamed by the renderer feature, shared by no components; a Helio-layer crate that reads files itself. Moving it means the sky component naming a panorama asset whose GPU form is its float mip chain |

## Issues log

(Non-blocking problems found on the way, resolved in phase 6.) Failing before
this work, in code it does not touch:

- `pulsar_scenedb`'s unit tests do not compile (`super::vram`, and
  `pulsar_reflection::MethodType`), nor does `subsystem_reflection_test`.
- `pulsar_scenedb`'s `world_state`: two unlisted statics in `gpu/vram.rs`.

Open, from this work:

- No new setting or per-component option: the release delay is fixed at 120
  pumps, the texture format and quality follow the device, and no component
  can ask for its own. The project settings that apply (`streaming`:
  `max_texture_size`, `coarse_texture_size`, with 0 for full only, and
  `texture_vram_budget_mb`) are the engine's.
- `resolve_slot_material` (meshes, for drawing outside a world's instances)
  can take texture slots in a world's store without a reference. If the same
  file was also referenced in that world and then fell out of use, the slot is
  given back under it.
- A texture wanted by a second use only after it loaded is encoded for it by
  whoever composes or binds, which for a component is the frame thread.
- A material composes again for each need that sharpens: a graph of five
  textures rebinds its users up to five more times while loading.
- No measurement yet of load time or video memory with coarse-first
  materials.
- `.agents/MEMORY.md`, `LEVEL_LOADING.md` and `SCENEDB_MIGRATION.md` are
  updated; scene ledger rows were not added (the gate passes without them).
