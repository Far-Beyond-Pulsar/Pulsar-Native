@group(2) @binding(0) var filtered_lighting: texture_2d<u32>;
@group(2) @binding(1) var filtered_geometry: texture_2d<u32>;
@group(2) @binding(2) var spatial_lighting: texture_storage_2d<rg32uint,write>;
var<workgroup> neighborhood_normal_depth: array<vec4<f32>,144>;
var<workgroup> neighborhood_diffuse_age: array<vec4<f32>,144>;
var<workgroup> neighborhood_specular: array<vec3<f32>,144>;
const SPATIAL_WEIGHT_RADIUS_1 = array<f32, 9>(
    1.0, 0.36787945, 0.13533528, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
const SPATIAL_WEIGHT_RADIUS_2 = array<f32, 9>(
    1.0, 0.7788008, 0.60653067, 0.47236654, 0.36787945,
    0.2865048, 0.22313017, 0.17377394, 0.13533528);
@compute @workgroup_size(8,8)
fn spatial(@builtin(global_invocation_id) id: vec3<u32>,
    @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    for(var index=lane;index<144u;index+=64u) {
        let q=clamp(vec2<i32>(group.xy*8u)+vec2<i32>(i32(index%12u)-2,i32(index/12u)-2),
            vec2<i32>(0),vec2<i32>(globals.sample_size)-1);
        let geometry=load_geometry(filtered_geometry,q);
        neighborhood_normal_depth[index]=vec4<f32>(oct_decode(geometry.xy),exp2(geometry.z));
        neighborhood_diffuse_age[index]=vec4<f32>(load_radiance(filtered_lighting,q,0u),geometry.w);
        neighborhood_specular[index]=load_radiance(filtered_lighting,q,1u);
    }
    workgroupBarrier();
    if any(id.xy>=globals.sample_size) { return; }
    let pixel=sample_pixel(id.xy,globals.frame);
    if helio_is_sky(textureLoad(gbuf_depth,vec2<i32>(pixel),0)) {
        textureStore(spatial_lighting,vec2<i32>(id.xy),vec4<u32>(0u)); return;
    }
    let s=surface_at(pixel);
    let z=-(cameras[0].view*vec4<f32>(s.position,1.0)).z;
    let sample_pos=vec2<f32>(id.xy);
    let center=clamp(vec2<i32>(round(sample_pos)),vec2<i32>(0),vec2<i32>(globals.sample_size)-1);
    let moments=load_moments(filtered_geometry,center);
    let cd=vec4<f32>(load_radiance(filtered_lighting,center,0u),moments.x);
    let cs=vec4<f32>(load_radiance(filtered_lighting,center,1u),moments.y);
    let mean_d=luminance(cd.rgb); let mean_s=luminance(cs.rgb);
    let variance=max(max(cd.a-mean_d*mean_d,0.0)/max(mean_d*mean_d,0.0001),
        max(cs.a-mean_s*mean_s,0.0)/max(mean_s*mean_s,0.0001));
    let age=load_geometry(filtered_geometry,center).w;
    let confidence=(textureLoad(filtered_geometry,center,0).y&(1u<<28u))!=0u;
    var diffuse=vec3<f32>(0.0); var specular=vec3<f32>(0.0); var weight_sum=0.0;
    if globals.debug_mode!=0u || age>globals.max_history || (age>=4.0 && variance<=0.002) {
        textureStore(spatial_lighting,center,textureLoad(filtered_lighting,center,0)); return;
    } else {
        // One sparse rotated filter, with a narrow footprint for stable signals.
        let glossy=(globals.surface_flags&4u)!=0u && s.roughness<0.2;
        let radius=select(1,2,!glossy && (variance>select(0.02,0.05,confidence) || age<4.0));
        let phase=globals.frame&3u;
        for(var y=-2;y<=2;y++) { for(var x=-2;x<=2;x++) {
            if abs(x)>radius || abs(y)>radius { continue; }
            if radius==2 && abs(x)+abs(y)>2 && ((u32(x+2)+u32(y+2)+phase)&1u)==0u { continue; }
            let p=center+vec2<i32>(x,y);
            if any(p<vec2<i32>(0)) || any(p>=vec2<i32>(globals.sample_size)) { continue; }
            let index=(lane/8u+u32(y+2))*12u+lane%8u+u32(x+2);
            let normal_depth=neighborhood_normal_depth[index];
            let diffuse_age=neighborhood_diffuse_age[index];
            let alignment=dot(normal_depth.xyz,s.normal);
            if !(diffuse_age.w>0.0 && alignment>0.9 && abs(normal_depth.w-z)<max(0.02,abs(z)*0.01)) { continue; }
            let distance_squared=u32(x*x+y*y);
            let spatial=select(SPATIAL_WEIGHT_RADIUS_1[distance_squared],
                SPATIAL_WEIGHT_RADIUS_2[distance_squared],radius==2);
            let weight=spatial*normal_weight(alignment);
            let d=diffuse_age.xyz; let sp=neighborhood_specular[index];
            // Tonemapped accumulation for disocclusions suppresses sparse fireflies.
            if age<4.0 {
                diffuse+=d/(1.0+luminance(d))*weight; specular+=sp/(1.0+luminance(sp))*weight;
            } else { diffuse+=d*weight; specular+=sp*weight; }
            weight_sum+=weight;
        }}
    }
    if weight_sum>0.0 {
        diffuse/=weight_sum; specular/=weight_sum;
        if age<4.0 && globals.debug_mode==0u {
            diffuse/=max(1.0-luminance(diffuse),1e-4); specular/=max(1.0-luminance(specular),1e-4);
        }

    }
    textureStore(spatial_lighting,center,vec4<u32>(pack_radiance(diffuse,id.xy,22u).x,pack_radiance(specular,id.xy,25u).x,0u,0u));
}
