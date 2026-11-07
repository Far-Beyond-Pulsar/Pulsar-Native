@group(2) @binding(0) var<storage, read> grid: array<LightTile>;
@group(2) @binding(1) var<storage, read> previous_visible: array<VisibleTile>;
@group(2) @binding(2) var<storage, read_write> next_visible: array<VisibleTile>;
@group(2) @binding(3) var raw_lighting: texture_storage_2d<rg32uint,write>;
@group(2) @binding(4) var previous_geometry: texture_2d<u32>;
@group(2) @binding(5) var screen_depth_bounds: texture_2d<f32>;
@group(2) @binding(6) var<storage,read> tile_proposals: array<LightProposal>;
@group(2) @binding(7) var previous_reservoirs: texture_2d<u32>;
@group(2) @binding(8) var next_reservoirs: texture_storage_2d<rgba32uint,write>;

fn reservoir_pixel(pixel: vec2<i32>, sample: u32) -> vec2<i32> {
    return pixel+vec2<i32>(0,i32(sample*globals.sample_size.y));
}

var<workgroup> seen_ids: array<u32,256>;
var<workgroup> seen_weights: array<f32,256>;
var<workgroup> bucket_weights: array<atomic<u32>,64>;
var<workgroup> bucket_ids: array<atomic<u32>,64>;
var<workgroup> slot_ids: array<u32,64>;
var<workgroup> slot_weights: array<f32,64>;
var<workgroup> sorted_ids: array<u32,16>;
var<workgroup> confidence_bits: array<atomic<u32>,2>;

struct Reservoir { selected: u32, importance_value: f32, weight_sum: f32, random_value: f32, }
fn reservoir_add(r: ptr<function, Reservoir>, id: u32, importance_value: f32, weight: f32) {
    if weight<=0.0 { return; }
    (*r).weight_sum+=weight;
    let probability=weight/(*r).weight_sum;
    // Warp the selected/rejected interval back to [0,1), preserving STBN.
    if (*r).random_value<probability {
        (*r).selected=id; (*r).importance_value=importance_value;
        (*r).random_value/=max(probability,1e-20);
    } else {
        (*r).random_value=((*r).random_value-probability)/max(1.0-probability,1e-20);
    }
}

fn record_visible(id: u32, s: Surface, index: u32) {
    if USE_TILE_PRESAMPLING { return; }
    let weight=importance(id,s);
    seen_ids[index]=id;
    seen_weights[index]=weight;
    // Positive IEEE float bits preserve order; normalize signed zero.
    if weight>=0.0 { atomicMax(&bucket_weights[hash_u32(id)&63u],select(bitcast<u32>(weight),0u,weight==0.0)); }
}

fn trace_visibility(id: u32, surface: Surface, pixel: vec2<u32>) -> Visibility {
    return shadow_factor(id,surface.position,surface.normal,vec2<f32>(pixel)+0.5,globals.frame);
}

fn guided(tile: u32, count: u32, id: u32) -> bool {
    var lo=0u; var hi=count;
    while lo<hi {
        let mid=(lo+hi)/2u;
        if previous_visible[tile].indices[mid]<id { lo=mid+1u; } else { hi=mid; }
    }
    return lo<count && previous_visible[tile].indices[lo]==id;
}

fn sampled_proposal(id: u32, inverse_probability: f32) -> LightProposal {
    return LightProposal(id,inverse_probability,0u,0.0,0.0,INVALID_LIGHT,
        array<f32,4>(0.0,0.0,0.0,0.0));
}

// For at most 1,024 allocated lights, each coarse lane stores the exact
// weights of its four-light stratum. The alias table chooses a stratum, then
// a conditional draw chooses a light. Larger sets use the lane reservoir.
fn discovery_proposal(pixel: vec2<u32>, tile: u32, population: u32, overflow: bool,
    candidate: u32, candidate_count: u32, rng: ptr<function,u32>, use_tile: bool) -> LightProposal {
    // Score a compact local population exhaustively before spending the fixed
    // shadow-ray budget. A tiny random candidate set otherwise adds avoidable
    // variance even when only a few lights can contribute to this surface.
    if USE_TILE_PRESAMPLING && population<=32u {
        // An overflowing tile's population is the global live list.
        var id=INVALID_LIGHT;
        if overflow { id=live_light(candidate); }
        else { id=(grid[tile].indices[candidate/2u]>>(16u*(candidate&1u)))&65535u; }
        if id==tile_proposals[0].key_light { return sampled_proposal(INVALID_LIGHT,0.0); }
        return sampled_proposal(id,f32(population));
    }
    // Narrow glossy lobes bypass the shared reservoir pool: independent
    // per-pixel discovery prevents tile-wide errors when a bright light moves.
    if USE_TILE_PRESAMPLING && use_tile && globals.light_count<=65535u {
        let coarse=(pixel.y/COARSE_TILE_SIZE)*div_ceil(globals.screen_size,COARSE_TILE_SIZE).x+pixel.x/COARSE_TILE_SIZE;
        let roll=(f32(candidate)+random(rng))/f32(candidate_count);
        // Reserve a uniform component so floating-point proposal tables cannot
        // remove support for dim lights or a poorly represented receiver.
        // Uniform over live lights, not allocated rows: vacant rows carry no
        // light, so the estimator is unchanged and no sample is wasted on them.
        let live_count=live_light_count();
        if live_count==0u { return sampled_proposal(INVALID_LIGHT,0.0); }
        let uniform_fraction=0.0625;
        let uniform_pdf=uniform_fraction/f32(live_count);
        if roll<uniform_fraction {
            let k=min(u32(roll/uniform_fraction*f32(live_count)),live_count-1u);
            let id=live_light(k);
            if id==tile_proposals[coarse*256u].key_light { return sampled_proposal(INVALID_LIGHT,0.0); }
            let total=tile_proposals[coarse*256u].total_weight;
            var weight=0.0;
            if live_count<=1024u {
                weight=tile_proposals[coarse*256u+k%256u].weights[k/256u];
            } else {
                let center=min((pixel/COARSE_TILE_SIZE)*COARSE_TILE_SIZE+vec2<u32>(COARSE_TILE_SIZE/2u),globals.screen_size-1u);
                let center_position=world_position(vec2<f32>(center)+0.5,textureLoad(gbuf_depth,vec2<i32>(center),0));
                weight=proposal_weight(lights[id],center_position);
            }
            let pdf=(1.0-uniform_fraction)*weight/max(total,1e-20)+uniform_pdf;
            return sampled_proposal(id,1.0/pdf);
        }
        let scaled=(roll-uniform_fraction)/(1.0-uniform_fraction)*256.0;
        let slot=min(u32(scaled),255u);
        let entry=tile_proposals[coarse*256u+slot];
        let index=select(slot,entry.alias_index,fract(scaled)>=entry.alias_probability);
        let proposal=tile_proposals[coarse*256u+index];
        if live_count<=1024u {
            let weights=proposal.weights;
            let group_weight=weights[0]+weights[1]+weights[2]+weights[3];
            if group_weight<=0.0 { return sampled_proposal(INVALID_LIGHT,0.0); }
            let roll_in_group=random(rng)*group_weight;
            var member=0u;
            if roll_in_group>=weights[0] { member=1u; }
            if roll_in_group>=weights[0]+weights[1] { member=2u; }
            if roll_in_group>=weights[0]+weights[1]+weights[2] { member=3u; }
            let k=index+256u*member;
            if k>=live_count { return sampled_proposal(INVALID_LIGHT,0.0); }
            let id=live_light(k);
            if id==proposal.key_light { return sampled_proposal(INVALID_LIGHT,0.0); }
            let pdf=(1.0-uniform_fraction)*weights[member]/max(proposal.total_weight,1e-20)+uniform_pdf;
            return sampled_proposal(id,1.0/pdf);
        }
        if proposal.id==INVALID_LIGHT { return proposal; }
        let pdf=(1.0-uniform_fraction)/proposal.inverse_probability+uniform_pdf;
        return sampled_proposal(proposal.id,1.0/pdf);
    }
    let pick=min(u32((f32(candidate)+random(rng))*(f32(population)/f32(candidate_count))),population-1u);
    var id=INVALID_LIGHT;
    if overflow { id=live_light(pick); }
    else { id=(grid[tile].indices[pick/2u]>>(16u*(pick&1u)))&65535u; }
    if USE_TILE_PRESAMPLING && globals.light_count<=65535u {
        if id==tile_proposals[0].key_light { id=INVALID_LIGHT; }
    }
    return sampled_proposal(id,f32(population));
}

@compute @workgroup_size(8,8)
fn sample_lights(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    for(var i=0u;i<4u;i++) { seen_ids[i*64u+lane]=INVALID_LIGHT; seen_weights[i*64u+lane]=0.0; }
    atomicStore(&bucket_weights[lane],0u);
    atomicStore(&bucket_ids[lane],INVALID_LIGHT);
    if lane==0u { atomicStore(&confidence_bits[0],0u); atomicStore(&confidence_bits[1],0u); }
    workgroupBarrier();
    if all(gid.xy<globals.sample_size) {
        let pixel=sample_pixel(gid.xy,globals.frame);
        let reuse=(globals.surface_flags&8u)!=0u && globals.light_count<=65535u;
        if reuse {
            for(var sample=0u;sample<globals.sample_count;sample++) {
                textureStore(next_reservoirs,reservoir_pixel(vec2<i32>(gid.xy),sample),vec4<u32>(65535u,0u,0u,0u));
            }
        }
        var result=Lighting(vec3<f32>(0.0),vec3<f32>(0.0));
        var confidence=1.0;
        if !helio_is_sky(textureLoad(gbuf_depth,vec2<i32>(pixel),0)) && globals.light_count>0u {
            let s=surface_at(pixel);
            let pixel_sample_count=select(globals.sample_count,4u,USE_TILE_PRESAMPLING && s.roughness<0.2);
            let tile=(pixel.y/TILE_SIZE)*div_ceil(globals.screen_size,TILE_SIZE).x+pixel.x/TILE_SIZE;
            let grid_count=grid[tile].count;
            let overflow=grid_count>GRID_CAPACITY;
            let population=select(grid_count,live_light_count(),overflow);
            if population<=pixel_sample_count || (USE_RAY_TRANSMISSION && population<=32u) || globals.debug_mode==1u {
                // Exact path for small sets, and an uncapped oracle for GPU regression tests.
                // Transmitting sheets create sharp chromatic visibility changes;
                // use exact local sets up to 32 lights at the shading resolution.
                // This is a separate quality/cost tier from the opaque two-ray path.
                let n=select(population,live_light_count(),globals.debug_mode==1u);
                let origin=shadow_receiver(s.position,s.normal,vec2<f32>(pixel)+0.5);
                for(var i=0u;i<n;i++) {
                    var id=INVALID_LIGHT;
                    if overflow || globals.debug_mode==1u { id=live_light(i); }
                    else { id=((grid[tile].indices[i/2u]>>(16u*(i&1u)))&65535u); }
                    if USE_TILE_PRESAMPLING && globals.light_count<=65535u && id==tile_proposals[0].key_light { continue; }
                    if USE_RAY_TRANSMISSION {
                        if !can_illuminate(id,s) { continue; }
                    } else if importance(id,s)<=0.0 { continue; }
                    let vis=shadow_factor_from_receiver(id,origin,s.position,s.normal,vec2<f32>(pixel)+0.5,globals.frame);
                    let light=evaluate_light(id,s,vis);
                    result.diffuse+=light.diffuse; result.specular+=light.specular;
                    if visibility_nonzero(vis) && i<4u { record_visible(id,s,i*64u+lane); }
                }
            } else {
                confidence=0.0;
                let prev_uv=previous_uv(pixel,s.position);
                let valid_uv=all(prev_uv>=vec2<f32>(0.0)) && all(prev_uv<vec2<f32>(1.0));
                let prev_pixel=vec2<i32>(sample_position_from_uv(prev_uv));
                let previous_z=-(globals.previous_view*vec4<f32>(s.position,1.0)).z;
                var valid_history=false;
                if globals.history_valid!=0u && valid_uv {
                    valid_history=geometry_matches(load_geometry(previous_geometry,prev_pixel),s.normal,previous_z);
                }
                // Stochastic bilinear tile lookup. Adjacent tiles share support at borders.
                let tile_dims=div_ceil(globals.sample_size,TILE_SIZE);
                let tile_f=sample_position_from_uv(prev_uv)/f32(TILE_SIZE)-0.5;
                let tile_jitter=vec2<f32>(stbn(gid.xy,0u),stbn(gid.xy,1u));
                let guide_xy=vec2<u32>(clamp(floor(tile_f+tile_jitter),vec2<f32>(0.0),vec2<f32>(tile_dims-1u)));
                let guide_tile=guide_xy.y*tile_dims.x+guide_xy.x;
                var guide_count=0u;
                if globals.history_valid!=0u && !USE_TILE_PRESAMPLING { guide_count=min(previous_visible[guide_tile].count,VISIBLE_CAPACITY); }
                let hidden_fraction=select(0.5,globals.discovery_fraction,valid_history);
                let surface_candidates=select(globals.candidate_count,max(globals.candidate_count,8u),USE_TILE_PRESAMPLING && s.roughness<0.2);
                let candidate_count=select(select(min(surface_candidates*2u,16u),surface_candidates,valid_history),population,USE_TILE_PRESAMPLING && population<=32u);
                var rng=hash_u32(pixel.x+pixel.y*globals.screen_size.x+globals.frame*0x9e3779b9u);
                var guide_energy=0.0;
                var covered_energy=0.0;
                var selected_ids=vec4<u32>(INVALID_LIGHT);
                var selected_normalization=vec4<f32>(0.0);
                var traced_visibility: VisibilityCache;
                // The guide and surface are shared by every sample. Preserve
                // their accumulation order without rescoring the same lights.
                var guided_directional_weight=0.0; var guided_local_weight=0.0;
                // Directional lights intersect every coarse tile, including
                // overflow tiles. With none, guide budget scores are unused too.
                if grid[tile].has_directional!=0u {
                    for(var i=0u;i<guide_count;i++) {
                        let id=previous_visible[guide_tile].indices[i];
                        let proxy=importance(id,s);
                        if lights[id].light_type==0u { guided_directional_weight+=proxy; }
                        else { guided_local_weight+=proxy; }
                    }
                }
                // Local-light guide weights are identical for every sample.
                // Advance their stratified reservoirs together, scoring each
                // guide entry once without a private or workgroup score array.
                let local_guide=grid[tile].has_directional==0u && pixel_sample_count>1u;
                var guide_ids=vec4<u32>(INVALID_LIGHT);
                var guide_importance=vec4<f32>(0.0);
                var guide_sum=0.0;
                var guide_random=(vec4<f32>(0.0,1.0,2.0,3.0)+stbn(gid.xy,2u))/f32(pixel_sample_count);
                if local_guide {
                    for(var i=0u;i<guide_count;i++) {
                        let id=previous_visible[guide_tile].indices[i];
                        let proxy=importance(id,s);
                        if proxy<=0.0 { continue; }
                        guide_sum+=proxy;
                        let probability=proxy/guide_sum;
                        let accepted=guide_random<vec4<f32>(probability);
                        guide_ids=select(guide_ids,vec4<u32>(id),accepted);
                        guide_importance=select(guide_importance,vec4<f32>(proxy),accepted);
                        guide_random=select((guide_random-probability)/max(1.0-probability,1e-20),
                            guide_random/max(probability,1e-20),accepted);
                    }
                }
                for(var sample=0u;sample<pixel_sample_count;sample++) {
                    // Replay proposals after computing the directional budget.
                    // Streaming both scans keeps candidates out of private arrays.
                    let candidate_seed=rng;
                    var directional_weight=guided_directional_weight; var local_weight=guided_local_weight;
                    // With no directional proposals the scale is exactly one.
                    // Keep private arrays out of the shader and advance the RNG
                    // through the reservoir scan below when this scan is skipped.
                    if grid[tile].has_directional!=0u || guided_directional_weight>0.0 {
                        for(var candidate=0u;candidate<candidate_count;candidate++) {
                            let proposal=discovery_proposal(pixel,tile,population,overflow,candidate,candidate_count,&rng,true);
                            let id=proposal.id;
                            if id==INVALID_LIGHT { continue; }
                            let inverse_proposal=proposal.inverse_probability/f32(candidate_count);
                            var proxy=importance(id,s);
                            if guided(guide_tile,guide_count,id) { proxy=0.0; }
                            if lights[id].light_type==0u { directional_weight+=proxy*inverse_proposal; }
                            else { local_weight+=proxy*inverse_proposal; }
                        }
                    }
                    var directional_scale=1.0;
                    if local_weight>1e-5 && directional_weight>0.0 { directional_scale=min(1.0,0.5*local_weight/directional_weight); }
                    var visible_reservoir=Reservoir(INVALID_LIGHT,0.0,0.0,(f32(sample)+stbn(gid.xy,2u))/f32(pixel_sample_count));
                    var hidden_reservoir=Reservoir(INVALID_LIGHT,0.0,0.0,
                        select((f32(sample)+stbn(gid.xy,3u))/f32(pixel_sample_count),stbn(gid.xy,3u+sample*7u),reuse));
                    if local_guide {
                        visible_reservoir=Reservoir(guide_ids[sample],guide_importance[sample],guide_sum,guide_random[sample]);
                    } else {
                        for(var i=0u;i<guide_count;i++) {
                            let id=previous_visible[guide_tile].indices[i];
                            var proxy=importance(id,s);
                            if lights[id].light_type==0u { proxy*=directional_scale; }
                            reservoir_add(&visible_reservoir,id,proxy,proxy);
                        }
                    }
                    var replay=candidate_seed;
                    for(var candidate=0u;candidate<candidate_count;candidate++) {
                        let proposal=discovery_proposal(pixel,tile,population,overflow,candidate,candidate_count,&replay,true);
                        let id=proposal.id;
                        if id==INVALID_LIGHT { continue; }
                        let inverse_proposal=proposal.inverse_probability/f32(candidate_count);
                        var proxy=importance(id,s);
                        if guided(guide_tile,guide_count,id) { proxy=0.0; }
                        if lights[id].light_type==0u { proxy*=directional_scale; }
                        reservoir_add(&hidden_reservoir,id,proxy,proxy*inverse_proposal);
                    }
                    rng=replay;
                    let v=visible_reservoir.weight_sum; let h=hidden_reservoir.weight_sum;
                    var hidden_budget=h;
                    // Cap hidden selection weight to 20% (50% on disocclusion),
                    // relaxing when there is no useful visible history.
                    if v>1e-5 && hidden_fraction<1.0 { hidden_budget=min(h,v*hidden_fraction/(1.0-hidden_fraction)); }
                    let p_hidden=hidden_budget/max(v+hidden_budget,1e-20);
                    var chosen=visible_reservoir; var group_probability=1.0-p_hidden;
                    // Repeated reservoir warps distort the finite noise-rank
                    // distribution. Each sample needs a separate full-range
                    // group draw, independent of the selected reservoir stratum.
                    let group_roll=stbn(gid.xy,28u+sample);
                    if group_roll<p_hidden { chosen=hidden_reservoir; group_probability=p_hidden; }
                    var selected=chosen.selected;
                    var normalization=chosen.weight_sum/max(chosen.importance_value*group_probability,1e-20);
                    if reuse && sample<globals.sample_count {
                        var target_weight=0.0;
                        if selected!=INVALID_LIGHT { target_weight=importance(selected,s); }
                        var merged=Reservoir(selected,target_weight,normalization*target_weight,stbn(gid.xy,40u+sample));
                        // Each fresh RIS estimate is one effective sample. Cap
                        // history at seven; do not reuse previous visibility.
                        // Packed record: ID16 | M16, inverse-PDF weight, stamp64.
                        var count=1.0;
                        let key=tile_proposals[0].key_light;
                        let stamp=tile_proposals[arrayLength(&tile_proposals)-1u];
                        if valid_history {
                            let prior=textureLoad(previous_reservoirs,reservoir_pixel(prev_pixel,sample),0);
                            let old_count=f32(min(prior.x>>16u,7u));
                            let prior_id=prior.x&65535u;
                            if old_count>0.0 && prior.z==stamp.id && prior.w==stamp.alias_index {
                                count+=old_count;
                                if prior_id<globals.light_count && prior_id!=key {
                                    let target_weight=importance(prior_id,s);
                                    reservoir_add(&merged,prior_id,target_weight,target_weight*bitcast<f32>(prior.y)*old_count);
                                }
                            }
                        }
                        selected=merged.selected;
                        normalization=merged.weight_sum/max(merged.importance_value*count,1e-20);
                        textureStore(next_reservoirs,reservoir_pixel(vec2<i32>(gid.xy),sample),
                            vec4<u32>((selected&65535u)|(u32(count)<<16u),bitcast<u32>(normalization),stamp.id,stamp.alias_index));
                    }
                    if selected==INVALID_LIGHT { continue; }
                    if reuse { normalization/=f32(pixel_sample_count); }
                    else { normalization=chosen.weight_sum/max(chosen.importance_value*group_probability*f32(pixel_sample_count),1e-20); }
                    let mask=vec4<u32>(0u,1u,2u,3u)==vec4<u32>(sample);
                    selected_ids=select(selected_ids,vec4<u32>(selected),mask);
                    selected_normalization=select(selected_normalization,vec4<f32>(normalization),mask);
                }
                // Finish selection before traversal so reservoir state need not
                // remain live across hardware ray-query operations.
                for(var sample=0u;sample<pixel_sample_count;sample++) {
                    let selected=selected_ids[sample];
                    if selected==INVALID_LIGHT { continue; }
                    var vis=Visibility(-1.0);
                    if sample>0u && selected_ids.x==selected { vis=traced_visibility[0]; }
                    else if sample>1u && selected_ids.y==selected { vis=traced_visibility[1]; }
                    else if sample>2u && selected_ids.z==selected { vis=traced_visibility[2]; }
                    let first_trace=visibility_missing(vis);
                    if first_trace { vis=trace_visibility(selected,s,pixel); }
                    traced_visibility[sample]=vis;
                    let normalization=selected_normalization[sample];
                    let light=evaluate_light(selected,s,vis);
                    if first_trace && guided(guide_tile,guide_count,selected) {
                        covered_energy+=luminance(light.diffuse*s.albedo+light.specular*s.specular_factor);
                    }
                    result.diffuse+=light.diffuse*normalization;
                    result.specular+=light.specular*normalization;
                    if visibility_nonzero(vis) { record_visible(selected,s,sample*64u+lane); }
                }
                // Only the >= 0.8 decision is stored. Once a nonnegative
                // partial sum makes the ratio smaller, remaining terms cannot
                // restore confidence. HDR albedo can make diffuse terms negative,
                // so preserve the full sum for that input range.
                if valid_history && covered_energy>0.0 {
                    for(var i=0u;i<guide_count;i++) {
                        let potential=evaluate_light(previous_visible[guide_tile].indices[i],s,Visibility(1.0));
                        guide_energy+=luminance(potential.diffuse*s.albedo+potential.specular*s.specular_factor);
                        if all(s.albedo<=vec3<f32>(1.0)) && guide_energy>0.00001
                            && covered_energy/guide_energy<0.8 { break; }
                    }
                    if guide_energy>0.00001 { confidence=clamp(covered_energy/guide_energy,0.0,1.0); }
                }
            }
        }
        if confidence>=0.8 { atomicOr(&confidence_bits[lane/32u],1u<<(lane%32u)); }
        textureStore(raw_lighting,vec2<i32>(gid.xy),vec4<u32>(pack_radiance(result.diffuse,gid.xy,4u).x,pack_radiance(result.specular,gid.xy,7u).x,0u,0u));
    }
    workgroupBarrier();
    let output_tile=group.y*div_ceil(globals.sample_size,TILE_SIZE).x+group.x;
    if USE_TILE_PRESAMPLING {
        if lane==0u {
            var key=INVALID_LIGHT;
            if globals.light_count<=65535u { key=tile_proposals[0].key_light; }
            next_visible[output_tile].count=select(0u,1u,key!=previous_visible[output_tile].indices[0]);
            next_visible[output_tile].indices[0]=key;
            next_visible[output_tile].confidence_low=atomicLoad(&confidence_bits[0]);
            next_visible[output_tile].confidence_high=atomicLoad(&confidence_bits[1]);
        }
        return;
    }
    // Parallel deduplication keeps one explicit ID in each of 64 scratch slots.
    // Visibility is binary: penumbra hits rank by unoccluded importance too.
    // The greatest weight is already reduced while recording visibility.
    // A second exact reduction chooses the lowest ID among equal weights.
    for(var sample=0u;sample<select(globals.sample_count,4u,globals.debug_mode==1u);sample++) {
        let index=sample*64u+lane;
        let id=seen_ids[index];
        let weight=seen_weights[index];
        if id!=INVALID_LIGHT && weight>=0.0 {
            let bucket=hash_u32(id)&63u;
            let bits=select(bitcast<u32>(weight),0u,weight==0.0);
            if bits==atomicLoad(&bucket_weights[bucket]) { atomicMin(&bucket_ids[bucket],id); }
        }
    }
    workgroupBarrier();
    var best=atomicLoad(&bucket_ids[lane]);
    let weight=bitcast<f32>(atomicLoad(&bucket_weights[lane]));
    slot_ids[lane]=best; slot_weights[lane]=weight;
    if lane<VISIBLE_CAPACITY { sorted_ids[lane]=INVALID_LIGHT; }
    workgroupBarrier();
    // Rank scratch slots by importance, retaining at most 16. Collisions and
    // capacity eviction leave lights discoverable through the hidden reservoir.
    var rank=0u;
    for(var i=0u;i<64u;i++) {
        if slot_weights[i]>weight || (slot_weights[i]==weight && slot_ids[i]<best) { rank++; }
    }
    // Rank is unique for valid IDs, so only the retained 16 slots need
    // numerical sorting. Keep scratch reads separate from output writes.
    if rank<VISIBLE_CAPACITY && best!=INVALID_LIGHT { sorted_ids[rank]=best; }
    workgroupBarrier();
    if lane<VISIBLE_CAPACITY {
        best=sorted_ids[lane];
        var sorted_rank=0u; var count=0u;
        for(var i=0u;i<VISIBLE_CAPACITY;i++) {
            if sorted_ids[i]<best { sorted_rank++; }
            if sorted_ids[i]!=INVALID_LIGHT { count++; }
        }
        if best!=INVALID_LIGHT { next_visible[output_tile].indices[sorted_rank]=best; }
        else { next_visible[output_tile].indices[lane]=INVALID_LIGHT; }
        if lane==0u {
            next_visible[output_tile].count=count;
            next_visible[output_tile].confidence_low=atomicLoad(&confidence_bits[0]);
            next_visible[output_tile].confidence_high=atomicLoad(&confidence_bits[1]);
        }
    }
}


// Globally small populations need neither reservoirs nor shared ID sorting.
@compute @workgroup_size(8,8)
fn sample_small(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    if lane==0u {
        let tile=group.y*div_ceil(globals.sample_size,TILE_SIZE).x+group.x;
        next_visible[tile].count=0u;
        if USE_TILE_PRESAMPLING {
            let key=tile_proposals[0].key_light;
            next_visible[tile].count=select(0u,1u,key!=previous_visible[tile].indices[0]);
            next_visible[tile].indices[0]=key;
        }
        next_visible[tile].confidence_low=0xffffffffu;
        next_visible[tile].confidence_high=0xffffffffu;
    }
    if any(gid.xy>=globals.sample_size) { return; }
    let pixel=sample_pixel(gid.xy,globals.frame);
    if (globals.surface_flags&8u)!=0u {
        for(var sample=0u;sample<globals.sample_count;sample++) {
            textureStore(next_reservoirs,reservoir_pixel(vec2<i32>(gid.xy),sample),vec4<u32>(65535u,0u,0u,0u));
        }
    }
    var result=Lighting(vec3<f32>(0.0),vec3<f32>(0.0));
    if !helio_is_sky(textureLoad(gbuf_depth,vec2<i32>(pixel),0)) {
        let s=surface_at(pixel);
        for(var k=0u;k<live_light_count();k++) {
            let id=live_light(k);
            if USE_TILE_PRESAMPLING && id==tile_proposals[0].key_light { continue; }
            if importance(id,s)<=0.0 { continue; }
            let light=evaluate_light(id,s,trace_visibility(id,s,pixel));
            result.diffuse+=light.diffuse; result.specular+=light.specular;
        }
    }
    textureStore(raw_lighting,vec2<i32>(gid.xy),vec4<u32>(pack_radiance(result.diffuse,gid.xy,4u).x,pack_radiance(result.specular,gid.xy,7u).x,0u,0u));
}
