# Contract proposal: modular feature and plugin ownership

Status: **proposed — review required**

## Proposed decision

Each engine feature owns its component schema, reflection metadata, validation/migrations, asset dependencies, optional GPU layout declaration, and domain systems. The render-graph crate alone owns the mapping from component schemas to pass inputs, pass construction/configuration, and pass scheduling. Shared infrastructure owns generic registration, storage, transaction, GPU reflection and resource lifetime. Shared infrastructure does not accumulate a central `match class_name` table.

SceneDB is a general typed data/GPU-reflection layer and does not depend on Helio, editor UI, a particular component crate, or JSON. The engine composition crate registers actual component bundles and starts the render-graph service; it does not name or instantiate pass types. The render-graph crate is the sole crate that depends on concrete pass crates. It binds reflected SceneDB buffers and configures/schedules the graph. Editor UI depends on reflected component descriptors and SceneDB transactions, not pass types or component-specific constructors scattered across panels.

### Confirmed current boundary violation

At the audit baseline, this boundary is violated in multiple layers, not just engine composition:

- `crates/core/engine_backend/Cargo.toml` directly declares optional dependencies on concrete passes including billboard, forward-lit, decal, gbuffer, water-sim, postprocess, voxel-planet, sky and TSR; its `render` feature enables them.
- Helio's `crates/helio/Cargo.toml` directly depends on portal, foliage, postprocess, volumetric fog, TSR, sky, billboard, corona, forward-lit, object-batch, gbuffer, shadow-matrix and radiance-cascade passes.
- `crates/helio-component/Cargo.toml` directly depends on postprocess, volumetric-fog, water-sim, voxel-planet, foliage-place, portal-cull and deferred-light passes. This is especially direct component-to-pass coupling and must be removed.
- `crates/helio-asset-compat`, `crates/helio-wasm`, `crates/helio-web-demos`, `crates_other/helio-snapshot`, and `crates/examples` also declare concrete pass dependencies. Audit whether each should use the graph API or, for pass implementation tests, remain inside its owning pass crate.

The corrective work must move production graph composition/pass dependencies into the graph owner, make component/asset APIs pass-agnostic, and leave engine composition with the graph service API plus component registration bundles. The strict target also removes pass-to-pass dependencies; move genuinely shared pass contracts into neutral crates and let the graph connect pass inputs/outputs. Verify Rust imports and Cargo dependency edges, not just where `RenderGraph::add_pass` appears. A workspace-level dependency alias is not itself a consuming crate; check actual manifest edges and source imports.

The current Helio candidate for the single graph owner is `crates/renderer/helio/crates/helio-default-graphs`: its manifest explicitly depends on the pass crates and its builder imports their concrete types. `helio-default-graphs` already depends on the Helio facade, so Phase 0 must also move any concrete pass composition currently inside that facade out to the graph owner without creating a dependency cycle. Confirm whether all production graphs and application graph variants can be composed there. If a dedicated replacement is needed, make one crate the sole concrete pass dependency hub; do not split concrete pass references across engine backend, component/compatibility crates and multiple graph-builder crates.

## Feature module contract

A feature module may register:

- Stable component schema ID/version and concrete Rust type.
- Factory, reflected fields/methods, validation and typed clone/snapshot policy.
- Optional GPU-upload schema generated from explicitly marked fields; no render capability, data-domain tag, pass name, pass ID, pass type, or graph dependency.
- Asset/resource requirements and typed completion handling.
- File migrations and optional human-readable interchange codecs.
- Domain simulation or gameplay event producers/consumers.
- Tests proving typed storage, GPU layout, lifecycle, and real consumer behavior.

It must not register an ad hoc CPU scene mirror or require a global renderer loop to reparse values. A feature with simulation output separates authored input, transient simulation state and persisted state by ownership and lifetime.

## Plugin lifecycle and isolation

Plugins can use the same registration contract through the supported SDK. Registration is validated before the type can be attached to scenes. A plugin component may mark fields for GPU upload through a supported schema. The renderer graph independently decides whether and how those buffers are consumed. Plugin component code never references concrete passes; plugin pass extensions, if supported, are registered through the render-graph crate's extension API.

The current permanent-DLL policy makes plugin-provided function pointers and concrete type destructors valid for process lifetime. Make this requirement explicit in the API. If unload is ever introduced, it requires proving there are no live values, cursors, callbacks, GPU schemas or archived operations from that module. Never unload by assumption.

Document and test the Rust ABI/toolchain compatibility required by plugins. If plugins must cross compiler or language versions, use a stable C ABI with opaque handles and registered codecs rather than exposing Rust `Any`/trait objects across that boundary. Internal engine crates may use Rust trait objects where versions are controlled.

## Subsystems and dependency direction

- Registration is data driven and modular; optional features can be absent without making core SceneDB depend on them.
- Only the render-graph crate imports concrete pass crates, maps stable component schemas/GPU layouts to pass inputs, and schedules passes. No engine, component, editor, subsystem or plugin SDK crate refers to a pass type or ID.
- Component modules publish data and domain systems; they do not call pass APIs or own pass-specific runtime state.
- Physics/audio/gameplay systems query typed state and write their owned outputs through transactions. Their real simulation loops and domain events remain valid.
- Runtime and editor share component factories, schemas, asset resolution and render inputs. UI-only behaviors stay in editor modules.
- Optional capability absence returns an explicit unsupported/unavailable result, with no successful-looking dormant JSON attachment.

## Quality and dependency rules

Keep compile-time feature dependencies acyclic. Prefer registration bundles/registries generated by each feature and gathered at application startup over hard-coded central component type lists. Detect duplicate stable IDs/layout incompatibility at startup. Ensure component crates do not need editor, engine-composition, render-graph or pass crates; the engine-composition crate references component registration bundles and the render-graph API, never concrete passes; SceneDB does not need Helio; generic reflection does not need JSON for in-process values. The render-graph crate is the only owner of concrete pass dependencies.

Every feature registration includes contract tests. Shared changes to erased-value hooks, stable IDs, GPU layout or archive format require API/version updates and compatibility tests for built-in and plugin components. An extension guide must explain how to register, persist, reflect, render, migrate and test one new component.

## REVIEW: decisions to confirm or edit

- How should the graph report uploaded component data that no installed pass consumes?
- Is the strict dependency boundary clear: concrete pass dependencies and identifiers exist only in the render-graph crate?
- What plugin ABI/toolchain guarantees are required by the supported SDK?
- Should optional feature registration happen through static inventory, explicit startup bundles, or plugin registration calls?
- Which module owns cross-cutting resource handles and archive type registries?
- What compatibility window must third-party component schemas receive?
