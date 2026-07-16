# Noise Shader Builtins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 16 hash-based procedural noise nodes (white/value/Perlin/simplex/Voronoi + fBm/turbulence/ridged, 2D & 3D) in a new "Noise" shader-graph category, plus the helper-function codegen mechanism they require.

**Architecture:** Three layers across three repos, all building together locally via the Pulsar-Native root `[patch]` table: (1) Graphy's `NodeMetadata` gains a serde-defaulted `helper_functions: Vec<(String, String)>`; (2) PSGC's WGSL codegen emits deduplicated module-scope helpers for reachable nodes; (3) `wgsl_std` registers the noise nodes whose `function_source` expressions call those helpers. The shader editor auto-discovers the category (plus a two-line icon/color polish).

**Tech Stack:** Rust, WGSL, linkme distributed slices, serde, naga (test-only validation).

**Spec:** `docs/superpowers/specs/2026-07-16-noise-shader-builtins-design.md`

## Global Constraints

- All helper WGSL function names carry the `pn_` prefix; dedup is by name, first definition wins, differing redefinition logs `tracing::warn!`.
- Every scalar noise output is normalized/clamped to [0,1]; Voronoi returns `vec3<f32>(F1, F2, cell_random)` with raw distances.
- Node params are uniformly: `p` (vec2/vec3), `scale: f32`, `seed: f32`; fractals add `octaves: f32` (clamped 1–10 in WGSL), `lacunarity: f32`, `gain: f32`. All `f32` pins; unconnected defaults are `0.0` (existing engine behavior) and each description documents suggested values.
- No `sin()`-based hashes — PCG (Jarzynski–Olano) only. No license-encumbered code copied verbatim.
- Category string is exactly `"Noise"`.
- Commits: one per plan task, in the repo the task touches. Branches: graphy `feat/node-helper-functions`, psgc `feat/81-noise-builtins`, shader_editor `feat/81-noise-category` (created from the currently checked-out pinned SHAs).
- **Build/run topology (critical):** graphy + psgc unit tests run from the superproject root (`cargo test -p graphy -p psgc`) where the root `[patch]` wires both to local paths. `shader_tests` is NOT in the root graph — run it from `crates/third-party/psgc` with a **local, never-committed** `[patch]` (Task 2 Step 0 below). Before every psgc commit run `git -C crates/third-party/psgc diff --stat -- Cargo.toml` and confirm the patch is not staged.
- Shell note: this machine's bash lacks coreutils; use PowerShell for cargo/git commands.

---

### Task 1: Graphy — `helper_functions` on NodeMetadata

**Files:**
- Modify: `crates/third-party/graphy/src/core/metadata.rs` (struct at ~line 123, builders at ~line 212)

**Interfaces:**
- Produces: `NodeMetadata.helper_functions: Vec<(String, String)>` (public field) and builder `pub fn with_helpers(self, helpers: &[(&str, &str)]) -> Self`. Tasks 2+ rely on exactly these names.

- [ ] **Step 1: Create the branch**

```powershell
git -C crates/third-party/graphy switch -c feat/node-helper-functions
```

- [ ] **Step 2: Write the failing tests** — append to `crates/third-party/graphy/src/core/metadata.rs` (file has no test module yet; add at end):

```rust
#[cfg(test)]
mod helper_function_tests {
    use super::*;

    #[test]
    fn with_helpers_stores_named_sources() {
        let meta = NodeMetadata::new("perlin_2d", NodeTypes::pure, "Noise")
            .with_helpers(&[("pn_hash21", "fn pn_hash21() {}"), ("pn_perlin", "fn pn_perlin() {}")]);
        assert_eq!(meta.helper_functions.len(), 2);
        assert_eq!(meta.helper_functions[0].0, "pn_hash21");
        assert_eq!(meta.helper_functions[1].1, "fn pn_perlin() {}");
    }

    #[test]
    fn helper_functions_default_empty_from_legacy_json() {
        // Serialized metadata written before this field existed must load.
        let meta = NodeMetadata::new("add", NodeTypes::pure, "Math");
        let mut json: serde_json::Value = serde_json::to_value(&meta).unwrap();
        json.as_object_mut().unwrap().remove("helper_functions");
        let restored: NodeMetadata = serde_json::from_value(json).unwrap();
        assert!(restored.helper_functions.is_empty());
    }

    #[test]
    fn helper_functions_round_trip_serde() {
        let meta = NodeMetadata::new("perlin_2d", NodeTypes::pure, "Noise")
            .with_helpers(&[("pn_hash21", "fn pn_hash21() {}")]);
        let json = serde_json::to_string(&meta).unwrap();
        let restored: NodeMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.helper_functions, meta.helper_functions);
    }
}
```

If `serde_json` is not already a dev-dependency of graphy, add it: `cargo add serde_json --dev` run inside `crates/third-party/graphy`.

- [ ] **Step 3: Run tests to verify they fail**

```powershell
cargo test -p graphy helper_function_tests 2>&1 | Select-Object -Last 10
```
Expected: compile error — `no field helper_functions` / `no method with_helpers`.

- [ ] **Step 4: Implement** — in `NodeMetadata` struct, after the `metadata_version` field:

```rust
    /// Named module-scope helper function definitions required by
    /// `function_source`, as `(name, wgsl_source)` pairs. Emitted once per
    /// generated module (deduplicated by name across nodes) by shader
    /// codegen; ignored by blueprint codegen. List helpers dependency-first
    /// within a node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helper_functions: Vec<(String, String)>,
```

In `NodeMetadata::new`, add to the initializer: `helper_functions: Vec::new(),`

Among the builder methods, add:

```rust
    /// Sets named module-scope helper functions required by `function_source`.
    #[inline]
    #[must_use]
    pub fn with_helpers(mut self, helpers: &[(&str, &str)]) -> Self {
        self.helper_functions = helpers
            .iter()
            .map(|(name, source)| ((*name).to_string(), (*source).to_string()))
            .collect();
        self
    }
```

- [ ] **Step 5: Run tests to verify they pass**

```powershell
cargo test -p graphy 2>&1 | Select-Object -Last 5
```
Expected: all graphy tests PASS (not just the new ones — the full crate).

- [ ] **Step 6: Commit (graphy repo)**

```powershell
git -C crates/third-party/graphy add src/core/metadata.rs Cargo.toml
git -C crates/third-party/graphy commit -m "feat(metadata): named WGSL helper functions on NodeMetadata (Pulsar-Native#81)"
```

---

### Task 2: PSGC — helper emission in WGSL codegen

**Files:**
- Modify: `crates/third-party/psgc/crates/psgc/src/codegen/wgsl_codegen.rs` (struct ~line 25, `generate_entry_function` ~line 87)

**Interfaces:**
- Consumes: `NodeMetadata.helper_functions` from Task 1.
- Produces: generated WGSL modules contain each reachable node's helpers exactly once, at module scope between the uniforms binding and the entry function. `WGSLCodeGenerator` becomes generic: `WGSLCodeGenerator<'a, P: NodeMetadataProvider>` (constructor signature otherwise unchanged; `compiler.rs` needs no call-site changes).

- [ ] **Step 0: Create the psgc branch and add the LOCAL-ONLY graphy patch**

```powershell
git -C crates/third-party/psgc switch -c feat/81-noise-builtins
```

Append to `crates/third-party/psgc/Cargo.toml` (the psgc repo's workspace manifest) — **never commit this hunk**; it exists so `shader_tests` (outside the superproject graph) sees the Task 1 graphy changes:

```toml
[patch."https://github.com/Far-Beyond-Pulsar/Graphy.git"]
graphy = { path = "../graphy" }
```

- [ ] **Step 1: Write the failing test** — append to `wgsl_codegen.rs`:

```rust
#[cfg(test)]
mod helper_emission_tests {
    use super::*;
    use graphy::core::{NodeMetadata, NodeMetadataProvider};
    use graphy::{
        Connection, ConnectionType, DataResolver, DataType, GraphDescription, NodeInstance,
        NodeTypes, Pin, PinInstance, PinType, Position,
    };
    use std::collections::HashMap;

    struct TestProvider {
        nodes: HashMap<String, NodeMetadata>,
    }

    impl TestProvider {
        fn new(metas: Vec<NodeMetadata>) -> Self {
            Self {
                nodes: metas.into_iter().map(|m| (m.name.clone(), m)).collect(),
            }
        }
    }

    impl NodeMetadataProvider for TestProvider {
        fn get_node_metadata(&self, node_type: &str) -> Option<&NodeMetadata> {
            self.nodes.get(node_type)
        }
        fn get_all_nodes(&self) -> Vec<&NodeMetadata> {
            self.nodes.values().collect()
        }
        fn get_nodes_by_category(&self, category: &str) -> Vec<&NodeMetadata> {
            self.nodes.values().filter(|m| m.category == category).collect()
        }
    }

    fn scalar_node(graph: &mut GraphDescription, id: &str, node_type: &str) {
        let mut n = NodeInstance::new(id, node_type, Position { x: 0.0, y: 0.0 });
        n.inputs.push(PinInstance::new(
            format!("{id}_x"),
            Pin::new(format!("{id}_x"), "x", DataType::Typed(crate::TypeInfo::new("f32")), PinType::Input),
        ));
        n.outputs.push(PinInstance::new(
            format!("{id}_result"),
            Pin::new(format!("{id}_result"), "result", DataType::Typed(crate::TypeInfo::new("f32")), PinType::Output),
        ));
        graph.add_node(n);
    }

    /// Graph: helper_a → helper_a → helper_b → fragment_output.color (via rgba-less
    /// direct f32→vec4 is invalid, so helper_b returns vec4).
    fn build_graph_and_provider() -> (GraphDescription, TestProvider) {
        let provider = TestProvider::new(vec![
            NodeMetadata::new("noisy_scalar", NodeTypes::pure, "Test")
                .with_params(vec![graphy::ParamInfo::new("x", "f32")])
                .with_return_type("f32")
                .with_helpers(&[
                    ("pn_shared_hash", "fn pn_shared_hash(x: f32) -> f32 { return fract(x * 0.1031); }"),
                    ("pn_noisy", "fn pn_noisy(x: f32) -> f32 { return pn_shared_hash(x) * 2.0; }"),
                ])
                .with_source("pn_noisy(x)"),
            NodeMetadata::new("noisy_to_color", NodeTypes::pure, "Test")
                .with_params(vec![graphy::ParamInfo::new("x", "f32")])
                .with_return_type("vec4<f32>")
                .with_helpers(&[
                    // Shares pn_shared_hash with noisy_scalar — must emit once.
                    ("pn_shared_hash", "fn pn_shared_hash(x: f32) -> f32 { return fract(x * 0.1031); }"),
                    ("pn_to_color", "fn pn_to_color(x: f32) -> vec4<f32> { let v = pn_shared_hash(x); return vec4<f32>(v, v, v, 1.0); }"),
                ])
                .with_source("pn_to_color(x)"),
            NodeMetadata::new("fragment_output", NodeTypes::pure, "Output")
                .with_params(vec![graphy::ParamInfo::new("color", "vec4<f32>")])
                .with_return_type("vec4<f32>")
                .with_source("color"),
        ]);

        let mut graph = GraphDescription::new("helper_test");
        scalar_node(&mut graph, "n1", "noisy_scalar");
        scalar_node(&mut graph, "n2", "noisy_to_color");

        let mut out = NodeInstance::new("out", "fragment_output", Position { x: 0.0, y: 0.0 });
        out.inputs.push(PinInstance::new(
            "out_color",
            Pin::new("out_color", "color", DataType::Typed(crate::TypeInfo::new("vec4<f32>")), PinType::Input),
        ));
        graph.add_node(out);

        graph.add_connection(Connection::new("n1", "n1_result", "n2", "n2_x", ConnectionType::Data));
        graph.add_connection(Connection::new("n2", "n2_result", "out", "out_color", ConnectionType::Data));
        (graph, provider)
    }

    fn generate(graph: &GraphDescription, provider: &TestProvider) -> String {
        let resolver = DataResolver::build(graph, provider).expect("data flow");
        WGSLCodeGenerator::new(graph, provider, &resolver, ShaderStage::Fragment)
            .generate_shader()
            .expect("codegen")
    }

    #[test]
    fn helpers_emitted_once_at_module_scope() {
        let (graph, provider) = build_graph_and_provider();
        let wgsl = generate(&graph, &provider);

        // Shared helper appears exactly once despite two nodes declaring it.
        assert_eq!(wgsl.matches("fn pn_shared_hash").count(), 1, "shared helper deduped:\n{wgsl}");
        assert_eq!(wgsl.matches("fn pn_noisy").count(), 1);
        assert_eq!(wgsl.matches("fn pn_to_color").count(), 1);

        // Module scope: all helpers appear before the entry function.
        let entry = wgsl.find("@fragment").expect("entry marker");
        for helper in ["fn pn_shared_hash", "fn pn_noisy", "fn pn_to_color"] {
            assert!(wgsl.find(helper).unwrap() < entry, "{helper} must precede @fragment");
        }
    }

    #[test]
    fn unreachable_nodes_contribute_no_helpers() {
        let (mut graph, mut provider) = build_graph_and_provider();
        // A node type with a UNIQUE helper, instantiated but never connected
        // to the output — its helper must not be emitted.
        provider.nodes.insert(
            "orphan_only".to_string(),
            NodeMetadata::new("orphan_only", NodeTypes::pure, "Test")
                .with_params(vec![graphy::ParamInfo::new("x", "f32")])
                .with_return_type("f32")
                .with_helpers(&[("pn_orphan_helper", "fn pn_orphan_helper(x: f32) -> f32 { return x; }")])
                .with_source("pn_orphan_helper(x)"),
        );
        scalar_node(&mut graph, "orphan", "orphan_only");
        let wgsl = generate(&graph, &provider);
        assert!(!wgsl.contains("fn pn_orphan_helper"), "unreachable node's helper must not be emitted:\n{wgsl}");
        // And dedup for the reachable nodes still holds with the orphan present.
        assert_eq!(wgsl.matches("fn pn_shared_hash").count(), 1);
    }

    #[test]
    fn helper_free_nodes_generate_unchanged() {
        let provider = TestProvider::new(vec![NodeMetadata::new(
            "fragment_output",
            NodeTypes::pure,
            "Output",
        )
        .with_params(vec![graphy::ParamInfo::new("color", "vec4<f32>")])
        .with_return_type("vec4<f32>")
        .with_source("color")]);
        let mut graph = GraphDescription::new("plain");
        let mut out = NodeInstance::new("out", "fragment_output", Position { x: 0.0, y: 0.0 });
        out.inputs.push(PinInstance::new(
            "out_color",
            Pin::new("out_color", "color", DataType::Typed(crate::TypeInfo::new("vec4<f32>")), PinType::Input),
        ));
        graph.add_node(out);
        let wgsl = generate(&graph, &provider);
        assert!(!wgsl.contains("fn pn_"), "no helpers expected:\n{wgsl}");
        assert!(wgsl.contains("@fragment"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p psgc helper_emission 2>&1 | Select-Object -Last 10
```
Expected: compile error — `WGSLCodeGenerator::new` takes `&ShaderMetadataProvider`, not the generic test provider. (This drives the generic refactor.)

- [ ] **Step 3: Implement — genericize the generator and emit helpers**

In `wgsl_codegen.rs`, change the struct and impl to be generic over the provider (imports already include `NodeMetadataProvider`):

```rust
/// WGSL shader code generator
pub struct WGSLCodeGenerator<'a, P: NodeMetadataProvider> {
    graph: &'a GraphDescription,
    metadata_provider: &'a P,
    data_resolver: &'a DataResolver,
    stage: ShaderStage,
}

impl<'a, P: NodeMetadataProvider> WGSLCodeGenerator<'a, P> {
    pub fn new(
        graph: &'a GraphDescription,
        metadata_provider: &'a P,
        data_resolver: &'a DataResolver,
        stage: ShaderStage,
    ) -> Self {
```

(Remove the now-unused `use crate::metadata::ShaderMetadataProvider;` from this file. `compiler.rs` call sites compile unchanged because `ShaderMetadataProvider: NodeMetadataProvider`.)

In `generate_entry_function`, hoist the reachable-set computation above the uniforms block and add helper emission. Replace the beginning of the function body (up to and including the existing `let reachable = self.reachable_nodes(output_node);` line further down — delete that later line) with:

```rust
    fn generate_entry_function(&self, output_node: &NodeInstance) -> Result<String, GraphyError> {
        let mut code = String::new();

        // Only nodes the output actually depends on contribute code — both
        // helper functions here and `let` bindings below.
        let reachable = self.reachable_nodes(output_node);

        // Always declare the host-provided `uniforms` binding (matching the
        // preview renderer's vertex shader) so the generated fragment
        // module's bind group layout stays compatible whether or not this
        // particular graph reads from it (e.g. via the `time` input node).
        code.push_str("struct Uniforms {\n");
        code.push_str("    view_proj: mat4x4<f32>,\n");
        code.push_str("    model: mat4x4<f32>,\n");
        code.push_str("    time: f32,\n");
        code.push_str("};\n\n");
        code.push_str("@group(0) @binding(0) var<uniform> uniforms: Uniforms;\n\n");

        // Emit each reachable node's module-scope helper functions exactly
        // once (dedup by name, first definition wins). WGSL permits forward
        // references between module-scope functions, so emission order is a
        // readability nicety, not a correctness requirement.
        let mut emitted: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for node_id in self.data_resolver.get_pure_evaluation_order() {
            if !reachable.contains(node_id) {
                continue;
            }
            let Some(node) = self.graph.nodes.get(node_id) else { continue };
            let Some(node_meta) = self.metadata_provider.get_node_metadata(&node.node_type) else {
                continue;
            };
            for (name, source) in &node_meta.helper_functions {
                match emitted.get(name.as_str()) {
                    None => {
                        emitted.insert(name.as_str(), source.as_str());
                        code.push_str(source);
                        code.push_str("\n\n");
                    }
                    Some(prev) if *prev != source.as_str() => {
                        tracing::warn!(
                            "WGSL helper '{}' redefined with different source by node type '{}'; keeping first definition",
                            name,
                            node.node_type
                        );
                    }
                    _ => {}
                }
            }
        }
```

The rest of the function continues exactly as before (stage signature, `let` bindings loop using the hoisted `reachable`, return, closing brace) — only the duplicate `let reachable = …` line is removed.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo test -p psgc 2>&1 | Select-Object -Last 5
```
Expected: all psgc tests PASS.

- [ ] **Step 5: Baseline regression — psgc-workspace tests still green**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests 2>&1 | Select-Object -Last 5; Set-Location ..\..\..
```
Expected: existing `test_load_shader_nodes` and `test_compile_math_shader` PASS (helper-free graphs unchanged).

- [ ] **Step 6: Commit (psgc repo) — verify the patch hunk is NOT staged**

```powershell
git -C crates/third-party/psgc status --short
git -C crates/third-party/psgc add crates/psgc/src/codegen/wgsl_codegen.rs
git -C crates/third-party/psgc commit -m "feat(codegen): emit deduplicated module-scope WGSL helpers from NodeMetadata (Pulsar-Native#81)"
```
Confirm `Cargo.toml` does not appear in the commit (`git -C crates/third-party/psgc show --stat HEAD`).

---

### Task 3: shader_tests — naga validation harness

**Files:**
- Modify: `crates/third-party/psgc/crates/shader_tests/Cargo.toml`
- Modify: `crates/third-party/psgc/crates/shader_tests/src/lib.rs`

**Interfaces:**
- Produces: `fn validate_wgsl(wgsl: &str)` (panics with naga diagnostics on invalid WGSL) inside the tests module — Tasks 4–6 call it.

- [ ] **Step 1: Add naga as a dev-dependency**

```powershell
Set-Location crates/third-party/psgc; cargo add naga --dev --package shader_tests --features wgsl-in; Set-Location ..\..\..
```
Expected: latest naga added under `[dev-dependencies]` with the `wgsl-in` feature.

- [ ] **Step 2: Write the harness + failing-on-purpose canary, then baseline test** — inside the existing `mod tests` in `src/lib.rs` add:

```rust
    /// Parse + validate generated WGSL with naga. Panics with full
    /// diagnostics on any syntax or type error — no GPU required.
    fn validate_wgsl(wgsl: &str) {
        let module = naga::front::wgsl::parse_str(wgsl)
            .unwrap_or_else(|e| panic!("WGSL parse error: {}\n--- WGSL ---\n{wgsl}", e.emit_to_string(wgsl)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("WGSL validation error: {e:?}\n--- WGSL ---\n{wgsl}"));
    }

    #[test]
    fn validate_wgsl_rejects_garbage() {
        let result = std::panic::catch_unwind(|| validate_wgsl("fn broken( -> {"));
        assert!(result.is_err(), "garbage WGSL must fail validation");
    }
```

- [ ] **Step 3: Wire validation into the existing math-shader test** — in `test_compile_math_shader`, inside the `Ok(wgsl_code)` arm after the existing asserts, add:

```rust
                validate_wgsl(&wgsl_code);
```

- [ ] **Step 4: Run tests**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests 2>&1 | Select-Object -Last 6; Set-Location ..\..\..
```
Expected: PASS (3 tests). If the baseline WGSL fails naga validation, STOP — that is a pre-existing generator bug; report it before proceeding.

- [ ] **Step 5: Commit (psgc repo, patch hunk excluded)**

```powershell
git -C crates/third-party/psgc add crates/shader_tests/src/lib.rs crates/shader_tests/Cargo.toml
git -C crates/third-party/psgc commit -m "test(shader_tests): naga parse+validate harness for generated WGSL (Pulsar-Native#81)"
```
Note: `cargo add` edits `crates/shader_tests/Cargo.toml` (safe to commit); the workspace-root `Cargo.toml` patch hunk stays uncommitted.

---

### Task 4: wgsl_std — noise module scaffolding, hashes, white + value noise

**Files:**
- Create: `crates/third-party/psgc/crates/wgsl_std/src/shader/nodes/noise.rs`
- Modify: `crates/third-party/psgc/crates/wgsl_std/src/shader/nodes/mod.rs`
- Modify: `crates/third-party/psgc/crates/shader_tests/src/lib.rs` (test additions)

**Interfaces:**
- Produces: registered nodes `white_noise_2d`, `white_noise_3d`, `value_noise_2d`, `value_noise_3d` (all `(p, scale, seed) -> f32`, category "Noise") and Rust helper constants `PN_PCG2D`, `PN_PCG3D`, `PN_HASH21`, `PN_HASH22`, `PN_HASH31`, `PN_HASH33`, `PN_GRAD2`, `PN_GRAD3` reused by Tasks 5–6. Also the shader_tests helper `fn build_scalar_noise_graph(node_type: &str, extra_params: &[&str]) -> GraphDescription` reused by Tasks 5–6.

- [ ] **Step 1: Write the failing tests** — in `shader_tests/src/lib.rs` `mod tests`, add:

```rust
    /// Build: <noise node> (all params unconnected → defaults) → rgba.r → fragment_output.
    /// `extra_params` are input pin names beyond (p, scale, seed).
    fn build_scalar_noise_graph(node_type: &str, extra_params: &[&str]) -> GraphDescription {
        let mut graph = GraphDescription::new(&format!("{node_type}_test"));

        let mut output = NodeInstance::new("output_1", "fragment_output", Position { x: 900.0, y: 200.0 });
        output.inputs.push(PinInstance::new(
            "output_1_color",
            Pin::new("output_1_color", "color", DataType::Typed(psgc::TypeInfo::new("vec4<f32>")), PinType::Input),
        ));

        let mut rgba = NodeInstance::new("rgba_1", "rgba", Position { x: 700.0, y: 200.0 });
        for ch in ["r", "g", "b", "a"] {
            rgba.inputs.push(PinInstance::new(
                format!("rgba_1_{ch}"),
                Pin::new(format!("rgba_1_{ch}"), ch, DataType::Typed(psgc::TypeInfo::new("f32")), PinType::Input),
            ));
        }
        rgba.outputs.push(PinInstance::new(
            "rgba_1_result",
            Pin::new("rgba_1_result", "result", DataType::Typed(psgc::TypeInfo::new("vec4<f32>")), PinType::Output),
        ));
        rgba.properties.insert("rgba_1_a".to_string(), PropertyValue::Number(1.0));

        let mut noise = NodeInstance::new("noise_1", node_type, Position { x: 500.0, y: 200.0 });
        let p_type = if node_type.ends_with("_3d") { "vec3<f32>" } else { "vec2<f32>" };
        noise.inputs.push(PinInstance::new(
            "noise_1_p",
            Pin::new("noise_1_p", "p", DataType::Typed(psgc::TypeInfo::new(p_type)), PinType::Input),
        ));
        for param in ["scale", "seed"].iter().chain(extra_params.iter()) {
            noise.inputs.push(PinInstance::new(
                format!("noise_1_{param}"),
                Pin::new(format!("noise_1_{param}"), *param, DataType::Typed(psgc::TypeInfo::new("f32")), PinType::Input),
            ));
        }
        noise.outputs.push(PinInstance::new(
            "noise_1_result",
            Pin::new("noise_1_result", "result", DataType::Typed(psgc::TypeInfo::new("f32")), PinType::Output),
        ));

        graph.add_node(output);
        graph.add_node(rgba);
        graph.add_node(noise);
        graph.add_connection(Connection::new("noise_1", "noise_1_result", "rgba_1", "rgba_1_r", ConnectionType::Data));
        graph.add_connection(Connection::new("rgba_1", "rgba_1_result", "output_1", "output_1_color", ConnectionType::Data));
        graph
    }

    fn assert_noise_node_compiles(node_type: &str, extra_params: &[&str]) {
        let graph = build_scalar_noise_graph(node_type, extra_params);
        let wgsl = compile_fragment_shader(&graph)
            .unwrap_or_else(|e| panic!("{node_type} failed to compile: {e}"));
        assert!(wgsl.contains("fn pn_"), "{node_type} must emit pn_ helper functions:\n{wgsl}");
        validate_wgsl(&wgsl);
    }

    #[test]
    fn noise_white_and_value_nodes_compile_and_validate() {
        for node in ["white_noise_2d", "white_noise_3d", "value_noise_2d", "value_noise_3d"] {
            assert_noise_node_compiles(node, &[]);
        }
    }
```

- [ ] **Step 2: Run to verify failure**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests noise_white 2>&1 | Select-Object -Last 8; Set-Location ..\..\..
```
Expected: FAIL — `white_noise_2d failed to compile: NodeNotFound` (node type unregistered).

- [ ] **Step 3: Create `noise.rs`** with shared helper constants and the first four nodes:

```rust
//! Noise generation shader nodes (Pulsar-Native#81)
//!
//! Hash-based procedural noise: white, value, Perlin, simplex, Voronoi, and
//! fractal combinators. All helpers use the `pn_` prefix and PCG hashes
//! (Jarzynski & Olano) — no sin()-based hashing. Scalar outputs are in
//! [0, 1]; Voronoi returns vec3(F1, F2, cell_random).

use crate::SHADER_REGISTRY;
use graphy::core::{NodeMetadata, NodeTypes, ParamInfo};
use linkme::distributed_slice;

// ============================================================================
// Shared WGSL helpers (deduplicated by name at codegen)
// ============================================================================

pub(crate) const PN_PCG2D: (&str, &str) = ("pn_pcg2d", r#"fn pn_pcg2d(p: vec2<u32>) -> vec2<u32> {
    var v = p * 1664525u + 1013904223u;
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> vec2<u32>(16u));
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> vec2<u32>(16u));
    return v;
}"#);

pub(crate) const PN_PCG3D: (&str, &str) = ("pn_pcg3d", r#"fn pn_pcg3d(p: vec3<u32>) -> vec3<u32> {
    var v = p * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}"#);

pub(crate) const PN_HASH21: (&str, &str) = ("pn_hash21", r#"fn pn_hash21(p: vec2<f32>) -> f32 {
    let u = pn_pcg2d(bitcast<vec2<u32>>(vec2<i32>(floor(p))));
    return f32(u.x) * (1.0 / 4294967296.0);
}"#);

pub(crate) const PN_HASH22: (&str, &str) = ("pn_hash22", r#"fn pn_hash22(p: vec2<f32>) -> vec2<f32> {
    let u = pn_pcg2d(bitcast<vec2<u32>>(vec2<i32>(floor(p))));
    return vec2<f32>(u) * (1.0 / 4294967296.0);
}"#);

pub(crate) const PN_HASH31: (&str, &str) = ("pn_hash31", r#"fn pn_hash31(p: vec3<f32>) -> f32 {
    let u = pn_pcg3d(bitcast<vec3<u32>>(vec3<i32>(floor(p))));
    return f32(u.x) * (1.0 / 4294967296.0);
}"#);

pub(crate) const PN_HASH33: (&str, &str) = ("pn_hash33", r#"fn pn_hash33(p: vec3<f32>) -> vec3<f32> {
    let u = pn_pcg3d(bitcast<vec3<u32>>(vec3<i32>(floor(p))));
    return vec3<f32>(u) * (1.0 / 4294967296.0);
}"#);

pub(crate) const PN_GRAD2: (&str, &str) = ("pn_grad2", r#"fn pn_grad2(ip: vec2<f32>) -> vec2<f32> {
    let a = pn_hash21(ip) * 6.28318530718;
    return vec2<f32>(cos(a), sin(a));
}"#);

pub(crate) const PN_GRAD3: (&str, &str) = ("pn_grad3", r#"fn pn_grad3(ip: vec3<f32>) -> vec3<f32> {
    let h = pn_hash33(ip);
    let z = h.x * 2.0 - 1.0;
    let r = sqrt(max(1.0 - z * z, 0.0));
    let a = h.y * 6.28318530718;
    return vec3<f32>(r * cos(a), r * sin(a), z);
}"#);

// ============================================================================
// White noise
// ============================================================================

const PN_WHITE_2D: (&str, &str) = ("pn_white_2d", r#"fn pn_white_2d(p: vec2<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    return pn_hash21(floor(q));
}"#);

const PN_WHITE_3D: (&str, &str) = ("pn_white_3d", r#"fn pn_white_3d(p: vec3<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    return pn_hash31(floor(q));
}"#);

#[distributed_slice(SHADER_REGISTRY)]
pub fn white_noise_2d() -> NodeMetadata {
    NodeMetadata::new("white_noise_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_WHITE_2D])
        .with_source("pn_white_2d(p, scale, seed)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn white_noise_3d() -> NodeMetadata {
    NodeMetadata::new("white_noise_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH31, PN_WHITE_3D])
        .with_source("pn_white_3d(p, scale, seed)")
}

// ============================================================================
// Value noise (quintic-interpolated lattice)
// ============================================================================

const PN_VALUE_2D: (&str, &str) = ("pn_value_2d", r#"fn pn_value_2d(p: vec2<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    let i = floor(q);
    let f = fract(q);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = pn_hash21(i);
    let b = pn_hash21(i + vec2<f32>(1.0, 0.0));
    let c = pn_hash21(i + vec2<f32>(0.0, 1.0));
    let d = pn_hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}"#);

const PN_VALUE_3D: (&str, &str) = ("pn_value_3d", r#"fn pn_value_3d(p: vec3<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    let i = floor(q);
    let f = fract(q);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let n000 = pn_hash31(i);
    let n100 = pn_hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = pn_hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = pn_hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = pn_hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = pn_hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = pn_hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = pn_hash31(i + vec3<f32>(1.0, 1.0, 1.0));
    let nx00 = mix(n000, n100, u.x);
    let nx10 = mix(n010, n110, u.x);
    let nx01 = mix(n001, n101, u.x);
    let nx11 = mix(n011, n111, u.x);
    return mix(mix(nx00, nx10, u.y), mix(nx01, nx11, u.y), u.z);
}"#);

#[distributed_slice(SHADER_REGISTRY)]
pub fn value_noise_2d() -> NodeMetadata {
    NodeMetadata::new("value_noise_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_VALUE_2D])
        .with_source("pn_value_2d(p, scale, seed)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn value_noise_3d() -> NodeMetadata {
    NodeMetadata::new("value_noise_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH31, PN_VALUE_3D])
        .with_source("pn_value_3d(p, scale, seed)")
}
```

In `nodes/mod.rs`, add `pub mod noise;` after `pub mod math;` and `pub use noise::*;` after `pub use math::*;`.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests 2>&1 | Select-Object -Last 6; Set-Location ..\..\..
```
Expected: all PASS, including naga validation of all four generated shaders.

- [ ] **Step 5: Commit (psgc repo, patch hunk excluded)**

```powershell
git -C crates/third-party/psgc add crates/wgsl_std/src/shader/nodes/noise.rs crates/wgsl_std/src/shader/nodes/mod.rs crates/shader_tests/src/lib.rs
git -C crates/third-party/psgc commit -m "feat(wgsl_std): white + value noise nodes with PCG hash helpers (Pulsar-Native#81)"
```

---

### Task 5: wgsl_std — Perlin + simplex noise (2D/3D)

**Files:**
- Modify: `crates/third-party/psgc/crates/wgsl_std/src/shader/nodes/noise.rs`
- Modify: `crates/third-party/psgc/crates/shader_tests/src/lib.rs`

**Interfaces:**
- Consumes: `PN_PCG2D/3D`, `PN_HASH21/33`, `PN_GRAD2/3`, `build_scalar_noise_graph`, `assert_noise_node_compiles` from Task 4.
- Produces: nodes `perlin_2d`, `perlin_3d`, `simplex_2d`, `simplex_3d` (`(p, scale, seed) -> f32`); WGSL helpers `pn_perlin_2d_raw` / `pn_perlin_3d_raw` (signed output, reused by Task 6 fractals) via constants `PN_PERLIN_2D_RAW`, `PN_PERLIN_3D_RAW`.

- [ ] **Step 1: Failing test** — add to shader_tests:

```rust
    #[test]
    fn noise_perlin_and_simplex_nodes_compile_and_validate() {
        for node in ["perlin_2d", "perlin_3d", "simplex_2d", "simplex_3d"] {
            assert_noise_node_compiles(node, &[]);
        }
    }
```

- [ ] **Step 2: Verify failure**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests noise_perlin 2>&1 | Select-Object -Last 6; Set-Location ..\..\..
```
Expected: FAIL — `perlin_2d failed to compile: NodeNotFound`.

- [ ] **Step 3: Implement** — append to `noise.rs`:

```rust
// ============================================================================
// Perlin (gradient) noise
// ============================================================================

pub(crate) const PN_PERLIN_2D_RAW: (&str, &str) = ("pn_perlin_2d_raw", r#"fn pn_perlin_2d_raw(q: vec2<f32>) -> f32 {
    let i = floor(q);
    let f = fract(q);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let n00 = dot(pn_grad2(i), f);
    let n10 = dot(pn_grad2(i + vec2<f32>(1.0, 0.0)), f - vec2<f32>(1.0, 0.0));
    let n01 = dot(pn_grad2(i + vec2<f32>(0.0, 1.0)), f - vec2<f32>(0.0, 1.0));
    let n11 = dot(pn_grad2(i + vec2<f32>(1.0, 1.0)), f - vec2<f32>(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y) * 1.41421356;
}"#);

const PN_PERLIN_2D: (&str, &str) = ("pn_perlin_2d", r#"fn pn_perlin_2d(p: vec2<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    return clamp(pn_perlin_2d_raw(q) * 0.5 + 0.5, 0.0, 1.0);
}"#);

pub(crate) const PN_PERLIN_3D_RAW: (&str, &str) = ("pn_perlin_3d_raw", r#"fn pn_perlin_3d_raw(q: vec3<f32>) -> f32 {
    let i = floor(q);
    let f = fract(q);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let n000 = dot(pn_grad3(i), f);
    let n100 = dot(pn_grad3(i + vec3<f32>(1.0, 0.0, 0.0)), f - vec3<f32>(1.0, 0.0, 0.0));
    let n010 = dot(pn_grad3(i + vec3<f32>(0.0, 1.0, 0.0)), f - vec3<f32>(0.0, 1.0, 0.0));
    let n110 = dot(pn_grad3(i + vec3<f32>(1.0, 1.0, 0.0)), f - vec3<f32>(1.0, 1.0, 0.0));
    let n001 = dot(pn_grad3(i + vec3<f32>(0.0, 0.0, 1.0)), f - vec3<f32>(0.0, 0.0, 1.0));
    let n101 = dot(pn_grad3(i + vec3<f32>(1.0, 0.0, 1.0)), f - vec3<f32>(1.0, 0.0, 1.0));
    let n011 = dot(pn_grad3(i + vec3<f32>(0.0, 1.0, 1.0)), f - vec3<f32>(0.0, 1.0, 1.0));
    let n111 = dot(pn_grad3(i + vec3<f32>(1.0, 1.0, 1.0)), f - vec3<f32>(1.0, 1.0, 1.0));
    let nx00 = mix(n000, n100, u.x);
    let nx10 = mix(n010, n110, u.x);
    let nx01 = mix(n001, n101, u.x);
    let nx11 = mix(n011, n111, u.x);
    return mix(mix(nx00, nx10, u.y), mix(nx01, nx11, u.y), u.z) * 1.15470054;
}"#);

const PN_PERLIN_3D: (&str, &str) = ("pn_perlin_3d", r#"fn pn_perlin_3d(p: vec3<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    return clamp(pn_perlin_3d_raw(q) * 0.5 + 0.5, 0.0, 1.0);
}"#);

#[distributed_slice(SHADER_REGISTRY)]
pub fn perlin_2d() -> NodeMetadata {
    NodeMetadata::new("perlin_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_GRAD2, PN_PERLIN_2D_RAW, PN_PERLIN_2D])
        .with_source("pn_perlin_2d(p, scale, seed)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn perlin_3d() -> NodeMetadata {
    NodeMetadata::new("perlin_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH33, PN_GRAD3, PN_PERLIN_3D_RAW, PN_PERLIN_3D])
        .with_source("pn_perlin_3d(p, scale, seed)")
}

// ============================================================================
// Simplex noise (Gustavson construction)
// ============================================================================

const PN_SIMPLEX_2D: (&str, &str) = ("pn_simplex_2d", r#"fn pn_simplex_2d(p: vec2<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    let f2 = 0.36602540378;
    let g2 = 0.21132486540;
    let s = (q.x + q.y) * f2;
    let i = floor(q + s);
    let t = (i.x + i.y) * g2;
    let x0 = q - (i - t);
    var i1 = vec2<f32>(0.0, 1.0);
    if (x0.x > x0.y) {
        i1 = vec2<f32>(1.0, 0.0);
    }
    let x1 = x0 - i1 + g2;
    let x2 = x0 - 1.0 + 2.0 * g2;
    var n = vec3<f32>(0.0);
    var t0 = 0.5 - dot(x0, x0);
    if (t0 > 0.0) { t0 = t0 * t0; n.x = t0 * t0 * dot(pn_grad2(i), x0); }
    var t1 = 0.5 - dot(x1, x1);
    if (t1 > 0.0) { t1 = t1 * t1; n.y = t1 * t1 * dot(pn_grad2(i + i1), x1); }
    var t2 = 0.5 - dot(x2, x2);
    if (t2 > 0.0) { t2 = t2 * t2; n.z = t2 * t2 * dot(pn_grad2(i + vec2<f32>(1.0, 1.0)), x2); }
    let v = 70.0 * (n.x + n.y + n.z);
    return clamp(v * 0.5 + 0.5, 0.0, 1.0);
}"#);

const PN_SIMPLEX_3D: (&str, &str) = ("pn_simplex_3d", r#"fn pn_simplex_3d(p: vec3<f32>, scale: f32, seed: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    let f3 = 1.0 / 3.0;
    let g3 = 1.0 / 6.0;
    let s = (q.x + q.y + q.z) * f3;
    let i = floor(q + s);
    let t = (i.x + i.y + i.z) * g3;
    let x0 = q - (i - t);
    var i1 = vec3<f32>(0.0, 0.0, 0.0);
    var i2 = vec3<f32>(0.0, 0.0, 0.0);
    if (x0.x >= x0.y) {
        if (x0.y >= x0.z) {
            i1 = vec3<f32>(1.0, 0.0, 0.0);
            i2 = vec3<f32>(1.0, 1.0, 0.0);
        } else if (x0.x >= x0.z) {
            i1 = vec3<f32>(1.0, 0.0, 0.0);
            i2 = vec3<f32>(1.0, 0.0, 1.0);
        } else {
            i1 = vec3<f32>(0.0, 0.0, 1.0);
            i2 = vec3<f32>(1.0, 0.0, 1.0);
        }
    } else {
        if (x0.y < x0.z) {
            i1 = vec3<f32>(0.0, 0.0, 1.0);
            i2 = vec3<f32>(0.0, 1.0, 1.0);
        } else if (x0.x < x0.z) {
            i1 = vec3<f32>(0.0, 1.0, 0.0);
            i2 = vec3<f32>(0.0, 1.0, 1.0);
        } else {
            i1 = vec3<f32>(0.0, 1.0, 0.0);
            i2 = vec3<f32>(1.0, 1.0, 0.0);
        }
    }
    let x1 = x0 - i1 + g3;
    let x2 = x0 - i2 + 2.0 * g3;
    let x3 = x0 - 1.0 + 3.0 * g3;
    var n = vec4<f32>(0.0);
    var t0 = 0.6 - dot(x0, x0);
    if (t0 > 0.0) { t0 = t0 * t0; n.x = t0 * t0 * dot(pn_grad3(i), x0); }
    var t1 = 0.6 - dot(x1, x1);
    if (t1 > 0.0) { t1 = t1 * t1; n.y = t1 * t1 * dot(pn_grad3(i + i1), x1); }
    var t2 = 0.6 - dot(x2, x2);
    if (t2 > 0.0) { t2 = t2 * t2; n.z = t2 * t2 * dot(pn_grad3(i + i2), x2); }
    var t3 = 0.6 - dot(x3, x3);
    if (t3 > 0.0) { t3 = t3 * t3; n.w = t3 * t3 * dot(pn_grad3(i + vec3<f32>(1.0, 1.0, 1.0)), x3); }
    let v = 32.0 * (n.x + n.y + n.z + n.w);
    return clamp(v * 0.5 + 0.5, 0.0, 1.0);
}"#);

#[distributed_slice(SHADER_REGISTRY)]
pub fn simplex_2d() -> NodeMetadata {
    NodeMetadata::new("simplex_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_GRAD2, PN_SIMPLEX_2D])
        .with_source("pn_simplex_2d(p, scale, seed)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn simplex_3d() -> NodeMetadata {
    NodeMetadata::new("simplex_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH33, PN_GRAD3, PN_SIMPLEX_3D])
        .with_source("pn_simplex_3d(p, scale, seed)")
}
```

- [ ] **Step 4: Run tests to verify pass**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests 2>&1 | Select-Object -Last 6; Set-Location ..\..\..
```
Expected: PASS, naga validating the simplex branching + loops.

- [ ] **Step 5: Commit (psgc repo, patch hunk excluded)**

```powershell
git -C crates/third-party/psgc add crates/wgsl_std/src/shader/nodes/noise.rs crates/shader_tests/src/lib.rs
git -C crates/third-party/psgc commit -m "feat(wgsl_std): Perlin and simplex noise nodes, 2D and 3D (Pulsar-Native#81)"
```

---

### Task 6: wgsl_std — Voronoi, fractals, and vec3 component extraction

**Files:**
- Modify: `crates/third-party/psgc/crates/wgsl_std/src/shader/nodes/noise.rs`
- Modify: `crates/third-party/psgc/crates/wgsl_std/src/shader/nodes/vector.rs` (append after `vec3_split`, ~line 112)
- Modify: `crates/third-party/psgc/crates/shader_tests/src/lib.rs`

**Interfaces:**
- Consumes: `PN_PCG2D/3D`, `PN_HASH21/22/31/33`, `PN_PERLIN_2D_RAW`, `PN_PERLIN_3D_RAW`, test helpers from Tasks 4–5.
- Produces: nodes `voronoi_2d`/`voronoi_3d` (`(p, scale, seed) -> vec3<f32>`), `fbm_2d/3d`, `turbulence_2d/3d`, `ridged_2d/3d` (`(p, scale, seed, octaves, lacunarity, gain) -> f32`), and Vector nodes `vec3_x`, `vec3_y`, `vec3_z` (`(v: vec3<f32>) -> f32`).

- [ ] **Step 1: Failing tests** — add to shader_tests:

```rust
    #[test]
    fn noise_fractal_nodes_compile_and_validate() {
        for node in ["fbm_2d", "fbm_3d", "turbulence_2d", "turbulence_3d", "ridged_2d", "ridged_3d"] {
            assert_noise_node_compiles(node, &["octaves", "lacunarity", "gain"]);
        }
    }

    /// voronoi_2d (vec3 out) → vec3_x → rgba.r → fragment_output
    #[test]
    fn voronoi_output_splits_into_components() {
        for (voronoi, ptype) in [("voronoi_2d", "vec2<f32>"), ("voronoi_3d", "vec3<f32>")] {
            let mut graph = GraphDescription::new("voronoi_test");

            let mut output = NodeInstance::new("output_1", "fragment_output", Position { x: 900.0, y: 200.0 });
            output.inputs.push(PinInstance::new(
                "output_1_color",
                Pin::new("output_1_color", "color", DataType::Typed(psgc::TypeInfo::new("vec4<f32>")), PinType::Input),
            ));

            let mut rgba = NodeInstance::new("rgba_1", "rgba", Position { x: 700.0, y: 200.0 });
            for ch in ["r", "g", "b", "a"] {
                rgba.inputs.push(PinInstance::new(
                    format!("rgba_1_{ch}"),
                    Pin::new(format!("rgba_1_{ch}"), ch, DataType::Typed(psgc::TypeInfo::new("f32")), PinType::Input),
                ));
            }
            rgba.outputs.push(PinInstance::new(
                "rgba_1_result",
                Pin::new("rgba_1_result", "result", DataType::Typed(psgc::TypeInfo::new("vec4<f32>")), PinType::Output),
            ));

            let mut split = NodeInstance::new("x_1", "vec3_x", Position { x: 600.0, y: 200.0 });
            split.inputs.push(PinInstance::new(
                "x_1_v",
                Pin::new("x_1_v", "v", DataType::Typed(psgc::TypeInfo::new("vec3<f32>")), PinType::Input),
            ));
            split.outputs.push(PinInstance::new(
                "x_1_result",
                Pin::new("x_1_result", "result", DataType::Typed(psgc::TypeInfo::new("f32")), PinType::Output),
            ));

            let mut noise = NodeInstance::new("noise_1", voronoi, Position { x: 500.0, y: 200.0 });
            noise.inputs.push(PinInstance::new(
                "noise_1_p",
                Pin::new("noise_1_p", "p", DataType::Typed(psgc::TypeInfo::new(ptype)), PinType::Input),
            ));
            for param in ["scale", "seed"] {
                noise.inputs.push(PinInstance::new(
                    format!("noise_1_{param}"),
                    Pin::new(format!("noise_1_{param}"), param, DataType::Typed(psgc::TypeInfo::new("f32")), PinType::Input),
                ));
            }
            noise.outputs.push(PinInstance::new(
                "noise_1_result",
                Pin::new("noise_1_result", "result", DataType::Typed(psgc::TypeInfo::new("vec3<f32>")), PinType::Output),
            ));

            graph.add_node(output);
            graph.add_node(rgba);
            graph.add_node(split);
            graph.add_node(noise);
            graph.add_connection(Connection::new("noise_1", "noise_1_result", "x_1", "x_1_v", ConnectionType::Data));
            graph.add_connection(Connection::new("x_1", "x_1_result", "rgba_1", "rgba_1_r", ConnectionType::Data));
            graph.add_connection(Connection::new("rgba_1", "rgba_1_result", "output_1", "output_1_color", ConnectionType::Data));

            let wgsl = compile_fragment_shader(&graph)
                .unwrap_or_else(|e| panic!("{voronoi} failed to compile: {e}"));
            validate_wgsl(&wgsl);
        }
    }
```

- [ ] **Step 2: Verify failure**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests "noise_fractal" 2>&1 | Select-Object -Last 6; cargo test -p shader_tests voronoi 2>&1 | Select-Object -Last 6; Set-Location ..\..\..
```
Expected: both FAIL with `NodeNotFound`.

- [ ] **Step 3a: vec3 component extraction** — append to `vector.rs`:

```rust
#[distributed_slice(SHADER_REGISTRY)]
pub fn vec3_x() -> NodeMetadata {
    NodeMetadata::new("vec3_x", NodeTypes::pure, "Vector")
        .with_params(vec![ParamInfo::new("v", "vec3<f32>")])
        .with_return_type("f32")
        .with_source("v.x")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn vec3_y() -> NodeMetadata {
    NodeMetadata::new("vec3_y", NodeTypes::pure, "Vector")
        .with_params(vec![ParamInfo::new("v", "vec3<f32>")])
        .with_return_type("f32")
        .with_source("v.y")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn vec3_z() -> NodeMetadata {
    NodeMetadata::new("vec3_z", NodeTypes::pure, "Vector")
        .with_params(vec![ParamInfo::new("v", "vec3<f32>")])
        .with_return_type("f32")
        .with_source("v.z")
}
```

- [ ] **Step 3b: Voronoi + fractals** — append to `noise.rs`:

```rust
// ============================================================================
// Voronoi / Worley — returns vec3(F1, F2, cell_random)
// ============================================================================

const PN_VORONOI_2D: (&str, &str) = ("pn_voronoi_2d", r#"fn pn_voronoi_2d(p: vec2<f32>, scale: f32, seed: f32) -> vec3<f32> {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    let i = floor(q);
    let f = fract(q);
    var f1 = 8.0;
    var f2 = 8.0;
    var cell = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let n = vec2<f32>(f32(x), f32(y));
            let o = pn_hash22(i + n);
            let d = length(n + o - f);
            if (d < f1) {
                f2 = f1;
                f1 = d;
                cell = pn_hash21(i + n);
            } else if (d < f2) {
                f2 = d;
            }
        }
    }
    return vec3<f32>(f1, f2, cell);
}"#);

const PN_VORONOI_3D: (&str, &str) = ("pn_voronoi_3d", r#"fn pn_voronoi_3d(p: vec3<f32>, scale: f32, seed: f32) -> vec3<f32> {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    let i = floor(q);
    let f = fract(q);
    var f1 = 8.0;
    var f2 = 8.0;
    var cell = 0.0;
    for (var z = -1; z <= 1; z = z + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let n = vec3<f32>(f32(x), f32(y), f32(z));
                let o = pn_hash33(i + n);
                let d = length(n + o - f);
                if (d < f1) {
                    f2 = f1;
                    f1 = d;
                    cell = pn_hash31(i + n);
                } else if (d < f2) {
                    f2 = d;
                }
            }
        }
    }
    return vec3<f32>(f1, f2, cell);
}"#);

#[distributed_slice(SHADER_REGISTRY)]
pub fn voronoi_2d() -> NodeMetadata {
    NodeMetadata::new("voronoi_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("vec3<f32>")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_HASH22, PN_VORONOI_2D])
        .with_source("pn_voronoi_2d(p, scale, seed)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn voronoi_3d() -> NodeMetadata {
    NodeMetadata::new("voronoi_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
        ])
        .with_return_type("vec3<f32>")
        .with_helpers(&[PN_PCG3D, PN_HASH31, PN_HASH33, PN_VORONOI_3D])
        .with_source("pn_voronoi_3d(p, scale, seed)")
}

// ============================================================================
// Fractal combinators over signed Perlin (octaves clamped 1..10)
// ============================================================================

const PN_FBM_2D: (&str, &str) = ("pn_fbm_2d", r#"fn pn_fbm_2d(p: vec2<f32>, scale: f32, seed: f32, octaves: f32, lacunarity: f32, gain: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    let n = i32(clamp(octaves, 1.0, 10.0));
    var amp = 0.5;
    var freq = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var o = 0; o < n; o = o + 1) {
        sum = sum + amp * pn_perlin_2d_raw(q * freq);
        norm = norm + amp;
        freq = freq * lacunarity;
        amp = amp * gain;
    }
    return clamp((sum / max(norm, 0.00001)) * 0.5 + 0.5, 0.0, 1.0);
}"#);

const PN_FBM_3D: (&str, &str) = ("pn_fbm_3d", r#"fn pn_fbm_3d(p: vec3<f32>, scale: f32, seed: f32, octaves: f32, lacunarity: f32, gain: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    let n = i32(clamp(octaves, 1.0, 10.0));
    var amp = 0.5;
    var freq = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var o = 0; o < n; o = o + 1) {
        sum = sum + amp * pn_perlin_3d_raw(q * freq);
        norm = norm + amp;
        freq = freq * lacunarity;
        amp = amp * gain;
    }
    return clamp((sum / max(norm, 0.00001)) * 0.5 + 0.5, 0.0, 1.0);
}"#);

const PN_TURBULENCE_2D: (&str, &str) = ("pn_turbulence_2d", r#"fn pn_turbulence_2d(p: vec2<f32>, scale: f32, seed: f32, octaves: f32, lacunarity: f32, gain: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    let n = i32(clamp(octaves, 1.0, 10.0));
    var amp = 0.5;
    var freq = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var o = 0; o < n; o = o + 1) {
        sum = sum + amp * abs(pn_perlin_2d_raw(q * freq));
        norm = norm + amp;
        freq = freq * lacunarity;
        amp = amp * gain;
    }
    return clamp(sum / max(norm, 0.00001), 0.0, 1.0);
}"#);

const PN_TURBULENCE_3D: (&str, &str) = ("pn_turbulence_3d", r#"fn pn_turbulence_3d(p: vec3<f32>, scale: f32, seed: f32, octaves: f32, lacunarity: f32, gain: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    let n = i32(clamp(octaves, 1.0, 10.0));
    var amp = 0.5;
    var freq = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var o = 0; o < n; o = o + 1) {
        sum = sum + amp * abs(pn_perlin_3d_raw(q * freq));
        norm = norm + amp;
        freq = freq * lacunarity;
        amp = amp * gain;
    }
    return clamp(sum / max(norm, 0.00001), 0.0, 1.0);
}"#);

const PN_RIDGED_2D: (&str, &str) = ("pn_ridged_2d", r#"fn pn_ridged_2d(p: vec2<f32>, scale: f32, seed: f32, octaves: f32, lacunarity: f32, gain: f32) -> f32 {
    let q = (p + vec2<f32>(seed * 127.1, seed * 311.7)) * scale;
    let n = i32(clamp(octaves, 1.0, 10.0));
    var amp = 0.5;
    var freq = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var o = 0; o < n; o = o + 1) {
        let r = 1.0 - abs(pn_perlin_2d_raw(q * freq));
        sum = sum + amp * r * r;
        norm = norm + amp;
        freq = freq * lacunarity;
        amp = amp * gain;
    }
    return clamp(sum / max(norm, 0.00001), 0.0, 1.0);
}"#);

const PN_RIDGED_3D: (&str, &str) = ("pn_ridged_3d", r#"fn pn_ridged_3d(p: vec3<f32>, scale: f32, seed: f32, octaves: f32, lacunarity: f32, gain: f32) -> f32 {
    let q = (p + vec3<f32>(seed * 127.1, seed * 311.7, seed * 74.7)) * scale;
    let n = i32(clamp(octaves, 1.0, 10.0));
    var amp = 0.5;
    var freq = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var o = 0; o < n; o = o + 1) {
        let r = 1.0 - abs(pn_perlin_3d_raw(q * freq));
        sum = sum + amp * r * r;
        norm = norm + amp;
        freq = freq * lacunarity;
        amp = amp * gain;
    }
    return clamp(sum / max(norm, 0.00001), 0.0, 1.0);
}"#);
```

Then the six fractal node registrations. `fbm_2d` shown in full; `fbm_3d`, `turbulence_2d/3d`, `ridged_2d/3d` are identical except for the node name, `p` type (`vec3<f32>` for `_3d`), helper list (3D variants use `PN_PCG3D, PN_HASH33, PN_GRAD3, PN_PERLIN_3D_RAW` + own const), and source string:

```rust
#[distributed_slice(SHADER_REGISTRY)]
pub fn fbm_2d() -> NodeMetadata {
    NodeMetadata::new("fbm_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
            ParamInfo::new("octaves", "f32"),
            ParamInfo::new("lacunarity", "f32"),
            ParamInfo::new("gain", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_GRAD2, PN_PERLIN_2D_RAW, PN_FBM_2D])
        .with_source("pn_fbm_2d(p, scale, seed, octaves, lacunarity, gain)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn fbm_3d() -> NodeMetadata {
    NodeMetadata::new("fbm_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
            ParamInfo::new("octaves", "f32"),
            ParamInfo::new("lacunarity", "f32"),
            ParamInfo::new("gain", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH33, PN_GRAD3, PN_PERLIN_3D_RAW, PN_FBM_3D])
        .with_source("pn_fbm_3d(p, scale, seed, octaves, lacunarity, gain)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn turbulence_2d() -> NodeMetadata {
    NodeMetadata::new("turbulence_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
            ParamInfo::new("octaves", "f32"),
            ParamInfo::new("lacunarity", "f32"),
            ParamInfo::new("gain", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_GRAD2, PN_PERLIN_2D_RAW, PN_TURBULENCE_2D])
        .with_source("pn_turbulence_2d(p, scale, seed, octaves, lacunarity, gain)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn turbulence_3d() -> NodeMetadata {
    NodeMetadata::new("turbulence_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
            ParamInfo::new("octaves", "f32"),
            ParamInfo::new("lacunarity", "f32"),
            ParamInfo::new("gain", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH33, PN_GRAD3, PN_PERLIN_3D_RAW, PN_TURBULENCE_3D])
        .with_source("pn_turbulence_3d(p, scale, seed, octaves, lacunarity, gain)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn ridged_2d() -> NodeMetadata {
    NodeMetadata::new("ridged_2d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec2<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
            ParamInfo::new("octaves", "f32"),
            ParamInfo::new("lacunarity", "f32"),
            ParamInfo::new("gain", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG2D, PN_HASH21, PN_GRAD2, PN_PERLIN_2D_RAW, PN_RIDGED_2D])
        .with_source("pn_ridged_2d(p, scale, seed, octaves, lacunarity, gain)")
}

#[distributed_slice(SHADER_REGISTRY)]
pub fn ridged_3d() -> NodeMetadata {
    NodeMetadata::new("ridged_3d", NodeTypes::pure, "Noise")
        .with_params(vec![
            ParamInfo::new("p", "vec3<f32>"),
            ParamInfo::new("scale", "f32"),
            ParamInfo::new("seed", "f32"),
            ParamInfo::new("octaves", "f32"),
            ParamInfo::new("lacunarity", "f32"),
            ParamInfo::new("gain", "f32"),
        ])
        .with_return_type("f32")
        .with_helpers(&[PN_PCG3D, PN_HASH33, PN_GRAD3, PN_PERLIN_3D_RAW, PN_RIDGED_3D])
        .with_source("pn_ridged_3d(p, scale, seed, octaves, lacunarity, gain)")
}
```

- [ ] **Step 4: Registration completeness test** — add to shader_tests:

```rust
    #[test]
    fn all_sixteen_noise_nodes_registered() {
        let provider = ShaderMetadataProvider::new();
        let noise = provider.get_nodes_by_category("Noise");
        let expected = [
            "white_noise_2d", "white_noise_3d", "value_noise_2d", "value_noise_3d",
            "perlin_2d", "perlin_3d", "simplex_2d", "simplex_3d",
            "voronoi_2d", "voronoi_3d", "fbm_2d", "fbm_3d",
            "turbulence_2d", "turbulence_3d", "ridged_2d", "ridged_3d",
        ];
        assert_eq!(noise.len(), expected.len(), "exactly 16 Noise nodes");
        for name in expected {
            assert!(noise.iter().any(|n| n.name == name), "missing node: {name}");
        }
        for name in ["vec3_x", "vec3_y", "vec3_z"] {
            assert!(provider.get_node_metadata(name).is_some(), "missing vector node: {name}");
        }
    }
```

- [ ] **Step 5: Run everything**

```powershell
Set-Location crates/third-party/psgc; cargo test -p shader_tests 2>&1 | Select-Object -Last 6; cargo test -p psgc 2>&1 | Select-Object -Last 4; Set-Location ..\..\..
```
Expected: all PASS.

- [ ] **Step 6: Commit (psgc repo, patch hunk excluded)**

```powershell
git -C crates/third-party/psgc add crates/wgsl_std/src/shader/nodes/noise.rs crates/wgsl_std/src/shader/nodes/vector.rs crates/shader_tests/src/lib.rs
git -C crates/third-party/psgc commit -m "feat(wgsl_std): Voronoi, fBm/turbulence/ridged fractals, vec3 component nodes (Pulsar-Native#81)"
```

---

### Task 7: shader_editor — "Noise" category icon and color

**Files:**
- Modify: `plugins/vendor/shader_editor/src/core/definitions.rs` (icon match ~line 145, color match ~line 187)

**Interfaces:**
- Consumes: category string `"Noise"` from Task 4–6 nodes.

- [ ] **Step 1: Create the branch**

```powershell
git -C plugins/vendor/shader_editor switch -c feat/81-noise-category
```

- [ ] **Step 2: Add the match arms.** In the icon match inside `from_shader_metadata`:

```rust
                    "Noise" => "▒",
```
(insert after the `"Texture" => "◉",` arm). In `get_category_color`:

```rust
            "Noise" => "#8E44AD".to_string(),
```
(insert after the `"Texture"` arm).

- [ ] **Step 3: Compile check** (editor plugin builds within superproject graph):

```powershell
cargo check -p shader_editor_plugin 2>&1 | Select-Object -Last 3
```
If the package name differs, find it: `Get-Content plugins/vendor/shader_editor/Cargo.toml | Select-String "^name"` and use that name. Expected: clean check.

- [ ] **Step 4: Commit (shader_editor repo)**

```powershell
git -C plugins/vendor/shader_editor add src/core/definitions.rs
git -C plugins/vendor/shader_editor commit -m "feat(nodes): icon and color for the Noise category (Pulsar-Native#81)"
```

---

### Task 8: Full verification sweep + editor smoke test

**Files:** none new — verification and bookkeeping only.

- [ ] **Step 1: Full automated suite, all layers**

```powershell
cargo test -p graphy -p psgc 2>&1 | Select-Object -Last 5
Set-Location crates/third-party/psgc; cargo test 2>&1 | Select-Object -Last 5; Set-Location ..\..\..
```
Expected: all green everywhere.

- [ ] **Step 2: Release build of the editor for manual smoke test**

```powershell
cargo build --release -j 10
```
(background it; ~6–8 min). Then launch `target\release\pulsar_engine.exe` detached and hand off to the user with this checklist:
- New shader → node palette shows a **Noise** category (purple, ▒).
- Place `perlin_2d`, feed `frag_uv` into `p`, set scale via a connected constant → preview shows smooth grayscale noise.
- Place `fbm_2d` → visibly more detailed; changing octaves changes detail.
- Place `voronoi_2d` → `vec3_x` → cellular pattern.
- No console errors during any of the above.

- [ ] **Step 3: After user confirms, verify clean state of all three repos**

```powershell
git -C crates/third-party/graphy status --short
git -C crates/third-party/psgc status --short   # ONLY the uncommitted Cargo.toml patch hunk may appear
git -C plugins/vendor/shader_editor status --short
git status --short                              # superproject: submodule pointers + docs
```

- [ ] **Step 4: Superproject commit** — spec + plan + (later, at publish time) submodule bumps. Publishing (pushes, PR chain Graphy → PSGC rev-bump → Pulsar-Native) is a separate user-approved step, not part of this plan's execution.

```powershell
git add docs/superpowers/
git commit -m "docs(shader): spec + implementation plan for noise builtins (#81)"
```

---

## Publish checklist (post-plan, user-gated)

1. Push graphy branch; open Graphy PR; merge.
2. In psgc: update `crates/psgc/Cargo.toml` and `crates/shader_tests/Cargo.toml` graphy `rev =` to the merged Graphy SHA; **remove is unnecessary** (the patch hunk was never committed); commit `chore: bump graphy rev`; push; open PSGC PR; merge.
3. In superproject: `git add crates/third-party/graphy crates/third-party/psgc plugins/vendor/shader_editor`, commit pointer bumps, push branch `feat/81-noise-shader-builtins`, open Pulsar-Native PR with `Fixes #81`.
