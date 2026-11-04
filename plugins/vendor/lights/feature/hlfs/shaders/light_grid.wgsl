// Two-level conservative culling. Overflow is explicit, never a truncated light set.
@group(2) @binding(0) var<storage, read_write> coarse_grid: array<CoarseTile>;
@group(2) @binding(1) var<storage, read_write> fine_grid: array<LightTile>;
@group(2) @binding(2) var depth_bounds: texture_storage_2d<r32float,write>;
@group(2) @binding(3) var<storage,read_write> tile_proposals: array<LightProposal>;
var<workgroup> proposal_weights: array<f32,256>;
var<workgroup> alias_probabilities: array<f32,256>;
var<workgroup> alias_indices: array<u32,256>;
var<workgroup> small_aliases: array<u32,512>;
var<workgroup> large_aliases: array<u32,512>;
var<workgroup> alias_counts: array<atomic<u32>,4>;
var<workgroup> proposal_total: f32;
var<workgroup> key_light: u32;
var<workgroup> active_emitters: u32;
var<workgroup> accepted: atomic<u32>;
var<workgroup> has_directional: atomic<u32>;
var<workgroup> min_depth: atomic<u32>;
var<workgroup> max_depth: atomic<u32>;
var<workgroup> packed: array<u32, 256>;
var<workgroup> sorted_fine: array<u32, 64>;

fn sphere_in_tile(light: GpuLight, lo: vec2<u32>, hi: vec2<u32>, zlo: f32, zhi: f32) -> bool {
    // SceneDB buffers are sparse: unoccupied rows are zeroed, including
    // light_type (directional). They must not fill every tile or force overflow.
    if light.color_intensity.w<=0.0 || all(light.color_intensity.rgb<=vec3<f32>(0.0)) { return false; }
    if light.light_type == 0u { return true; }
    if light.position_range.w <= 0.0 { return false; }
    let m = cameras[0].view_proj;
    let r0 = vec4<f32>(m[0][0],m[1][0],m[2][0],m[3][0]);
    let r1 = vec4<f32>(m[0][1],m[1][1],m[2][1],m[3][1]);
    let r2 = vec4<f32>(m[0][2],m[1][2],m[2][2],m[3][2]);
    let r3 = vec4<f32>(m[0][3],m[1][3],m[2][3],m[3][3]);
    let lower = vec2<f32>(lo) / vec2<f32>(globals.screen_size);
    let upper = vec2<f32>(min(hi, globals.screen_size)) / vec2<f32>(globals.screen_size);
    let planes = array<vec4<f32>, 6>(
        r0 - (2.0*lower.x-1.0)*r3, (2.0*upper.x-1.0)*r3-r0,
        (1.0-2.0*lower.y)*r3-r1, r1-(1.0-2.0*upper.y)*r3,
        r2-zlo*r3, zhi*r3-r2);
    for (var i=0u; i<6u; i++) {
        let plane=planes[i];
        if dot(plane,vec4<f32>(light.position_range.xyz,1.0)) < -light.position_range.w*length(plane.xyz) - 1e-4 {
            return false;
        }
    }
    return true;
}
@compute @workgroup_size(256)
fn select_key(@builtin(local_invocation_index) lane: u32) {
    let presample=(globals.surface_flags&4u)!=0u;
    // Split one globally dominant emitter from the stochastic residual. The
    // same identity is used by every tile so filtering never crosses different
    // decompositions. Composition evaluates this emitter exactly at full size.
    if lane==0u { key_light=INVALID_LIGHT; active_emitters=0u; }
    // Population means active emitters, not allocated SceneDB entity slots.
    // Keep the same decomposition for equivalent dense and sparse light sets.
    if presample && globals.debug_mode!=1u && globals.light_count<=65535u {
        var power_sum=0.0; var maximum=0.0; var best=INVALID_LIGHT;
        var active_count=0u; var sun_power=0.0; var sun=INVALID_LIGHT;
        for(var k=lane;k<live_light_count();k+=256u) {
            let i=live_light(k);
            let light=lights[i];
            if light.color_intensity.w<=0.0 || all(light.color_intensity.rgb<=vec3<f32>(0.0)) { continue; }
            if light.light_type!=0u && light.position_range.w<=0.0 { continue; }
            let power=luminance(max(light.color_intensity.rgb*light.color_intensity.w,vec3<f32>(0.0)));
            if !(power>0.0) { continue; }
            active_count+=1u;
            power_sum+=power;
            if power>maximum { maximum=power; best=i; }
            if light.light_type==0u && power>sun_power { sun_power=power; sun=i; }
        }
        proposal_weights[lane]=power_sum;
        alias_probabilities[lane]=maximum;
        alias_indices[lane]=best;
        packed[lane]=active_count;
        small_aliases[lane]=sun;
        large_aliases[lane]=bitcast<u32>(sun_power);
        workgroupBarrier();
        if lane==0u {
            var total=0.0; var peak=0.0; var id=INVALID_LIGHT;
            var population=0u; var directional_power=0.0; var directional=INVALID_LIGHT;
            for(var i=0u;i<256u;i++) {
                total+=proposal_weights[i];
                population+=packed[i];
                if alias_probabilities[i]>peak || (alias_probabilities[i]==peak && alias_indices[i]<id) {
                    peak=alias_probabilities[i]; id=alias_indices[i];
                }
                let power=bitcast<f32>(large_aliases[i]);
                if power>directional_power || (power==directional_power && small_aliases[i]<directional) {
                    directional_power=power; directional=small_aliases[i];
                }
            }
            // Small outdoor sets retain a full-resolution sun. Larger sets
            // split only a globally dominant emitter from their residual.
            active_emitters=population;
            if population<=GRID_CAPACITY { key_light=directional; }
            else if peak>16.0*total/f32(population) { key_light=id; }
        }
    }
    workgroupBarrier();
    // A global two-word fingerprint rejects temporal proposals when light
    // positions, colors, ranges or shadow policy change at fixed slot capacity.
    // Reservoir reuse remains conservative for animated light sets; visibility
    // itself is always freshly traced, so moving casters do not require a reset.
    var stamp0=0u; var stamp1=0u;
    if (globals.surface_flags&8u)!=0u {
        // Vacant rows are zeroed and never sampled; only live rows can change
        // what a stored reservoir means.
        for(var k=lane;k<live_light_count();k+=256u) {
            let i=live_light(k);
            let light=lights[i];
            var a=hash_u32(i); var b=hash_u32(i^0x85ebca6bu);
            for(var component=0u;component<4u;component++) {
                a=hash_u32(a^bitcast<u32>(light.position_range[component]));
                a=hash_u32(a^bitcast<u32>(light.color_intensity[component]));
                b=hash_u32(b^bitcast<u32>(light.direction_outer[component]));
                b=hash_u32(b^bitcast<u32>(light.color_intensity[component]));
            }
            a=hash_u32(a^light.light_type^light.shadow_index);
            b=hash_u32(b^bitcast<u32>(light.inner_angle));
            stamp0^=a; stamp1+=b;
        }
        packed[lane]=stamp0; small_aliases[lane]=stamp1;
        workgroupBarrier();
        if lane==0u {
            stamp0=hash_u32(key_light); stamp1=hash_u32(globals.light_count)^hash_u32(live_light_count()+0x68e31da4u);
            for(var i=0u;i<256u;i++) { stamp0^=packed[i]; stamp1+=small_aliases[i]; }
        }
    }
    if lane==0u { tile_proposals[arrayLength(&tile_proposals)-1u]=LightProposal(stamp0,f32(active_emitters),stamp1,0.0,0.0,key_light,array<f32,4>(0.0,0.0,0.0,0.0)); }
}
@compute @workgroup_size(256)
fn coarse(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    if lane == 0u { atomicStore(&accepted,0u); atomicStore(&has_directional,0u); }
    workgroupBarrier();
    let lo=group.xy*COARSE_TILE_SIZE;
    if globals.light_count>65535u {
        if lane==0u {
            let index=group.y*div_ceil(globals.screen_size,COARSE_TILE_SIZE).x+group.x;
            coarse_grid[index].count=INVALID_LIGHT;
            // Packed IDs cannot represent this population. Conservatively keep
            // the budget scan when the coarse pass bypasses light inspection.
            coarse_grid[index].has_directional=1u;
        }
        return;
    }
    let proposal_index=group.y*div_ceil(globals.screen_size,COARSE_TILE_SIZE).x+group.x;
    let presample=(globals.surface_flags&4u)!=0u;
    if lane==0u {
        key_light=INVALID_LIGHT; active_emitters=0u;
        if presample {
            let stamp=tile_proposals[arrayLength(&tile_proposals)-1u];
            key_light=stamp.key_light; active_emitters=u32(stamp.inverse_probability);
        }
    }
    workgroupBarrier();
    // Sparse SceneDB slots are not active lights. Small active sets take the
    // exact shading path and never read the coarse alias table.
    let build_proposals=presample && active_emitters>32u;
    let center=min(lo+vec2<u32>(COARSE_TILE_SIZE/2u),globals.screen_size-1u);
    var center_position=vec3<f32>(0.0);
    if build_proposals { center_position=world_position(vec2<f32>(center)+0.5,textureLoad(gbuf_depth,vec2<i32>(center),0)); }
    var selected=INVALID_LIGHT; var selected_weight=0.0; var weight_sum=0.0;
    var stratum_weights=array<f32,4>(0.0,0.0,0.0,0.0);
    var rng=hash_u32(proposal_index*256u+lane+globals.frame*0x9e3779b9u);
    // Strata are positions in the live list (k), matching sample.wgsl's
    // conditional draw; the stored and packed IDs are rows (i).
    let live_count=live_light_count();
    for (var k=lane; k<live_count; k+=256u) {
        let i=live_light(k);
        if sphere_in_tile(lights[i],lo,lo+COARSE_TILE_SIZE,0.0,1.0) {
            if build_proposals && i!=key_light {
                let weight=proposal_weight(lights[i],center_position);
                if weight>0.0 {
                    if live_count<=1024u { stratum_weights[(k-lane)/256u]=weight; }
                    weight_sum+=weight;
                    if random(&rng)*weight_sum<weight { selected=i; selected_weight=weight; }
                }
            }
            if lights[i].light_type==0u { atomicStore(&has_directional,1u); }
            // Once overflow is established, the consumer uses the global set.
            // Preserve every light's proposal work without serializing more
            // atomic increments for a count whose exact value is unused.
            if atomicLoad(&accepted)<=COARSE_CAPACITY {
                let slot=atomicAdd(&accepted,1u);
                if slot<COARSE_CAPACITY { packed[slot]=i; }
            }
        }
    }
    if build_proposals {
        proposal_weights[lane]=weight_sum;
        workgroupBarrier();
        if lane==0u {
            var total=0.0;
            for(var i=0u;i<256u;i++) { total+=proposal_weights[i]; }
            proposal_total=total;
            for(var i=0u;i<4u;i++) { atomicStore(&alias_counts[i],0u); }
        }
        workgroupBarrier();
        alias_probabilities[lane]=1.0; alias_indices[lane]=lane;
        proposal_weights[lane]=proposal_weights[lane]*256.0/max(proposal_total,1e-20);
        if proposal_weights[lane]<1.0 { small_aliases[atomicAdd(&alias_counts[0],1u)]=lane; }
        else { large_aliases[atomicAdd(&alias_counts[1],1u)]=lane; }
        workgroupBarrier();
        // Pair disjoint small/large entries in parallel. A bounded number of
        // rounds avoids pathological barrier counts for very skewed weights;
        // the short remainder uses the ordinary exact alias construction.
        for(var round=0u;round<4u;round++) {
            let old=(round&1u)*2u; let next=((round+1u)&1u)*2u;
            let base=(round&1u)*256u; let next_base=((round+1u)&1u)*256u;
            let ns=atomicLoad(&alias_counts[old]); let nl=atomicLoad(&alias_counts[old+1u]);
            let pairs=min(ns,nl);
            if lane==0u { atomicStore(&alias_counts[next],0u); atomicStore(&alias_counts[next+1u],0u); }
            workgroupBarrier();
            if lane<pairs {
                let a=small_aliases[base+lane]; let b=large_aliases[base+lane];
                alias_probabilities[a]=proposal_weights[a]; alias_indices[a]=b;
                proposal_weights[b]=(proposal_weights[b]+proposal_weights[a])-1.0;
                if proposal_weights[b]<1.0 { small_aliases[next_base+atomicAdd(&alias_counts[next],1u)]=b; }
                else { large_aliases[next_base+atomicAdd(&alias_counts[next+1u],1u)]=b; }
            } else {
                if lane<ns { small_aliases[next_base+atomicAdd(&alias_counts[next],1u)]=small_aliases[base+lane]; }
                if lane<nl { large_aliases[next_base+atomicAdd(&alias_counts[next+1u],1u)]=large_aliases[base+lane]; }
            }
            workgroupBarrier();
        }
        if lane==0u {
            var small_count=atomicLoad(&alias_counts[0]); var large_count=atomicLoad(&alias_counts[1]);
            while small_count>0u && large_count>0u {
                small_count--; large_count--;
                let a=small_aliases[small_count]; let b=large_aliases[large_count];
                alias_probabilities[a]=proposal_weights[a]; alias_indices[a]=b;
                proposal_weights[b]=(proposal_weights[b]+proposal_weights[a])-1.0;
                if proposal_weights[b]<1.0 { small_aliases[small_count]=b; small_count++; }
                else { large_aliases[large_count]=b; large_count++; }
            }
        }
        workgroupBarrier();
        tile_proposals[proposal_index*256u+lane]=LightProposal(selected,proposal_total/max(selected_weight,1e-20),alias_indices[lane],alias_probabilities[lane],proposal_total,key_light,stratum_weights);
    } else if presample && lane==0u {
        tile_proposals[proposal_index*256u]=LightProposal(INVALID_LIGHT,0.0,0u,0.0,0.0,key_light,array<f32,4>(0.0,0.0,0.0,0.0));
    }
    workgroupBarrier();
    let index=group.y*div_ceil(globals.screen_size,COARSE_TILE_SIZE).x+group.x;
    let count=atomicLoad(&accepted);
    if lane==0u { coarse_grid[index].count=count; coarse_grid[index].has_directional=atomicLoad(&has_directional); }
    for(var i=lane;i<(min(count,COARSE_CAPACITY)+1u)/2u;i+=256u) {
        coarse_grid[index].indices[i]=packed[2u*i]|(select(65535u,packed[2u*i+1u],2u*i+1u<count)<<16u);
    }
}
@compute @workgroup_size(8,8)
fn fine(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    if lane==0u { atomicStore(&accepted,0u); atomicStore(&min_depth,bitcast<u32>(1.0)); atomicStore(&max_depth,0u); }
    workgroupBarrier();
    let lo=group.xy*TILE_SIZE;
    let p=lo+local.xy;
    if all(p<globals.screen_size) {
        let d=textureLoad(gbuf_depth,vec2<i32>(p),0);
        if !helio_is_sky(d) { atomicMin(&min_depth,bitcast<u32>(d)); atomicMax(&max_depth,bitcast<u32>(d)); }
    }
    workgroupBarrier();
    // Raw NDC depth interval of the tile's surfaces; empty (zlo > zhi) when it
    // is all sky. The screen-space shadow march wants the nearest, which under
    // reversed depth is the largest: zhi, and sky (0) for an empty tile.
    let zlo=bitcast<f32>(atomicLoad(&min_depth));
    let zhi=bitcast<f32>(atomicLoad(&max_depth));
    if lane==0u { textureStore(depth_bounds,vec2<i32>(group.xy),vec4<f32>(zhi)); }
    let ci=(lo.y/COARSE_TILE_SIZE)*div_ceil(globals.screen_size,COARSE_TILE_SIZE).x+lo.x/COARSE_TILE_SIZE;
    let coarse_count=coarse_grid[ci].count;
    let ti=group.y*div_ceil(globals.screen_size,TILE_SIZE).x+group.x;
    // Directional lights intersect every tile. The coarse flag remains valid
    // even when coarse/fine indices overflow and sampling uses the global set.
    if lane==0u { fine_grid[ti].has_directional=coarse_grid[ci].has_directional; }
    if coarse_count>COARSE_CAPACITY {
        // The sampler switches to the complete global set. All lights retain support.
        if lane==0u { fine_grid[ti].count=INVALID_LIGHT; }
        return;
    }
    if zlo<=zhi {
        for(var i=lane;i<coarse_count;i+=64u) {
            let id=(coarse_grid[ci].indices[i/2u]>>(16u*(i&1u)))&65535u;
            if sphere_in_tile(lights[id],lo,lo+TILE_SIZE,zlo,zhi) {
                if atomicLoad(&accepted)<=GRID_CAPACITY {
                    let slot=atomicAdd(&accepted,1u);
                    if slot<GRID_CAPACITY { packed[slot]=id; }
                }
            }
        }
    }
    workgroupBarrier();
    let count=atomicLoad(&accepted);
    if lane==0u { fine_grid[ti].count=count; }
    if count>GRID_CAPACITY { return; }
    // Atomic append gives the correct set but an execution-dependent order.
    // Reservoir strata index this array, so stable IDs are needed for stable
    // lighting when the same camera and lights are rendered twice.
    if lane>=count { packed[lane]=INVALID_LIGHT; }
    workgroupBarrier();
    var sort_length=1u;
    while sort_length<count { sort_length*=2u; }
    var read_packed=true;
    for(var width=2u;width<=sort_length;width*=2u) {
        for(var stride=width/2u;stride>0u;stride/=2u) {
            let partner=lane^stride;
            var value=packed[lane]; var other=packed[partner];
            if !read_packed { value=sorted_fine[lane]; other=sorted_fine[partner]; }
            let lower=select(max(value,other),min(value,other),
                ((lane&width)==0u)==((lane&stride)==0u));
            if read_packed { sorted_fine[lane]=lower; }
            else { packed[lane]=lower; }
            workgroupBarrier();
            read_packed=!read_packed;
        }
    }
    if lane<(min(count,GRID_CAPACITY)+1u)/2u {
        var lo=packed[2u*lane]; var hi=packed[2u*lane+1u];
        if !read_packed { lo=sorted_fine[2u*lane]; hi=sorted_fine[2u*lane+1u]; }
        fine_grid[ti].indices[lane]=lo|(select(65535u,hi,2u*lane+1u<count)<<16u);
    }
}
