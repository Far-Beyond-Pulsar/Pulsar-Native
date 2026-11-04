//!use helio_prelude
// Nearest-depth pyramid over the per-tile depth bounds, for the screen-space
// shadow march.
@group(2) @binding(0) var source_depth: texture_2d<f32>;
@group(2) @binding(1) var reduced_depth: texture_storage_2d<r32float,write>;
@compute @workgroup_size(8,8)
fn reduce_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy>=textureDimensions(reduced_depth)) { return; }
    let edge=vec2<i32>(textureDimensions(source_depth))-1;
    let p=vec2<i32>(id.xy)*2;
    let d=helio_depth_nearest(
        helio_depth_nearest(textureLoad(source_depth,min(p,edge),0).r,textureLoad(source_depth,min(p+vec2<i32>(1,0),edge),0).r),
        helio_depth_nearest(textureLoad(source_depth,min(p+vec2<i32>(0,1),edge),0).r,textureLoad(source_depth,min(p+vec2<i32>(1,1),edge),0).r));
    textureStore(reduced_depth,vec2<i32>(id.xy),vec4<f32>(d));
}
