# Noise Generation Builtins for Shaders — Design Spec

**Issue:** [Far-Beyond-Pulsar/Pulsar-Native#81](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/81)
**Date:** 2026-07-16
**Status:** Draft — pending review

## Goal

Add a complete, industry-standard family of procedural noise nodes to the shader
graph: white, value, Perlin, simplex, and Voronoi base generators plus fBm,
turbulence, and ridged fractal combinators, each in 2D and 3D — 16 nodes in a
new "Noise" category. All deterministic, hash-based, texture-free WGSL.

## Background — how shader builtins work today

Registration chain (verified against source):

1. **wgsl_std** (`psgc/crates/wgsl_std/src/shader/nodes/*.rs`): each builtin is a
   `#[distributed_slice(SHADER_REGISTRY)]` function returning a
   `graphy::core::NodeMetadata` — name, category, params, return type, and a
   WGSL **expression template** (`.with_source("a + b")`).
2. **Graphy** (`graphy/src/core/metadata.rs`): defines `NodeMetadata`, which
   already carries serde-defaulted "v2" extension fields — the pattern for
   backward-compatible additions.
3. **PSGC codegen** (`psgc/crates/psgc/src/codegen/wgsl_codegen.rs`):
   `generate_entry_function` emits a `Uniforms` struct + binding, the
   `@vertex`/`@fragment` signature, then one `let <var> = <expanded expr>;` per
   reachable pure node in dependency order (`expand_function_source`
   substitutes parameter names with argument expressions).
4. **shader_editor** (`plugins/vendor/shader_editor/src/core/definitions.rs`):
   builds its palette from `psgc::metadata::get_shader_nodes()`; categories are
   auto-discovered, with per-category icon/color match arms.

**The gap:** codegen supports only single-expression nodes. Noise requires real
WGSL helper functions (hashes, gradient evaluation, loops). There is no
mechanism to emit module-scope functions into the generated shader.

**Dependency wiring:** psgc pins graphy by git rev
(`psgc/crates/psgc/Cargo.toml`); the Pulsar-Native root workspace `[patch]`es
both `graphy` and `psgc`/`wgsl_std` to the local submodules, so all three
components build together locally before any PR merges. Publish order still
matters: Graphy → PSGC (rev bump) → Pulsar-Native (submodule bumps).

## Design

### 1. Graphy: helper functions on `NodeMetadata`

New field, following the existing v2 extension pattern:

```rust
/// Named module-scope helper function definitions required by
/// `function_source`, in (name, source) pairs. Emitted once per generated
/// module (deduplicated by name across nodes), in first-use order.
/// Helpers may reference earlier-listed helpers of the same node or any
/// helper emitted before them.
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub helper_functions: Vec<(String, String)>,
```

Builder: `.with_helpers(&[("pn_hash22", "fn pn_hash22(…) -> … { … }"), …])`.
Serde-defaulted → old serialized metadata deserializes unchanged; blueprint
(non-shader) codegen ignores the field entirely.

### 2. PSGC codegen: helper emission

In `generate_entry_function`, after the `Uniforms` block and before the entry
function signature:

- Walk the same `reachable` set already computed for `let`-binding emission,
  in the same `get_pure_evaluation_order()` order (deterministic output).
- For each node's `helper_functions`, append each `(name, source)` to the
  module if the name has not been emitted yet (`HashSet<String>` seen-guard,
  `Vec` for order).
- Emit collected helpers at module scope between the uniforms binding and the
  entry function.

Within one node, helpers are listed dependency-first (e.g. `pn_hash22` before
`pn_perlin_2d`); across nodes, dedup-by-name makes shared hashes emit once.
No cross-node topological sort is needed: WGSL permits forward references to
module-scope functions, so emission order is a readability nicety, not a
correctness requirement.

**Namespacing:** all helper names carry a `pn_` prefix (pulsar noise) to avoid
collisions with user-visible WGSL builtins and future nodes. Codegen does not
enforce the prefix; uniqueness-by-name is the contract.

### 3. wgsl_std: `shader/nodes/noise.rs` — the node library

Category `"Noise"`. All nodes `NodeTypes::pure`. Signatures:

| Node | Params | Returns | Notes |
|---|---|---|---|
| `white_noise_2d` | `p: vec2<f32>, scale: f32, seed: f32` | `f32` | hash of floor(p·scale), [0,1] |
| `white_noise_3d` | `p: vec3<f32>, scale: f32, seed: f32` | `f32` | |
| `value_noise_2d` | same as white | `f32` | smoothstep-interpolated lattice, [0,1] |
| `value_noise_3d` | | `f32` | |
| `perlin_2d` | same | `f32` | gradient noise, remapped to [0,1] |
| `perlin_3d` | | `f32` | |
| `simplex_2d` | same | `f32` | 2D simplex, remapped to [0,1] |
| `simplex_3d` | | `f32` | 3D simplex, remapped to [0,1] |
| `voronoi_2d` | same | `vec3<f32>` | x=F1, y=F2, z=cell random [0,1] |
| `voronoi_3d` | | `vec3<f32>` | |
| `fbm_2d` / `fbm_3d` | `p, scale, seed, octaves: f32, lacunarity: f32, gain: f32` | `f32` | Perlin-based, [0,1] |
| `turbulence_2d` / `_3d` | same as fbm | `f32` | sum of |signed perlin| |
| `ridged_2d` / `_3d` | same as fbm | `f32` | (1−|n|)² multifractal |

Conventions:

- **Output range:** every scalar output is normalized to [0,1] (artist-friendly,
  Blender-style); Voronoi F1/F2 are raw Euclidean distances (≈[0, 1.5]),
  documented per node. Signed variants are not separate nodes — `n*2-1` is one
  existing math node away.
- **`seed`** is a domain offset (`p + seed·<large irrational constant vector>`),
  cheap and artifact-free.
- **`octaves`** is `f32` for pin-type uniformity; helpers do
  `i32(clamp(octaves, 1.0, 10.0))` and loop dynamically (WGSL allows
  non-constant loop bounds).
- **Hashes:** shared PCG-style integer hashes (`pn_pcg2d`, `pn_pcg3d` after
  Jarzynski & Olano) with float wrappers `pn_hash21/22/31/33`. No `sin()`-based
  hashes (platform-dependent precision breakup).
- **Algorithms:** standard public-domain constructions (lattice value noise,
  Perlin with quintic fade, Gustavson-style simplex, 3×3/3×3×3 jittered-cell
  Voronoi), written fresh for WGSL — no license-encumbered code copied in.
- **Unconnected-input defaults**: `ParamInfo` is bare name+type (no defaults;
  verified), so unconnected pins get `default_value_for_type` → `0.0`, same as
  `multiply.b` today. The richer v2 `param_metas` (`ParamMeta.default_value`)
  does support defaults — populate it with `scale=1, octaves=4, lacunarity=2,
  gain=0.5` **only if** codegen's `DataSource::Default` path can be routed
  through it without touching unrelated blueprint behavior; otherwise ship
  zero-defaults and document them in each node's description string. Decide at
  implementation time; either outcome is acceptable for v1.

### 4. shader_editor: category polish

Two match-arm additions in `definitions.rs`: icon `"▒"` and color `"#8E44AD"`
for `"Noise"`. Everything else (palette, pins, vec3 outputs) auto-discovers;
`PinDataType::from_type_str` already handles `vec3<f32>`.

## Execution strategy

Work happens in the existing `issue-81-noise-shader-builtins` worktree (already
at main tip with submodules synced). The root `[patch]` table means all three
submodule changes build and test together locally from day one.

Order (TDD throughout — every step lands red tests before implementation):

1. **Graphy** (`crates/third-party/graphy`, branch `feat/node-helper-functions`):
   `helper_functions` field + builder + serde round-trip tests (old-format JSON
   without the field must deserialize).
2. **PSGC codegen** (`crates/third-party/psgc`, branch `feat/81-noise-builtins`):
   helper-emission in `wgsl_codegen.rs`. Tests: single node with helpers emits
   them at module scope; two nodes sharing a helper name emit it once;
   helper-free graphs generate byte-identical output to today (regression
   guard).
3. **wgsl_std noise library** (same PSGC branch): `noise.rs` with hash helpers
   first (unit-testable registration), then base noises, then fractals.
   Per-family shader_tests (see Verification).
4. **shader_editor** (`plugins/vendor/shader_editor`, branch
   `feat/81-noise-category`): icon/color arms.
5. **Superproject**: submodule bumps + this spec, branch
   `feat/81-noise-shader-builtins`.

Publish sequence: Graphy PR merges → psgc bumps its graphy git rev → PSGC PR
merges → Pulsar-Native PR bumps both submodules (+ shader_editor). Each PR
body links the chain. Nothing merges out of order without breaking the
git-rev pin, and the local patch topology keeps development unblocked
regardless of merge timing.

## Verification

1. **Graphy unit tests:** serde backward-compat (missing field → empty vec);
   builder round-trip.
2. **PSGC codegen unit tests:** helper emission, dedup-by-name,
   deterministic ordering, no-helpers regression (existing math shader output
   unchanged).
3. **shader_tests integration tests:** one graph per noise family
   (noise → rgba → fragment_output), asserting compilation succeeds and the
   generated WGSL is **validated by naga** (a *new, test-only* dev-dependency
   of `shader_tests` — naga is not currently in psgc's dependency tree, and
   this adds zero runtime weight). Parse + validate the module, not just
   string-matching: this catches WGSL syntax/type errors in every helper on
   every test run, without a GPU. The editor's preview renderer compiles
   shaders through the same `compile_fragment_shader` path
   (`shader_editor/src/features/compilation/conversion.rs`), so naga-clean
   output is exactly what the preview consumes.
4. **Registration test:** `get_shader_nodes()` contains all 16 noise nodes with
   category "Noise" and the expected pin counts/types.
5. **Editor smoke test (manual, with user):** run the editor from the worktree,
   create a shader, verify the Noise category appears, place `perlin_2d` →
   preview renders non-uniform grayscale; place `fbm_2d` and confirm octave
   parameter changes the look; confirm no console errors. (The preview
   renderer compiles the same generated WGSL, so naga-validated code is
   expected to render; the manual pass verifies visual plausibility, which
   automated tests cannot.)
6. **Order-independence check (automated):** helper emission must be correct
   regardless of node evaluation order — the emitted helper *set* for a fixed
   graph is exactly the union of reachable nodes' helpers with no duplicates,
   and generated WGSL validates whichever order helpers appear in (WGSL
   permits forward references). Note: cross-process codegen ordering is
   already nondeterministic today (`GraphDescription.nodes` is a std
   `HashMap`, and the topo-sort seeds from map iteration) — that is
   pre-existing behavior this feature must tolerate, not fix.

## Risks & mitigations

- **Helper name collisions with future nodes** → `pn_` prefix convention +
  dedup-by-name means a genuine collision silently drops the second body;
  mitigate with a codegen `debug_assert`/warning when two different sources
  claim the same name.
- **Simplex 3D correctness** (easiest to get subtly wrong) → naga validation
  catches type errors; visual smoke test catches artifacts; implementation
  follows the well-established Gustavson construction.
- **Loop cost in fractals** → octaves clamped to 10; documented.
- **Serde/ABI drift for plugins** (shader_editor consumes NodeMetadata across
  the plugin boundary via psgc, compiled in-tree) → field is additive and
  serde-defaulted; all consumers rebuild together in-tree.
- **PR sequencing mistakes** → git-rev pin makes out-of-order merge a compile
  error in CI rather than silent breakage.

## Audit notes (claims verified against source, 2026-07-16)

- `expand_function_source` (`wgsl_codegen.rs:258`) lexes full identifiers and
  replaces only exact param-name matches — helper-calling templates like
  `pn_perlin_2d(p, scale, seed)` cannot be corrupted by short param names. ✅
- ~~`vec3_split` exists~~ **Corrected on closer read:** `vec3_split`
  (`vector.rs:107`) is a pass-through (`source: "v"`, single vec3 result) —
  wgsl_std has **no** real component extraction. The plan therefore adds three
  one-expression nodes `vec3_x`, `vec3_y`, `vec3_z` (Vector category,
  `source: "v.x"` etc.) so the Voronoi vec3 output is actually consumable. ⚠️→✅
- The editor preview compiles through the same `compile_fragment_shader`
  codegen path (`shader_editor/src/features/compilation/conversion.rs`). ✅
- `SHADER_REGISTRY` is a linkme distributed slice (`wgsl_std/src/lib.rs:54`);
  a new `noise.rs` module just needs `mod noise;` in `nodes/mod.rs`. ✅
- No existing noise/perlin/simplex/voronoi identifiers anywhere in wgsl_std or
  shader_editor — no name collisions. ✅
- `ParamInfo` has no default-value support (name + type only); `ParamMeta`
  (v2) does — reflected in the defaults decision above. ✅
- naga absent from psgc's lockfile — new test-only dev-dependency. ✅
- Codegen node ordering is nondeterministic across processes today (std
  `HashMap` + map-iteration topo seed) — helper emission is designed
  order-independent; reflected in Verification #6. ✅

## Out of scope

- Signed-output node variants (use existing math nodes).
- 4D noise, curl noise, blue noise (follow-ups if wanted).
- Multi-output pins beyond the vec3 Voronoi convention.
- Property-panel UI for per-param defaults (depends on ParamInfo capabilities;
  noted above).
