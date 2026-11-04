// The lights' shadow atlas (`helio_light_api`, `//!use shadow_atlas`): the
// shadow views' per-face rows (`LightMatrix`, `helio_light_api::
// GpuShadowMatrix`), the cube-face order, and how a face's logical uv maps
// into the budgeted atlas (its tile, layer, resolution and fade). A reader
// binds the views' rows as `shadow_matrices: array<LightMatrix>` and passes
// the atlas it samples (dynamic or static casters, the coloured
// transmittance) to the sampling functions; how it filters is its own.

struct LightMatrix {
    mat:    mat4x4<f32>,
    // xy the face's tile offset in the atlas, z its size, w its fade.
    atlas:  vec4f,
    // x its physical layer, y the caster kinds it skips, z its resolution,
    // w 0 for an unbudgeted face, 2 for a resident one.
    policy: vec4u,
}

// A point light's cube face toward `dir`: +X, -X, +Y, -Y, +Z, -Z (the views'
// order; ties as the views break them).
fn point_light_face(dir: vec3<f32>) -> u32 {
    let a = abs(dir);
    if a.x >= a.y && a.x >= a.z { return select(0u, 1u, dir.x < 0.0); }
    if a.y >= a.x && a.y >= a.z { return select(2u, 3u, dir.y < 0.0); }
    return select(4u, 5u, dir.z < 0.0);
}

// Logical face metadata maps all filtering into a guarded physical tile.
fn budget_resolution(layer:u32)->f32 {
    if layer>=arrayLength(&shadow_matrices) {return 128.0;}
    let m=shadow_matrices[layer];
    return select(max(f32(m.policy.z),128.0),1024.0,m.policy.w==0u);
}
fn budget_layer(layer:u32)->u32 {
    return select(shadow_matrices[layer].policy.x,layer,shadow_matrices[layer].policy.w==0u);
}
fn budget_uv(uv:vec2f,layer:u32,dims:vec2f)->vec2f {
    let m=shadow_matrices[layer];
    let offset=select(m.atlas.xy,vec2f(0),m.policy.w==0u);
    let size=select(m.atlas.z,1.0,m.policy.w==0u);
    let half_texel=0.5/dims;
    return clamp(offset+uv*size,offset+half_texel,offset+vec2f(size)-half_texel);
}
// `disabled`: the caster-kind bit the atlas holds (16 static, 32 dynamic).
fn budget_valid(layer:u32,disabled:u32)->bool {
    if layer>=arrayLength(&shadow_matrices) {return false;}
    let m=shadow_matrices[layer];
    return m.policy.w==0u || (m.policy.w==2u && m.atlas.z>0.0 && (m.policy.y&disabled)==0u);
}
fn budget_strength(layer:u32)->f32 {
    let m=shadow_matrices[layer];return select(m.atlas.w,1.0,m.policy.w==0u);
}

// One comparison of `depth` against face `layer` of `atlas` at `uv`, faded
// by the face's residency; 1 (lit) where the face holds no such casters.
fn budget_compare(atlas:texture_depth_2d_array,samp:sampler_comparison,uv:vec2f,layer:u32,depth:f32,disabled:u32)->f32 {
    if !budget_valid(layer,disabled) {return 1.0;}
    if budget_layer(layer)>=textureNumLayers(atlas) {return 1.0;}
    let value=textureSampleCompareLevel(atlas,samp,budget_uv(uv,layer,vec2f(textureDimensions(atlas))),i32(budget_layer(layer)),depth);
    return mix(1.0,value,budget_strength(layer));
}
// The occluder depth of face `layer` of `atlas` at its texel `pixel`.
fn budget_depth(atlas:texture_depth_2d_array,pixel:vec2i,layer:u32,disabled:u32)->f32 {
    if !budget_valid(layer,disabled) {return 1.0;}
    if budget_layer(layer)>=textureNumLayers(atlas) {return 1.0;}
    let dims=vec2f(textureDimensions(atlas));
    let uv=(vec2f(pixel)+0.5)/budget_resolution(layer);
    return textureLoad(atlas,vec2i(budget_uv(uv,layer,dims)*dims),i32(budget_layer(layer)),0);
}

// Tint of the stained-glass panes a receiver lies behind (`transmittance`:
// the views' coloured transmittance, rgb the pane's absorption, a its
// depth). Each of the four nearest texels is depth-tested on its own, then
// the results are weighted bilinearly: filtering the pane depth first and
// testing once failed along every pane edge next to a texel with no pane.
fn budget_glass_tint(transmittance:texture_2d_array<f32>,uv:vec2f,layer:i32,receiver:f32)->vec3f {
    let face=u32(layer);
    if !budget_valid(face,16u) {return vec3f(1);}
    let physical=i32(budget_layer(face));
    if physical>=i32(textureNumLayers(transmittance)) {return vec3f(1);}
    let dims=vec2f(textureDimensions(transmittance));
    let p=budget_uv(uv,face,dims)*dims-0.5;
    let base=floor(p);let f=fract(p);var tint=vec3f(0);
    let m=shadow_matrices[face];
    let offset=select(m.atlas.xy,vec2f(0),m.policy.w==0u);
    let size=select(m.atlas.z,1.0,m.policy.w==0u);
    for(var i=0;i<4;i++) {
        let o=vec2i(i&1,i>>1);
        let pixel=clamp(vec2i(base)+o,vec2i(offset*dims),vec2i((offset+vec2f(size))*dims)-1);
        let t=textureLoad(transmittance,pixel,physical,0);
        let w=select(1.0-f.x,f.x,o.x==1)*select(1.0-f.y,f.y,o.y==1);
        tint+=w*select(vec3f(1),1.0-t.rgb,receiver<t.a);
    }
    return mix(vec3f(1),tint,budget_strength(face));
}
