# Radiant 2.0: A Template-Granular Material Pipeline with Per-Pixel Dispatch

**Author:** Tristan J. Poland
**Date:** July 2026
**Category:** Real-Time Rendering, Material Systems, GPU-Driven Pipelines

---

## 1. Abstract

We present **Radiant 2.0**, a material pipeline architecture for real-time rendering that decouples the cost of material dispatch from both the number of materials in a scene and the number of render passes in the pipeline. The system introduces three novel mechanisms: **Evaluation Points** — named injection sites in render passes where material-specific shader code is composed; **Templates** — pre-authored shading archetypes that serve as the atomic unit of GPU dispatch; and **Two-Tier Dispatch** — a design where the common case (material variants of existing templates) pays zero per-pixel overhead while the exceptional case (custom multi-pass materials) pays bounded, measurable cost.

A fundamental insight of this work is that the per-pixel dispatch check — `let mask = template_dispatches[template_id].eval_point_mask; if (mask & BIT) == 0 { discard; }` — has an invariance property: its GPU instruction cost is identical whether the dispatch table contains 1 entry or 10,000. This makes the system effectively free for existing assets and arbitrarily extensible for future ones. We provide formal analysis of the invariance property, a GPU-side material visibility hierarchy that eliminates compute-pass divergence, and a migration path from traditional per-pass material registries.

---

## 2. Introduction

Modern real-time engines confront a tension between material variety and rendering efficiency. Every distinct shading model — skin, hair, fabric, glass, water — requires a unique shader, and every unique shader multiplies the PSO (pipeline state object) compilation space. In current AAA engines, this is managed through material batching: geometry is sorted by shader, then by material parameters, and draws are issued in batches. The CPU cost scales with the number of batches, and the GPU cost scales with nothing — the GPU simply executes what it is told.

This approach works well when the number of material archetypes is small (20–30) and the number of material instances is moderate (hundreds). It degrades when either condition is violated. A fully dynamic editor-driven pipeline, where artists create thousands of material instances by adjusting parameters, and where third-party render passes introduce new shading domains, demands an architecture where dispatch cost does not depend on material count.

Radiant 2.0 addresses this by moving dispatch decisions from the CPU (per-material PSO selection) to the GPU (per-pixel template classification). The key claim is that this can be done with zero regression for existing assets and bounded, predictable cost for new ones.

---

## 3. System Architecture

### 3.1 The Three-Layer Model

Radiant 2.0 organizes material rendering into three layers, each with a distinct ownership and cost profile:

```
Layer 1 — Evaluation Points
  Owned by: Render passes
  What they are: Named injection sites in pass shaders where material-
    specific WGSL is composed. Each point defines a function signature,
    a base shader, a blend mode, depth state, and a threading model.
  Examples: "gbuffer", "transparent", "ssr", "shadow", "depth_prepass"
  Cost: Compiled once per pass. No per-pixel or per-material cost.

Layer 2 — Templates
  Owned by: The engine (built-in) or the user (custom)
  What they are: Complete shading archetypes. Each template declares
    which evaluation points it targets, provides the WGSL functions for
    each target, and defines a parameter schema.
  Examples: pbr, skin, hair, glass, water, eye, fabric
  Cost: Compiled once per template per targeted eval point. Shared by
    all material instances of this type. The dispatch table has one
    4-byte entry per template, regardless of instance count.

Layer 3 — Materials
  Owned by: The scene
  What they are: Template instances with concrete parameter values and
    texture references. A material is `template_id + params + textures`.
    No shader code, no evaluation point decisions, no compilation.
  Cost: Exactly zero at the pipeline level. Instance data carries the
    template_id (4 bytes, replaces previous padding in GpuInstanceData).
```

The critical property: **cost is bounded at Layer 2**. Adding 10,000 materials that all use the same template adds zero to the dispatch table, zero to the shader variant cache, and zero to the per-pixel instruction stream. Adding one new template adds one 4-byte dispatch table entry and one compiled variant per targeted eval point — invariant with the number of materials that use it.

### 3.2 Evaluation Points

An evaluation point is the atomic unit of material-pass integration. Every render pass that evaluates materials declares one or more evaluation points at graph construction time:

```rust
pub struct EvalPoint {
    pub name: &'static str,           // e.g. "gbuffer", "transparent"
    pub index: u32,                   // assigned at registration time
    pub entry_fn_name: &'static str,  // e.g. "eval_gbuffer"
    pub base_shader: &'static str,    // WGSL with override markers
    pub blend_mode: BlendMode,        // Opaque, AlphaBlend, Add, ...
    pub depth_state: DepthStencilState,
    pub threading: ThreadingModel,    // Fragment or Compute
}
```

Built-in evaluation points are:

| Name | Pass | Entry Function | Threading |
|---|---|---|---|
| gbuffer | GBufferPass | `eval_gbuffer(material_id, world_pos, N, uv) -> SurfaceData` | Fragment |
| transparent | TransparentPass | `eval_transparent(material_id, world_pos, N, uv) -> vec4f` | Fragment |
| shadow | ShadowPass | `eval_shadow(material_id, world_pos) -> bool` | Fragment |
| depth_prepass | DepthPrepass | `eval_depth(world_pos) -> f32` | Fragment |
| velocity | VelocityPass | `eval_velocity(world_pos_prev) -> vec2f` | Fragment |
| ssr | SsrPass | `eval_ssr(material_id, world_pos, N, roughness, f0) -> SsrParams` | Compute |
| sss | SssBlurPass | `eval_sss(material_id) -> SssParams` | Compute |
| decal | DecalPass | `eval_decal(material_id, uv) -> DecalOutput` | Compute |
| post_process | PostProcessPass | `eval_post(input) -> vec4f` | Compute |

A pass declares its points by implementing `RenderPass::register_eval_points()`. The graph builder collects all points after `graph.lock()` into a global `EvalPointRegistry`. The registry is frozen after collection — indices are stable for the lifetime of the graph.

---

## 4. Shader Composition

For each (template, eval_point) pair, the system composes a complete shader by extracting the template's eval function and splicing it into the eval point's base shader. The base shader contains:

1. **Bindings**: All `@group(n) @binding(m)` declarations the pass provides (camera, globals, instances, materials, textures)
2. **Vertex/entry point**: Either a `@vertex` + `@fragment` pair or a `@compute` entry
3. **Override marker**: `// RADIANT_OVERRIDE_START` / `// RADIANT_OVERRIDE_END` surrounding the line that calls the eval function
4. **Default eval function**: A simple fallback (e.g., plain PBR for gbuffer, 30% alpha gray for transparent)

The composition algorithm locates the eval function declaration in the base shader by matching the function name, finds its body via brace-depth tracking, and replaces it with the template's implementation. Everything outside the function body — bindings, vertex shader, fixed-function state — is preserved verbatim.

A single template WGSL file may define multiple eval functions (e.g., glass provides both `eval_gbuffer` for normals/roughness and `eval_transparent` for alpha blending). The composition extracts only the function matching the current eval point.

---

## 5. Two-Tier Dispatch

### 5.1 Tier 1 — Fast Path

The fast path handles the common case: materials that are parameter variants of a built-in template. These materials never define custom WGSL, never register new evaluation points, and never extend the dispatch table.

```rust
// Tier 1: 10,000 materials, all sharing one PBR template
for each material:
    let instance = GpuInstanceData {
        template_id: PBR_TEMPLATE_ID,  // 0 — same for all
        // ... transform, mesh_id, params ...
    };
```

**Cost analysis (Tier 1):**

| Context | Operation | Cost |
|---|---|---|
| Gbuffer pass (fragment) | `if (mask & GBUFFER_BIT) == 0 { discard }` | 0 cycles — never-taken predicated branch. Bit 0 is always set for Tier 1 templates. The condition is compile-time constant false. |
| Transparent pass (fragment) | Same check for TRANSPARENT_BIT | 0 cycles — early-Z kills the fragment before the shader runs. Opaque geometry already wrote depth; the transparent pass tests against read-only depth and fails immediately for opaque pixels. |
| Compute passes (SSR, SSS, ...) | Same check per pixel | 0 cycles for the coarse check — the material visibility hierarchy (Section 6) culls the entire tile. The per-pixel check within the tile never runs. |

A Tier 1 material costs exactly nothing beyond the instance data it already needs. This is the same cost profile as a traditional batched renderer — the GPU draws what it's told, with no per-pixel overhead.

### 5.2 Tier 2 — Custom Path

The custom path is available for materials that need a different shading model or multi-pass evaluation. The user registers a new template with explicit eval point targets:

```rust
let custom_template = engine.register_template(Template {
    name: "custom_iridescent",
    targets: &["gbuffer", "ssr"],      // this template writes to both
    eval_sources: Map {
        "gbuffer" => custom_gbuffer_wgsl,
        "ssr" => custom_ssr_wgsl,
    },
});
```

**Cost analysis (Tier 2):**

| Resource | Cost | Scaling |
|---|---|---|
| Dispatch table | +1 entry (4 bytes) | Total entries = builtin_templates + custom_templates |
| Shader variant | +1 per (template, eval_point) | Compiled once, shared by all instances of this template |
| Per-pixel check (targeted passes only) | 1 indexed load + 1 bit test | Same cost for 1 or 10,000 instances |
| Per-pixel check (non-targeted passes) | 0 (early-Z or hierarchy cull) | Same as Tier 1 |

A Tier 2 material pays exactly for what it uses. If it targets only `"gbuffer"` (same as Tier 1), the cost is identical to Tier 1. If it targets `["gbuffer", "transparent", "ssr"]`, it pays the check in three passes — all other passes cost zero.

---

## 6. Material Visibility Hierarchy

For compute passes, where early-Z is unavailable and thread divergence within a workgroup is expensive, a **material visibility mip chain** is constructed from the per-pixel template ID buffer.

### 6.1 Construction

The gbuffer pass writes a full-resolution `R32Uint` texture storing `template_id` per pixel. One subsequent compute dispatch builds a pyramid:

```
Level 0 (native res):   template_id per pixel
Level 1 (½ res):        eval_point_mask bitwise OR of each 2×2 block
Level 2 (¼ res):        OR of each 4×4 block
Level N (coarsest):     OR of the entire active region
```

Each texel at level L stores the union of all `eval_point_mask` values in its corresponding region. If any pixel in a 16×16 tile targets SSR, that tile's coarse texel has `SSR_BIT` set.

Cost: one compute dispatch, ~0.05 ms at 1080p (same hardware path as Hi-Z depth pyramid construction).

### 6.2 Usage

Before evaluating per-pixel work, a compute pass checks the coarse level covering its tile:

```wgsl
@compute @workgroup_size(16, 16, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let tile = id.xy >> 4;
    let coarse = textureLoad(mat_visibility_mip, tile, COARSE_LEVEL).r;
    if (coarse & (1u << SSR_BIT)) == 0u { return; }  // whole group exits

    let tid = textureLoad(template_id_tex, id.xy, 0).r;
    let mask = template_dispatches[tid].eval_point_mask;
    if (mask & (1u << SSR_BIT)) == 0u { return; }     // per-pixel fallback

    // ... SSR evaluation ...
}
```

The hierarchy **cannot be worse** than per-pixel dispatch alone. At tile boundaries where materials mix, it falls through to the per-pixel check — same cost. On large uniform regions (most of the screen), it eliminates the check for the entire workgroup in a single instruction.

---

## 7. The Invariance Property

The per-pixel dispatch check is:

```wgsl
let mask = template_dispatches[input.template_id].eval_point_mask;
if (mask & (1u << THIS_BIT)) == 0u { discard; }
```

This instruction sequence has a critical property: **its cost is invariant with the size of the dispatch table**. The instruction stream is identical whether the table has 1 entry or 10,000. The only variable is `template_id` — a 4-byte value in the instance data that indexes into the table. The index operation is a single scalar load:

1. `template_id` is in the register file (flat-interpolated from VertexOutput)
2. `template_dispatches` base address is in a constant buffer
3. The effective address is `base + template_id * 4` — one fused add+load
4. The bit test is one `S_AND_B32` instruction (AMD) or `LaneAnd` (NVIDIA)
5. The conditional discard is one predicated branch

The entire sequence is 2–3 GPU instructions. At 10,000 templates, the dispatch table is 40 KB — well within the L1 data cache of any modern GPU (RDNA3: 32 KB per CU with 60 CUs; Ada: 128 KB per SM with 72 SMs). The buffer is read-only and accessed coherently: every thread in a warp reads from the same table, so the cache line serving the table is pinned in L1 for the duration of the dispatch.

Formally:

$$C_{\text{pixel}} = L_{\text{indexed}} + A_{\text{bit}} + B_{\text{pred}}$$

Where $L$ is the latency of a cached indexed load (4–8 cycles), $A$ is the latency of a bit test (1 cycle), and $B$ is the branch penalty (0 cycles for correctly predicted branches — always-taken and never-taken branches incur no penalty on modern GPUs).

All three terms are independent of $N$, the number of templates. Therefore:

$$\frac{\partial C_{\text{pixel}}}{\partial N} = 0$$

The marginal cost of adding a template is zero at the per-pixel level. The only system-wide cost is +4 bytes of GPU memory for the dispatch table entry.

---

## 8. Template Library

The engine ships a library of approximately 30 material templates. These are not "variations of PBR" — each is a distinct shading archetype with the BRDF, lighting model, and pipeline integration appropriate to the surface it represents:

| Template | Targets | Shading Model | Distinct Features |
|---|---|---|---|
| pbr | gbuffer | Cook-Torrance GGX | Metallic workflow, IBL |
| skin | gbuffer, sss | d-Lobe/s-Lobe + SSS | Dual-lobe specular, subsurface |
| hair | gbuffer | d-Box anisotropic | Specular streaks, directional AO |
| fabric | gbuffer | Microfiber + sheen | Fuzz normal, cloth D/G |
| clear_coat | gbuffer | Dual-layer GGX | Base + clear coat, thickness |
| glass | gbuffer, transparent | Fresnel + transmission | IOR, thin-film, tint |
| water | transparent, ssr | Gerstner + foam | Animated waves, caustics |
| eye | gbuffer | Cornea/iris/sclera | Three-region shading |
| velvet | gbuffer | Inverted Gaussian | Back-scatter highlight |
| iridescent | gbuffer | Thin-film interference | Wavelength-shifting F0 |
| subsurface | gbuffer, sss | Lambertian transmission | Thickness absorption |

Each template is a production-grade WGSL file (200–500 lines). Once authored, it is shared by every material instance of that type in every project. Users extending the engine register new templates via the same API — they automatically receive a dispatch table entry, compiled variants for each targeted eval point, and full visibility hierarchy integration.

---

## 9. Performance Characteristics

### 9.1 CPU Cost

| Operation | Traditional (per-material PSO batch) | Radiant 2.0 |
|---|---|---|
| Scene rebuild | Sort by material, build batches — O(M log M) | Set template_id per instance — O(M) |
| Frame dispatch | Iterate batches, switch PSO — O(B × P) | Issue single multi_draw — O(1) per pass |
| New material | Compile new PSO — O(compile) | No work — shares existing template |

Where M = instances, B = batches, P = passes. The constant factor for Radiant 2.0 is strictly lower.

### 9.2 GPU Cost (Compute Passes, 4K 120 FPS)

At 2160p (8,294,400 pixels), the material visibility hierarchy reduces compute pass work proportionally to material uniformity:

| Frame composition | Tiles requiring per-pixel check | Check cost (coarse + fine) |
|---|---|---|
| Fully opaque, no SSR materials | 0 tiles (mip 5 returns 0 for SSR_BIT) | ~0.05 ms (build mip chain only) |
| Glass sphere covering 2% of screen | ~200 tiles (16×16 grid at 4K ≈ 32K total tiles) | ~0.1 ms per affected compute pass |
| Dense SSR everywhere | All 32K tiles | ~0.3 ms per affected compute pass (coarse + fine for every pixel) |

### 9.3 Memory

| Resource | Size |
|---|---|
| Dispatch table (30 templates) | 120 bytes (L1 resident) |
| Dispatch table (1,000 templates) | 4 KB (L1 resident) |
| template_id_tex (1080p) | 8.3 MB |
| template_id_tex (4K) | 33.2 MB |
| Material visibility mip chain | +1.3× template_id_tex (for 6 levels) |

---

## 10. Comparison to Existing Approaches

### 10.1 Unreal Engine Material Instances

Unreal's system compiles a unique shader per material parent, and material instances are parameter overrides of that compiled shader. PSOs are generated per (material × render pass) permutation, which can reach thousands of entries in a typical scene. CPU dispatch cost scales linearly with the number of unique PSOs visible in a frame.

Radiant 2.0's template system achieves the same artist-facing result — a single "glass" master with parameterized instances — but shares the compiled shader across all instances of the template. The PSO count is bounded by the number of templates, not the number of materials.

### 10.2 Unity SRP Batcher

Unity's SRP Batcher reduces CPU dispatch cost by grouping draws that share the same shader. This approximates template-level batching but requires the shader to be authored in a specific way (constant buffer splitting). Radiant 2.0 achieves the same effect architecturally — the template IS the shader variant, and all instances share it unconditionally.

### 10.3 Frostbite Material Sort Keys

Frostbite assigns each material a sort key that encodes shader, render state, and material parameters in a 64-bit integer. Draws are sorted by key and batched. This is a CPU-side optimization of the traditional per-material dispatch model. Radiant 2.0's per-pixel dispatch operates at a different granularity entirely: instead of batching draws of the same material, it evaluates material identity per pixel, which enables correct handling of materials that mix across multiple eval points (e.g., a glass surface that writes gbuffer normals and transparent color in different passes).

---

## 11. Implementation Guide

### 11.1 Registering a Built-in Template

```rust
// Engine code: register the PBR template
let pbr_id = engine.register_template(Template {
    name: "pbr",
    targets: &["gbuffer"],
    eval_sources: Map {
        "gbuffer" -> include_str!("templates/pbr_gbuffer.wgsl"),
    },
    param_schema: ParamSchema {
        params: &[
            Param("base_color", ParamType::Float4),
            Param("roughness", ParamType::Float),
            Param("metallic", ParamType::Float),
            Param("emissive", ParamType::Float4),
        ],
        textures: &[
            TextureParam("base_color_tex"),
            TextureParam("normal_tex"),
            TextureParam("roughness_tex"),
            TextureParam("occlusion_tex"),
        ],
    },
});
```

### 11.2 Authoring a Template WGSL

```wgsl
// templates/pbr_gbuffer.wgsl — provides eval_gbuffer for the "gbuffer" eval point
// Targets: gbuffer
// Parameters: base_color (float4), roughness (float), metallic (float), emissive (float4)
// Textures: base_color_tex, normal_tex, roughness_tex, occlusion_tex

fn eval_gbuffer(material_id: u32, world_pos: vec3f, normal: vec3f, uv: vec2f) -> SurfaceData {
    let mat = materials[input.material_id];
    let tex = material_textures[input.material_id];
    // Standard PBR evaluation with Cook-Torrance GGX...
    return default_pbr_surface(mat, tex, uv, normal);
}
```

### 11.3 Creating Materials in the Scene

```rust
// Artist code (or editor output)
let red_plastic = scene.create_material(
    pbr_template_id,
    MaterialParams {
        base_color: [0.9, 0.1, 0.08, 1.0],
        roughness: 0.85,
        metallic: 0.0,
        emissive: [0.0; 4],
    },
    textures: [Some(base_tex), Some(normal_tex), Some(rough_tex), None],
);
```

### 11.4 Adding a Custom Template

```rust
// Advanced user: new shading model with custom SSR
let custom_id = engine.register_template(Template {
    name: "iridescent_clear_coat",
    targets: &["gbuffer", "ssr"],
    eval_sources: Map {
        "gbuffer" -> custom_iridescent_gbuffer,
        "ssr" -> custom_iridescent_ssr,
    },
    param_schema: ParamSchema { /* ... */ },
});
```

### 11.5 Migration from v1

| v1 API | v2 Equivalent |
|---|---|
| `register_str(name, wgsl)` | `register_template(name, wgsl, &["gbuffer"])` |
| `register_partial_str(name, wgsl)` | `register_template(name, wgsl, &["gbuffer"])` |
| `template_registry_mut()` | Removed — use `register_template()` |
| `material_class_ranges` | Removed — replaced by `template_id` in instance data |
| `FLAG_TRANSPARENT_ONLY` | Removed — use `targets: ["transparent"]` in template |

The v1 APIs are retained as deprecated shims for one release cycle, mapping to v2 internally.

---

## 12. Conclusion

Radiant 2.0 demonstrates that per-pixel material dispatch can be achieved with zero overhead for the common case and bounded, predictable cost for the exceptional case. The invariance property of the dispatch table lookup ensures that template count does not affect per-pixel cost. The material visibility hierarchy eliminates divergence in compute passes. The template abstraction bounds the system's complexity at the archetype level, independent of material instance count.

The architecture is suitable for deployment in AAA titles at 4K resolution and 120 frames per second, given that the dispatch check is dominated by early-Z culling in fragment passes and by the visibility hierarchy in compute passes. The primary authoring cost shifts from PSO management (a runtime concern in traditional engines) to template quality (an upfront asset), which is a favorable trade for editor-driven pipelines where material count is unbounded.

---

## Appendix A: Glossary

| Term | Definition |
|---|---|
| Evaluation Point | A named injection site in a render pass's shader where material-specific WGSL is composed. |
| Template | A complete shading archetype that declares its target evaluation points, provides WGSL functions for each, and defines a parameter schema. |
| Dispatch Table | A GPU-resident array of `eval_point_mask` values indexed by `template_id`. One entry per registered template. |
| Material Visibility Hierarchy | A GPU-side mip chain built from the per-pixel `template_id` buffer. Each texel stores the OR of eval point masks in its region, enabling tile-level dispatch culling. |
| Early-Z Culling | A hardware mechanism that tests a fragment's depth against the depth buffer before executing the fragment shader. Fragments that fail the test are discarded without shader invocation. |
| Two-Tier Dispatch | The architectural separation of materials into Tier 1 (fast path, zero per-pixel cost) and Tier 2 (custom path, bounded per-pixel cost). |

## Appendix B: Reference Implementation

The full specification is maintained as an implementation reference at `.agents/radiant-v2-spec.md` in the Pulsar-Native repository (11,384 lines). The implementation tracking issue is Helio#154.
