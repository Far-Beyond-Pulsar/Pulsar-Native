# PMSH bounds and precise viewport picking (issue #705)

## Goal
Give every native static-mesh asset one validated, authoritative local-space bound computed from its final imported geometry. Carry that bound through residency, SceneDB, render derivations, authored LOD and editor selection. Make viewport selection hit actual visible mesh geometry rather than an approximate box. Preserve old assets and geometry content identity.

## Current state
- PMSH v1-v4 have geometry, content identity, material metadata and optional virtual geometry, but no bounds.
- `load_mesh_asset_data` rescans positions to derive `StaticMeshComponent.bounds_local` (a sphere).
- Renderer culling consumes this sphere-derived component data. CPU picking scans scene objects, turns sphere bounds into AABBs, and accepts any AABB hit without checking triangles.
- No source-level change should duplicate scene ownership; SceneDB remains authoritative.

## Work plan
1. **Asset contract and migration**
   - Define a finite, ordered `MeshLocalBounds` AABB and explicit empty-geometry representation.
   - Compute bounds from final `MeshAssetUpload.geometry` after import transforms, including split/import/reimport and generated-mesh writers.
   - Add PMSH v5 bounds serialization without changing vertex/index content-id semantics or v4 VG preservation.
   - Decode v1-v4 unchanged and derive bounds once as a legacy fallback. Reject malformed lengths, non-finite/inverted bounds, and overflow safely. Keep v1-v4 read support indefinitely unless a separate migration policy is approved.
2. **Runtime and render propagation**
   - Keep the canonical AABB in loaded mesh residency; derive the existing GPU culling sphere from it for SceneDB's static-mesh component/render-row contract.
   - Ensure component writes, reloads, authored LOD selection, and GPU mirror schema preserve the derived sphere. Where CPU AABBs are needed, transform them conservatively with the absolute linear transform; handle rotated, nonuniform and negative scale and parent-resolved transforms.
   - Audit culling/occlusion and all instance-bound consumers for independent guessed radii or fallback assumptions.
3. **Viewport hit testing**
   - Keep bounds as a broad-phase rejection only.
   - Build/reuse a triangle BVH per mesh content id during background asset loading, invalidate on content change, and perform ray/triangle tests in mesh-local space after broad-phase. Handle multiple enabled mesh attachments and select the nearest positive world-space hit under nonuniform/negative scale.
   - Preserve scene visibility/locking semantics, selection tie-breaking, camera-relative precision, and gizmo priority. Avoid synchronous asset IO or BVH construction on click; use resident CPU geometry and explicit no-geometry behavior.
   - Check GPUI click coordinate normalization/clamping and make out-of-viewport input a no-op.
4. **Acceptance and documentation**
   - Add format round-trip and malformed-input coverage; legacy v1-v4 decode coverage; transformed bounds cases; BVH/ray hit cases (concave mesh, misses inside AABB, nearest overlapping surfaces); and a scene-level viewport selection regression.
   - Update `.agents/CRATES.md`, `.agents/SCENEDB_MIGRATION.md`, and any mesh-format docs with the final contract and ownership path.

## Delivery sequence
A. Implement and review the PMSH v5 schema plus legacy decode, bounds validation, and import coverage.
B. Plumb exact bounds into component/GPU/render consumers and audit the live culling path.
C. Add content-keyed BVH hit testing and coordinate/selection regression coverage.
D. Run focused crate tests/checks and the repository-required architecture checks; review full diff for submodule, content-id, and import-option regressions.

## Acceptance criteria
- A freshly imported v5 asset contains exact bounds from its final vertex payload; hydration does not scan its vertices to recover bounds.
- Existing v1-v4 assets remain readable and receive correct transient bounds without identity changes or loss of embedded VG/material metadata.
- All rendering and picking consumers agree on bounds under rotation and arbitrary finite scale.
- Clicking a mesh selects only when the ray intersects its triangles (subject to an explicitly documented editor tolerance); a broad-phase bounds hit alone never selects it.
- No stale BVH survives mesh content replacement, no per-click disk load occurs, and large scenes remain responsive.

## Implementation status
- Implemented PMSH v5 local AABBs with v1-v4 transient-bound migration, vgeo/material preservation, and geometry identity unchanged.
- Implemented bounds-based culling-sphere hydration and exact resident-geometry viewport picking with a bounded, prewarmed BVH cache.
- Added viewport click-bound validation and regressions for format migration, contradictory bounds flags, exact triangle hits, transformed meshes, and viewport misses.
- `rustfmt --check` and `git diff --check` pass for the touched files. Focused Cargo tests are currently blocked by an unrelated compile error in the concurrently modified `crates/ui/wgpui-component` submodule (`Node::ListItem` missing from an exhaustive match).
