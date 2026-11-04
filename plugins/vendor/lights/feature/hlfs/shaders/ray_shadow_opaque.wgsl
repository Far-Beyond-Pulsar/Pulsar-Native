@group(3) @binding(0) var acc_struct: acceleration_structure;
// Same visibility interface as shadows.wgsl. Both sampling and reconstruction
// use this implementation in RayTraced mode; screen depth never decides a hit.
fn shadow_factor_from_receiver(id: u32, origin: vec3<f32>, position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>, frame: u32) -> Visibility {
    let light=lights[id];
    let cast_shadow=select(light.shadow_index!=INVALID_LIGHT,(light._pad&2u)!=0u,(light._pad&1u)!=0u);
    let caster_mask=select(1u,0u,(light._pad&16u)!=0u)|select(2u,0u,(light._pad&32u)!=0u);
    if !cast_shadow || caster_mask==0u { return Visibility(1.0); }
    let inc=incident(light,origin);
    var distance=globals.ray_settings.x;
    if light.light_type!=0u { distance=length(light.position_range.xyz-origin)-0.0001; }
    if distance<=0.0001 { return Visibility(1.0); }
    var query: ray_query;
    rayQueryInitialize(&query,acc_struct,RayDesc(0x05u,caster_mask,0.0001,distance,origin,inc.direction));
    while rayQueryProceed(&query) {}
    return Visibility(select(0.0,1.0,rayQueryGetCommittedIntersection(&query).kind==RAY_QUERY_INTERSECTION_NONE));
}
fn shadow_factor(id: u32, position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>, frame: u32) -> Visibility {
    return shadow_factor_from_receiver(id,shadow_receiver(position,normal,pixel),position,normal,pixel,frame);
}
