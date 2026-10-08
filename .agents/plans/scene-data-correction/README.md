# Scene data correction: contract review packet

Status: **proposals for review**. None of the proposed contracts is approved or implemented. Edit these files directly; when ready, return the edited packet and we can reconcile the open decisions before implementation.

This packet turns the confirmed SceneDB/rendering failures into a set of architecture contracts. It follows the stated direction: SceneDB owns live scene state, reflected typed values are written directly, GPU reflection follows the database write lifecycle, render passes read reflected GPU data, and state notifications fan out to subscribers without acting as the source of truth.

## Documents

| File | Decision area |
|---|---|
| [00-initial-corrective-plan.md](00-initial-corrective-plan.md) | Original scope, evidence, sequence and acceptance criteria |
| [01-authority-and-identity.md](01-authority-and-identity.md) | Scene object and component instance ownership, multiplicity, IDs |
| [02-typed-reflection-and-writes.md](02-typed-reflection-and-writes.md) | Type-erased values, factories, property edits, mutation hooks |
| [03-gpu-reflection-and-rendering.md](03-gpu-reflection-and-rendering.md) | GPU layouts, renderer reads, joins, resources, pass boundaries |
| [04-state-notifications.md](04-state-notifications.md) | Independent subscriber delivery and separation from gameplay events |
| [05-persistence-and-undo.md](05-persistence-and-undo.md) | Non-JSON internal state, file archives, migration, undo/redo |
| [06-modules-and-plugins.md](06-modules-and-plugins.md) | Feature ownership, registration, plugin and subsystem boundaries |
| [07-assets-and-runtime-lifecycle.md](07-assets-and-runtime-lifecycle.md) | Asset loading, async completion, render lifecycle parity |
| [08-implementation-and-delegation.md](08-implementation-and-delegation.md) | Dependency order, work package boundaries, definition of done |
| [09-component-structure-and-graph-boundary.md](09-component-structure-and-graph-boundary.md) | Generic component lifecycle, component module template, pass isolation rule |
| [10-phase-0-audit.md](10-phase-0-audit.md) | Primary-reviewed source/dependency inventory and current feasibility gaps |
| [11-phase-0-closure.md](11-phase-0-closure.md) | Phase 0 exit: decisions D1–D4, validation targets, failure baseline |
| [12-phase-1.md](12-phase-1.md) | Phase 1: SceneDB erased writes and mirror replay, intrinsic GPU companions, component-instance entities |
| [13-phase-2.md](13-phase-2.md) | Phase 2: the GPU scene join for meshes and lights; the CPU projection and render subscriptions removed |
| [14-phase-3.md](14-phase-3.md) | Phase 3: typed history, commands, class templates and script producers; explicit load migrations |
| [15-phase-4.md](15-phase-4.md) | Phase 4: render components reach a pass through derived rows and graph-owned joins, or are reported unfinished with a tracking issue (#1053–#1060; limitations #1061–#1066); the queue and behavior dispatch deleted |
| [16-phase-5.md](16-phase-5.md) | Phase 5: panels and scripts read changes through their own cursors (`ComponentWatch`); cursors bound to their World; the forced resync deleted; undo/redo, level replacement and viewports need no repair path |
| [17-phase-6.md](17-phase-6.md) | Phase 6: object subscriptions for views (the panel receives values), SceneDB's shared queue and Pulsar's compatibility residue removed, every ledger row closed, architecture checks, docs as built; acceptance gaps tracked in #1081 |
| [18-phase-7.md](18-phase-7.md) | Phase 7: the #1081 acceptance gaps tested; fixes found by them (read-only `get_mut` upload, `ClassInstance` disable, load errors naming the property, Helio debug lines' camera, mesh hot reload, Helio's SceneDB pin); runtime parity through one shared game renderer |
| [voxel-branch-porting.md](voxel-branch-porting.md) | Notes for rebasing the voxel branches (Pulsar-Native#994, Helio#314) onto the corrected path: what to replace, what to keep, what to decide |
| [ledger.toml](ledger.toml) | Closure ledger checked by `cargo test -p scene_inventory` |

## How to review

Each settling document has a proposed contract, rationale, implications, and a short list of decisions you can change. Search for `REVIEW:` to find explicit questions. You can accept the proposal, edit it, or write a different decision. The proposals are intentionally concrete enough to expose consequences, while keeping unresolved choices visible.

The documents reference one another. If you change component identity, for example, please also review the GPU owner joins, component references, history, and plugin APIs. The implementation plan must be updated after the contracts are reconciled.

## Current proposed spine

The central proposal is one SceneDB entity for each scene object and one SceneDB entity for each attached component instance. The component entity carries its actual registered Rust component value plus typed ownership/order/enabled metadata. This represents duplicate instances without storing live copies in JSON. Stable IDs bridge save/load and long-lived editor/script references; raw ECS entity handles remain world-local.

Each component registration supplies reflection and may declare fields for SceneDB to reflect/upload as GPU columns. SceneDB writes of the real component value update those columns. Components provide data; they do not render, produce pixels, or declare a render capability. The renderer decides whether and how to consume the uploaded buffers. The render-graph crate alone maps component schemas to concrete passes and owns every pass dependency; the engine composes components and the graph API, and passes own their implementation. GPU passes can join the columns to owner transform/visibility data. GPU-generated culling or indirect draw records are transient execution products, not another editable scene representation. No CPU render projection, event drain, or manual refresh is required to make data visible. Every registered component uses the same storage/reflection/write lifecycle; GPU upload is an optional schema declaration, while rendering is owned by the renderer.

In-process editing, class expansion, scripts, and undo use owned typed/reflected values. A versioned archive codec handles persistence. JSON is an optional import/export or external protocol adapter. State notifications are per-subscriber invalidations/change descriptions over committed state; gameplay events remain a separate ordered stream.

These are proposals, not hidden implementation assumptions. In particular, SceneDB's current ECS/GPU capabilities may need upstream changes to support the proposed identity and erased-write model. The implementation must validate feasibility before locking the public API.

The strict pass boundary is not how the current checkout is wired: `engine_backend`, Helio's facade and component crates, asset compatibility, wasm, demo and snapshot/example crates directly depend on concrete `helio-pass-*` crates. The proposed target moves production composition and pass names into one graph-owning crate, currently expected to be `helio-default-graphs` after Phase 0 confirms its scope and resolves the current facade dependency cycle.
