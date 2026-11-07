// Shared receiver offset for opaque and transmitting RT paths.
fn shadow_receiver(position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    // Perspective depth quantization grows into millimetres of world-space
    // error at distance. A position-magnitude-only offset self-shadows even a
    // flat receiver. Estimate the local error along its normal from one depth
    // ULP, in addition to the floor for transform/traversal rounding.
    let rounding=max(max(abs(position.x),abs(position.y)),abs(position.z))*0.000002;
    let velocity=textureLoad(gbuf_velocity,vec2<i32>(pixel),0);
    // Corrected G-buffer receivers no longer carry depth-buffer quantization
    // error. Bound the FP16 residual's relative rounding plus transform error;
    // legacy producers retain the depth-ULP bound instead of assuming precision.
    // w: 2 marks a valid residual (1 a moving instance, helio-pass-gbuffer).
    let corrected=globals.has_velocity!=0u && (u32(velocity.w)&2u)!=0u;
    var error=abs(velocity.z)*0.001;
    if !corrected {
        let depth=textureLoad(gbuf_depth,vec2<i32>(pixel),0);
        let adjacent_depth=bitcast<f32>(bitcast<u32>(depth)+1u);
        let depth_error=abs(world_position(pixel,adjacent_depth)-position);
        error=dot(abs(normal),depth_error);
    }
    let bias=max(0.0001,error+rounding);
    return position+normal*bias;
}
