// Cable pair expansion. One thread per ordered connector pair (a, b): a live
// pair (same nonzero ID, a < b, enabled definition) appends one instance per
// bundle lane to the ribbon or tube list and bumps that list's indirect draw
// instance count. The draw never sees a dead candidate.
@group(0) @binding(0) var<uniform> u: CableUniforms;
@group(0) @binding(1) var<storage, read> connector_words: array<u32>;
@group(0) @binding(2) var<storage, read> definition_words: array<u32>;
@group(0) @binding(3) var<storage, read_write> args: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> instances: array<u32>;

@compute @workgroup_size(64)
fn build(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = u.connector_count;
    if (gid.x >= n * n) { return; }
    let a = gid.x / n;
    let b = gid.x % n;
    if (b <= a) { return; }
    let ao = a * CONNECTOR_WORDS;
    let bo = b * CONNECTOR_WORDS;
    if (connector_words[ao + 17u] == 0u || connector_words[bo + 17u] == 0u) { return; }
    let id = connector_words[ao + 16u];
    if (id == 0u || connector_words[bo + 16u] != id) { return; }
    var definition = 0xffffffffu;
    for (var k = 0u; k < u.definition_count; k++) {
        let d = k * DEFINITION_WORDS;
        if (definition_words[d] == id && definition_words[d + 1u] != 0u) {
            definition = d;
            break;
        }
    }
    if (definition == 0xffffffffu) { return; }
    let tube = definition_words[definition + 2u] != 0u;
    let lanes = select(1u, clamp(definition_words[definition + 3u], 1u, BUNDLE_LANES), tube);
    let list = select(0u, 1u, tube);
    let base = atomicAdd(&args[list * 4u + 1u], lanes);
    for (var lane = 0u; lane < lanes; lane++) {
        let slot = base + lane;
        if (slot >= INSTANCE_CAPACITY) { break; }
        let o = (list * INSTANCE_CAPACITY + slot) * 4u;
        instances[o] = a;
        instances[o + 1u] = b;
        instances[o + 2u] = definition;
        instances[o + 3u] = lane;
    }
}
