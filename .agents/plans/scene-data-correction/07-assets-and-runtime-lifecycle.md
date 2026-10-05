# Contract proposal: assets and runtime lifecycle

Status: **proposed — review required**

## Proposed decision

An authored component stores a typed stable asset reference and authored settings. An asset subsystem owns imported source, immutable CPU data, GPU allocation and content version. A renderer pass reads a reflected handle/generation; it does not ask the editor to create a parallel draw row. Loading and simulation are real work, but their completion commits typed state through SceneDB's normal lifecycle.

## Asset reference and readiness

Use a stable `AssetId` plus optional import settings and expected resource kind in component state. Resolve it through the asset registry to a runtime resource handle with generation/content version. Keep source URI/path, imported data and GPU allocation in their respective owners; do not copy full vertex buffers into every component instance.

Represent unresolved, loading, ready and failed status explicitly in asset/resource state. The renderer's behavior for each status is defined (for example skip draw while loading and report a diagnostic). A component cannot report a successful visible mesh merely because its file path deserialized.

## Async completion safety

An async request captures scene/world identity, stable component ID, expected schema, asset ID, request token and content version. Completion re-resolves the component, checks it still references the same asset/request and only then commits the resource handle/status. Removal, replacement, world swap, undo and newer requests invalidate older completions. Cancellation is advisory; generation checks are authoritative.

Asset reload updates shared immutable resource generations, invalidates dependent GPU bindings/draw outputs, and notifies each observer independently. Component property edits unrelated to asset references do not trigger disk load, geometry rebuild or whole-component replacement.

## Scene lifecycle and frame integration

Editor, standalone runtime, embedded runtime and play-in-editor use the same SceneDB write and GPU-consumption path. The scene lifecycle owns world creation, typed hydration, asset resolution, mirror attachment, GPU upload, render graph setup, world replacement and teardown. UI panels do not own renderer synchronization. Runtime behavior hooks handle genuine component lifecycle/simulation work only; a generic component dispatcher is not a render bridge.

Frame readiness includes pending typed commits, GPU mirror writes, asset completions, simulation work and render-resource recreation. CPU scene revision alone cannot decide that all work is idle. A renderer can be idle only when its required upload and GPU work are settled or safely scheduled for a later frame.

## Resource lifetime

Scene components own references, not raw backend pointers. Asset/GPU resources have explicit owners, generations, reference lifetimes, device-reset recovery and budget/eviction rules. A history snapshot may retain an asset reference while the component is removed. Device loss rebuilds allocations from asset data without rehydrating component JSON or rescanning the entire scene into a secondary representation.

## REVIEW: decisions to confirm or edit

- Which asset state is authored in the component and which is derived by the asset service?
- Are unresolved assets allowed in saved scenes, and what should the viewport display for them?
- What retention guarantees apply to resources referenced only by undo history?
- Which thread owns asset completion commits and SceneDB GPU upload scheduling?
- What is the editor's required response to stale/cancelled asset completions and import failure?
