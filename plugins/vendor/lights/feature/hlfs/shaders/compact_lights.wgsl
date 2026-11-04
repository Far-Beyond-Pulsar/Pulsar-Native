struct SourceLight {
    position_range: vec4<f32>, direction_outer: vec4<f32>, color_intensity: vec4<f32>,
    shadow_index: u32, light_type: u32, inner_angle: f32, _pad: u32,
    god_rays_enabled: u32, god_rays_density: f32, god_rays_weight: f32, god_rays_decay: f32,
    god_rays_exposure: f32, flare_enabled: u32, flare_type: u32, flare_intensity: f32,
    flare_scale: f32, flare_tint_r: f32, flare_tint_g: f32, flare_tint_b: f32,
    ies_profile_index: i32, light_function_index: i32, ies_angle_scale: f32, ies_angle_offset: f32,
}
struct CompactLight {
    position_range: vec4<f32>, direction_outer: vec4<f32>, color_intensity: vec4<f32>,
    shadow_index: u32, light_type: u32, inner_angle: f32, _pad: u32,
}
@group(0) @binding(0) var<storage,read> source_lights: array<SourceLight>;
@group(0) @binding(1) var<storage,read_write> compact_lights: array<CompactLight>;

@compute @workgroup_size(256)
fn compact(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index=gid.x;
    if index>=arrayLength(&source_lights) || index>=arrayLength(&compact_lights) { return; }
    let source=source_lights[index];
    compact_lights[index]=CompactLight(source.position_range,source.direction_outer,
        source.color_intensity,source.shadow_index,source.light_type,source.inner_angle,source._pad);
}

// Prefix of HLFS `Globals`: only the allocated row count is read here.
struct Counts { frame: u32, sample_count: u32, light_count: u32, history_valid: u32, }
@group(0) @binding(2) var<storage,read_write> live_lights: array<u32>;
@group(0) @binding(3) var<uniform> counts: Counts;
var<workgroup> chunk_offsets: array<u32,256>;

// A row that can light anything. Vacant SceneDB rows are zeroed, and the
// light grid rejects exactly these, so leaving them out changes no result.
fn is_live(light: SourceLight) -> bool {
    if light.color_intensity.w<=0.0 || all(light.color_intensity.rgb<=vec3<f32>(0.0)) { return false; }
    return light.light_type==0u || light.position_range.w>0.0;
}

// Live rows in row order (Pulsar-Native#838): live_lights[0] is the count and
// live_lights[1+k] the row of the k-th live light. Sweeps and uniform picks
// run over this list, so despawned rows cost nothing. Order is deterministic
// (reservoir strata index it) and IDs stay row indices, so history is kept.
@compute @workgroup_size(256)
fn list_live(@builtin(local_invocation_index) lane: u32) {
    let n=min(counts.light_count,arrayLength(&source_lights));
    let chunk=(n+255u)/256u;
    let begin=min(lane*chunk,n);
    let end=min(begin+chunk,n);
    var live=0u;
    for(var i=begin;i<end;i++) { if is_live(source_lights[i]) { live++; } }
    chunk_offsets[lane]=live;
    workgroupBarrier();
    if lane==0u {
        var total=0u;
        for(var j=0u;j<256u;j++) { let c=chunk_offsets[j]; chunk_offsets[j]=total; total+=c; }
        live_lights[0]=total;
    }
    workgroupBarrier();
    var slot=chunk_offsets[lane]+1u;
    for(var i=begin;i<end;i++) {
        if is_live(source_lights[i]) { live_lights[slot]=i; slot++; }
    }
}
