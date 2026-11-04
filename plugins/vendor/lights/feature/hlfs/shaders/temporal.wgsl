@group(2) @binding(0) var raw_lighting: texture_2d<u32>;
@group(2) @binding(1) var history_lighting: texture_2d<u32>;
@group(2) @binding(2) var history_geometry: texture_2d<u32>;
@group(2) @binding(3) var out_lighting: texture_storage_2d<rg32uint,write>;
@group(2) @binding(4) var out_geometry: texture_storage_2d<rg32uint,write>;
@group(2) @binding(5) var<storage,read> grid: array<LightTile>;
@group(2) @binding(6) var<storage,read> visible: array<VisibleTile>;

// Cooperative 12x12 halo for the unchanged 5x5 temporal filter.
var<workgroup> neighborhood_position: array<vec4<f32>,144>;
var<workgroup> neighborhood_normal: array<vec3<f32>,144>;
var<workgroup> neighborhood_diffuse: array<vec3<f32>,144>;
var<workgroup> neighborhood_specular: array<vec3<f32>,144>;

fn to_ycocg(c: vec3<f32>) -> vec3<f32> { return vec3<f32>(0.25*c.r+0.5*c.g+0.25*c.b,0.5*c.r-0.5*c.b,-0.25*c.r+0.5*c.g-0.25*c.b); }
fn from_ycocg(c: vec3<f32>) -> vec3<f32> { return vec3<f32>(c.x+c.y-c.z,c.x+c.z,c.x-c.y-c.z); }
fn moment(c: vec3<f32>) -> f32 { let l=luminance(max(c,vec3<f32>(0.0))); return l*l; }
fn filtered_history(current: vec3<f32>, history: vec4<f32>, mean: vec3<f32>, extent: vec3<f32>, age: f32) -> vec4<f32> {
    let original=to_ycocg(history.rgb);
    var bounds=extent;
    if globals.ray_settings.z>0.5 { bounds.x=min(bounds.x,0.05*abs(mean.x)); }
    let clipped=clamp(original,mean-bounds,mean+bounds);
    let distance=length((original-clipped)/max(bounds,vec3<f32>(0.01)));
    // Strongly inconsistent history expires immediately; never a fixed 95% blend.
    let reactive=globals.ray_settings.z>0.5;
    let blend=min(age/(age+1.0),0.95)*select(clamp(1.0-distance,0.0,1.0),1.0,reactive);
    // When old illumination is statistically inconsistent, use the current
    // neighborhood estimate instead of exposing a single noisy sample.
    let reset=reactive && distance>=1.0;
    let history_color=max(from_ycocg(select(clipped,mean,reset)),vec3<f32>(0.0));
    let color=finite_color(mix(current,history_color,blend));
    return vec4<f32>(color,mix(moment(current),select(history.a,moment(history_color),reset),blend));
}
@compute @workgroup_size(8,8)
fn temporal(@builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    // All lanes participate, including partial edge workgroups.
    for(var index=lane;index<144u;index+=64u) {
        let q=clamp(vec2<i32>(group.xy*8u)+vec2<i32>(i32(index%12u)-2,i32(index/12u)-2),
            vec2<i32>(0),vec2<i32>(globals.sample_size)-1);
        let fp=sample_pixel(vec2<u32>(q),globals.frame);
        let d=textureLoad(gbuf_depth,vec2<i32>(fp),0);
        neighborhood_position[index]=vec4<f32>(world_position(vec2<f32>(fp)+0.5,d),d);
        neighborhood_normal[index]=safe_normalize(textureLoad(gbuf_normal,vec2<i32>(fp),0).xyz);
        neighborhood_diffuse[index]=to_ycocg(load_radiance(raw_lighting,q,0u));
        neighborhood_specular[index]=to_ycocg(load_radiance(raw_lighting,q,1u));
    }
    workgroupBarrier();
    if any(gid.xy>=globals.sample_size) { return; }
    let p=vec2<i32>(gid.xy);
    let full=sample_pixel(gid.xy,globals.frame);
    let depth=textureLoad(gbuf_depth,vec2<i32>(full),0);
    if helio_is_sky(depth) {
        textureStore(out_lighting,p,vec4<u32>(0u));
        textureStore(out_geometry,p,vec4<u32>(0u)); return;
    }
    let normal=safe_normalize(textureLoad(gbuf_normal,vec2<i32>(full),0).xyz);
    let glossy=(globals.surface_flags&4u)!=0u && textureLoad(gbuf_orm,vec2<i32>(full),0).g<0.2;
    let position=world_position(vec2<f32>(full)+0.5,depth);
    let z=-(cameras[0].view*vec4<f32>(position,1.0)).z;
    let tile=(full.y/TILE_SIZE)*div_ceil(globals.screen_size,TILE_SIZE).x+full.x/TILE_SIZE;
    let count=grid[tile].count;
    let population=select(count,globals.light_count,count>GRID_CAPACITY);
    // Match sample.wgsl's exhaustive thin-sheet path. Exact visibility has no
    // sampling noise; filtering it would blur chromatic shadow boundaries.
    let exact=population<=select(globals.sample_count,4u,glossy)
        || ((globals.surface_flags&16u)!=0u && population<=32u)
        || globals.debug_mode==1u;
    let diff=vec4<f32>(load_radiance(raw_lighting,p,0u),select(0.0,1.0,exact));
    let spec=vec4<f32>(load_radiance(raw_lighting,p,1u),0.0);
    var result_diff=vec4<f32>(diff.rgb,moment(diff.rgb));
    var result_spec=vec4<f32>(spec.rgb,moment(spec.rgb));
    // An age above the configured cap marks an exactly evaluated light set.
    var age=select(1.0,globals.max_history+1.0,diff.a>=1.0);
    let uv=previous_uv(full,position);
    let residual_tile=(gid.y/TILE_SIZE)*div_ceil(globals.sample_size,TILE_SIZE).x+gid.x/TILE_SIZE;
    let same_residual=(globals.surface_flags&4u)==0u || visible[residual_tile].count==0u;
    if globals.history_valid!=0u && same_residual && diff.a<1.0 && globals.debug_mode==0u && all(uv>=vec2<f32>(0.0)) && all(uv<vec2<f32>(1.0)) {
        let old=vec2<i32>(sample_position_from_uv(uv));
        let previous_z=-(globals.previous_view*vec4<f32>(position,1.0)).z;
        let geo=load_geometry(history_geometry,old);
        if geometry_matches(geo,normal,previous_z) {
            var mean_d=vec3<f32>(0.0); var mean_s=vec3<f32>(0.0);
            var squared_d=vec3<f32>(0.0); var squared_s=vec3<f32>(0.0); var count=0.0; var specular_count=0.0;
            // Geometry-aware 5x5 moments prevent bright neighbors leaking across edges.
            for(var y=-2;y<=2;y++) { for(var x=-2;x<=2;x++) {
                let index=(lane/8u+u32(y+2))*12u+lane%8u+u32(x+2);
                let position_depth=neighborhood_position[index];
                let d=position_depth.w;
                let n=neighborhood_normal[index];
                let wp=position_depth.xyz;
                if helio_is_sky(d) || dot(n,normal)<0.9 || abs(dot(wp-position,normal))>max(0.02,abs(z)*0.01) { continue; }
                let cd=neighborhood_diffuse[index];
                let cs=neighborhood_specular[index];
                mean_d+=cd; squared_d+=cd*cd; count+=1.0;
                if !glossy || (abs(x)<=1 && abs(y)<=1) {
                    mean_s+=cs; squared_s+=cs*cs; specular_count+=1.0;
                }
            }}
            mean_d/=max(count,1.0); mean_s/=max(specular_count,1.0);
            let extent_d=2.0*sqrt(max(squared_d/max(count,1.0)-mean_d*mean_d,vec3<f32>(0.0))/select(1.0,max(count,1.0),globals.ray_settings.z>0.5));
            let extent_s=2.0*sqrt(max(squared_s/max(specular_count,1.0)-mean_s*mean_s,vec3<f32>(0.0))/select(1.0,max(specular_count,1.0),globals.ray_settings.z>0.5));
            age=min(geo.w+1.0,globals.max_history);
            result_diff=filtered_history(diff.rgb,vec4<f32>(load_radiance(history_lighting,old,0u),load_moments(history_geometry,old).x),mean_d,extent_d,age-1.0);
            result_spec=filtered_history(spec.rgb,vec4<f32>(load_radiance(history_lighting,old,1u),load_moments(history_geometry,old).y),mean_s,extent_s,age-1.0);
        }
    }
    textureStore(out_lighting,p,vec4<u32>(pack_radiance(result_diff.rgb,gid.xy,10u).x,pack_radiance(result_spec.rgb,gid.xy,13u).x,0u,0u));
    let tile_id=(gid.y/TILE_SIZE)*div_ceil(globals.sample_size,TILE_SIZE).x+gid.x/TILE_SIZE;
    let confidence_bits=select(visible[tile_id].confidence_low,visible[tile_id].confidence_high,lane>=32u);
    let confidence=(confidence_bits&(1u<<(lane%32u)))!=0u;
    // Log depth retains relative rejection precision across large view ranges.
    textureStore(out_geometry,p,pack_geometry(vec4<f32>(oct_encode(normal),log2(max(z,1e-4)),age),vec2<f32>(result_diff.a,result_spec.a),gid.xy,confidence));
}
