// GPU view scoring, bounded selection, persistent tile allocation and commit.
struct GpuLight {
    position_range:   vec4f,
    direction_outer:  vec4f,
    color_intensity:  vec4f,
    shadow_index:     u32,
    light_type:       u32,
    inner_angle:      f32,
    _pad:             u32,
    god_rays_enabled:  u32,
    god_rays_density:  f32,
    god_rays_weight:   f32,
    god_rays_decay:    f32,
    god_rays_exposure: f32,
    flare_enabled:      u32,
    flare_type:         u32,
    flare_intensity:    f32,
    flare_scale:        f32,
    flare_tint_r:       f32,
    flare_tint_g:       f32,
    flare_tint_b:       f32,
    ies_profile_index:    i32,
    light_function_index: i32,
    ies_angle_scale:      f32,
    ies_angle_offset:     f32,
}


struct Params {
    rows:u32, capacity:u32, nonce:u32, atlas_size:u32,
    view_proj:mat4x4f, inv_view_proj:mat4x4f,
    camera:vec4f, // xyz position, w max shadow distance
    tuning:vec4f, // viewport height, max resolution, hysteresis, unused
}
struct Candidate { key:u32, resolution:u32, mask:u32, fade:f32 }
struct Resident {
    owner:u32, kind:u32, flags:u32, fade_target:u32,
    score:f32, strength:f32, resolution:u32, hash:u32,
    tiles:array<vec4u,6>,
}
struct Table { header:vec4u, slots:array<Resident,256> }
@group(0) @binding(0) var<storage,read_write> lights:array<GpuLight>;
@group(0) @binding(1) var<uniform> params:Params;
@group(0) @binding(2) var<storage,read_write> proposed:Table;
@group(0) @binding(3) var<storage,read> committed:Table;
@group(0) @binding(4) var<storage,read_write> candidates:array<Candidate>;
const NONE:u32=0xffffffffu;
fn requested(l:GpuLight)->bool {
    return select(l.shadow_index!=NONE,(l._pad&4u)!=0u,(l._pad&8u)!=0u);
}
fn plane_outside(p:vec4f,c:vec3f,r:f32)->bool { return dot(p,vec4f(c,1.0)) < -r*length(p.xyz); }
// Shadow lights are chosen for a view slightly wider than the screen, so a light
// that is about to turn into view already has its map when it arrives.
const VIEW_MARGIN:f32=1.6;
fn visible(l:GpuLight)->bool {
    let m=transpose(params.view_proj);
    let c=l.position_range.xyz;
    let r=max(l.position_range.w,0.0);
    return !(plane_outside(m[3]*VIEW_MARGIN+m[0],c,r)||plane_outside(m[3]*VIEW_MARGIN-m[0],c,r)
        ||plane_outside(m[3]*VIEW_MARGIN+m[1],c,r)||plane_outside(m[3]*VIEW_MARGIN-m[1],c,r)
        ||plane_outside(m[2],c,r)||plane_outside(m[3]-m[2],c,r));
}
var<workgroup> frustum_corners:array<vec3f,8>;
fn receiver_corner(c:u32)->vec3f {
    let clip=vec4f(select(-VIEW_MARGIN,VIEW_MARGIN,(c&1u)!=0u),select(-VIEW_MARGIN,VIEW_MARGIN,(c&2u)!=0u),select(0.0,1.0,(c&4u)!=0u),1.0);
    let h=params.inv_view_proj*clip;
    let far=params.camera.xyz+normalize(h.xyz-h.w*params.camera.xyz)*params.camera.w;
    return select(far,h.xyz/h.w,abs(h.w)>1e-6);
}
fn point_mask(pos:vec3f)->u32 {
    // Conservative SAT: reject a cube-face pyramid only when a plane
    // separates all eight receiver-frustum vertices from that pyramid.
    // The camera projection is reversed with no far plane, so the clip z = 0
    // vertices are points at infinity (w = 0): walk their rays out to the
    // max shadow distance, where receivers stop mattering.
    var dirs=array<vec3f,6>(vec3f(1,0,0),vec3f(-1,0,0),vec3f(0,1,0),vec3f(0,-1,0),vec3f(0,0,1),vec3f(0,0,-1));
    var mask=0u;
    for(var f=0u;f<6u;f++) {
        let d=dirs[f];
        let a=select(vec3f(1,0,0),vec3f(0,1,0),f<2u);
        let b=cross(d,a);
        var planes=array<vec3f,5>(d,d+a,d-a,d+b,d-b);
        var keep=true;
        for(var p=0u;p<5u;p++) {
            var maximum=-3.0e38;
            for(var c=0u;c<8u;c++) {
                maximum=max(maximum,dot(planes[p],frustum_corners[c]-pos));
            }
            if maximum < -0.001 { keep=false; }
        }
        if keep { mask|=1u<<f; }
    }
    return mask;
}
@compute @workgroup_size(64)
fn score_lights(@builtin(global_invocation_id) gid:vec3u,@builtin(local_invocation_index) lid:u32) {
    // The receiver frustum is the same for every light: unproject its corners once per workgroup.
    if lid<8u { frustum_corners[lid]=receiver_corner(lid); }
    workgroupBarrier();
    let i=gid.x;
    if i>=min(params.rows,arrayLength(&lights)) { return; }
    candidates[i]=Candidate(0u,0u,0u,0.0);
    let l=lights[i];
    if !(l.color_intensity.w>0.0)||!requested(l)||(l._pad&48u)==48u { return; }
    let incumbent=(l._pad&8u)!=0u && l.shadow_index!=NONE && l.shadow_index/6u<params.capacity;
    // An incumbent that left the (widened) view keeps its tiles at the lowest priority
    // while the pool has room, instead of being dropped and re-rendered on return.
    var offscreen=false;
    if l.light_type!=0u&&!visible(l) { if !incumbent { return; } offscreen=true; }
    let distance=length(l.position_range.xyz-params.camera.xyz);
    let fade=select(1.0-smoothstep(params.camera.w*0.8,params.camera.w,max(0.0,distance-l.position_range.w)),1.0,l.light_type==0u);
    if fade<=0.0 { return; }
    let clip=params.view_proj*vec4f(l.position_range.xyz,1);
    let rows=transpose(params.view_proj);
    let projection_scale=max(length(rows[0].xyz),length(rows[1].xyz));
    let radius=projection_scale*max(l.position_range.w,0.0)/max(abs(clip.w),0.01);
    let coverage=select(clamp(radius*radius,0.000001,1.0),1.0,l.light_type==0u);
    let priority=select(1.0,f32((l._pad>>8u)&255u)/16.0,((l._pad>>8u)&255u)!=0u);
    var score=coverage*fade*priority;
    score*=select(1.0,1.0+params.tuning.z,incumbent);
    let max_log=(l._pad>>16u)&15u;
    let limit=min(u32(params.tuning.y),select(2048u,1u<<max_log,max_log!=0u));
    let pixels=sqrt(coverage)*params.tuning.x;
    var res=128u;
    for(var t=0u;t<4u;t++) { if f32(res)<pixels && res*2u<=limit { res*=2u; } }
    // Schmitt thresholds around the incumbent tier prevent camera jitter from resizing.
    if incumbent {
        let old=committed.slots[l.shadow_index/6u].resolution;
        if old>=128u && old<=limit && pixels<=f32(old)*(1.0+params.tuning.z) && pixels>=f32(old/2u)*(1.0-params.tuning.z) {res=old;}
    }
    var mask=1u;
    if l.light_type==0u { mask=15u; }
    if l.light_type==1u { mask=point_mask(l.position_range.xyz); }
    if mask==0u { if !incumbent { return; } offscreen=true; }
    if offscreen {
        let held=committed.slots[l.shadow_index/6u];
        mask=0u;
        for(var f=0u;f<6u;f++){ if held.tiles[f].z!=0u { mask|=1u<<f; } }
        if mask==0u { return; }
        res=held.resolution;
        score*=0.001;
    }
    candidates[i]=Candidate(bitcast<u32>(score),res,mask,fade);
}
var<workgroup> histogram:array<atomic<u32>,256>;
var<workgroup> offsets:array<u32,256>;
var<workgroup> threshold:u32;
var<workgroup> threshold_mask:u32;
var<workgroup> needed:u32;
var<workgroup> take_all:u32;
fn scan(lid:u32)->u32 {
    workgroupBarrier();
    if lid==0u { var sum=0u; for(var j=0u;j<256u;j++){let n=offsets[j];offsets[j]=sum;sum+=n;} }
    workgroupBarrier();
    return offsets[lid];
}
@compute @workgroup_size(256)
fn select_lights(@builtin(local_invocation_index) lid:u32) {
    let n=min(params.rows,arrayLength(&candidates));
    let chunk=(n+255u)/256u;
    let begin=min(lid*chunk,n); let end=min(begin+chunk,n);
    if lid==0u { threshold=0u;threshold_mask=0u;needed=params.capacity;take_all=0u; }
    for(var digit=0u;digit<4u;digit++) {
        let shift=24u-8u*digit;
        atomicStore(&histogram[lid],0u);workgroupBarrier();
        let prefix=threshold;let mask=threshold_mask;
        for(var i=begin;i<end;i++) { let k=candidates[i].key;if k!=0u&&(k&mask)==prefix { atomicAdd(&histogram[(k>>shift)&255u],1u); } }
        workgroupBarrier();
        if lid==0u&&take_all==0u {
            if digit==0u { var total=0u;for(var b=0u;b<256u;b++){total+=atomicLoad(&histogram[b]);} if total<=needed {take_all=1u;} }
            if take_all==0u {
                var above=0u;var bucket=255u;
                for(var b=0u;b<256u;b++){bucket=255u-b;let count=atomicLoad(&histogram[bucket]);if above+count>=needed {break;} above+=count;}
                needed-=min(above,needed);threshold|=bucket<<shift;threshold_mask|=255u<<shift;
            }
        }
        workgroupBarrier();
    }
    var ties=0u;
    for(var i=begin;i<end;i++){if candidates[i].key!=0u&&candidates[i].key==threshold{ties++;}}
    offsets[lid]=ties;let tie_base=scan(lid);
    var tie_rank=tie_base;var winners=0u;
    for(var i=begin;i<end;i++) {
        let k=candidates[i].key;if k==0u{continue;}
        var win=take_all!=0u||k>threshold;
        if !win&&k==threshold {win=tie_rank<needed;tie_rank++;}
        if win {winners++;}
    }
    workgroupBarrier();offsets[lid]=winners;var slot=scan(lid);tie_rank=tie_base;
    for(var i=begin;i<end;i++) {
        let k=candidates[i].key;if k==0u{continue;}
        var win=take_all!=0u||k>threshold;
        if !win&&k==threshold {win=tie_rank<needed;tie_rank++;}
        if win&&slot<params.capacity {proposed.slots[slot].owner=i+1u;slot++;}
    }
    if lid==255u {proposed.header=vec4u(min(slot,params.capacity),params.nonce,0u,0u);}
}
// Tile packing runs as one 256-thread workgroup. The priority sort, the
// resident carry-over and the slot lookups are all parallel (one thread per
// slot or per selected light); only the rare allocation of a *new* tile is
// serial, and that runs over an atlas occupancy bitmap held in workgroup
// memory (one bit per 128px cell) instead of a scanned 4096-word array.
var<workgroup> sel_owner:array<u32,256>;
var<workgroup> sel_key:array<u32,256>;
var<workgroup> sorted_owner:array<u32,256>;
var<workgroup> slot_found:array<u32,256>;
var<workgroup> need_faces:array<u32,256>;
var<workgroup> s_owner:array<u32,256>;
var<workgroup> s_fade:array<u32,256>;
var<workgroup> s_score:array<f32,256>;
var<workgroup> s_tiled:array<u32,256>;
var<workgroup> s_missing:array<u32,256>;
var<workgroup> occ:array<atomic<u32>,128>; // two words per cell row, bit = cell column
var<workgroup> sel_count:u32;
var<workgroup> victims:array<u32,256>; // retire-able residents, lowest score first
var<workgroup> victim_count:u32;
fn span_mask(x:u32,w:u32)->u32 {
    return select(0xffffffffu,(1u<<w)-1u,w<32u)<<(x&31u);
}
fn mark_tile(tile:vec4u) {
    if tile.z==0u { return; }
    let x0=tile.x/128u; let y0=tile.y/128u; let w=max(tile.z/128u,1u);
    if x0+w>64u||y0+w>64u||(x0&31u)+w>32u { return; }
    let m=span_mask(x0,w);
    for(var dy=0u;dy<w;dy++) { atomicOr(&occ[(y0+dy)*2u+(x0>>5u)],m); }
}
fn allocate_tile(wanted:u32)->vec4u {
    let cells=min(params.atlas_size/128u,64u);
    var size=min(wanted,params.atlas_size);
    for(var tier=0u;tier<5u;tier++) {
        let step=max(size/128u,1u);
        if step<=32u {
            let m=span_mask(0u,step);
            for(var y=0u;y+step<=cells;y+=step) {
                // Rows y..y+step OR'd once, then each aligned column is one bit test.
                var lo=0u;var hi=0u;
                for(var dy=0u;dy<step;dy++){lo|=atomicLoad(&occ[(y+dy)*2u]);hi|=atomicLoad(&occ[(y+dy)*2u+1u]);}
                for(var x=0u;x+step<=cells;x+=step) {
                    let word=select(lo,hi,x>=32u);
                    if (word&(m<<(x&31u)))==0u {
                        let tile=vec4u(x*128u,y*128u,size,0u);mark_tile(tile);return tile;
                    }
                }
            }
        }
        if size<=128u {break;} size/=2u;
    }
    return vec4u(0u);
}
@compute @workgroup_size(256)
fn pack_tiles(@builtin(local_invocation_index) t:u32) {
    let cap=params.capacity;
    var empty:Resident;
    if t==0u { sel_count=proposed.header.x; }
    if t<128u { atomicStore(&occ[t],0u); }
    workgroupBarrier();
    let count=sel_count;
    var my_owner=0u;var my_key=0u;
    if t<count { my_owner=proposed.slots[t].owner; my_key=candidates[my_owner-1u].key; }
    sel_owner[t]=my_owner;sel_key[t]=my_key;
    workgroupBarrier();
    // Stable priority order: higher key first, lower owner first on ties.
    if t<count {
        var rank=0u;
        for(var j=0u;j<count;j++) {
            let kj=sel_key[j];
            if kj>my_key||(kj==my_key&&sel_owner[j]<my_owner) {rank++;}
        }
        sorted_owner[rank]=my_owner;
    }
    // Carry each resident slot over: one thread per slot.
    var r=empty;
    if t<cap {
        r=committed.slots[t];var keep=false;
        if r.owner!=0u && r.fade_target==0u && r.strength<=0.0 {r=empty;}
        for(var i=0u;i<count;i++){if r.owner!=0u&&sel_owner[i]==r.owner {keep=true;}}
        r.fade_target=0u;
        if r.owner>0u&&r.owner<=params.rows&&keep {
            let c=candidates[r.owner-1u];r.score=bitcast<f32>(c.key);r.flags=lights[r.owner-1u]._pad;
            // Fade before reallocating tiers; the old tile remains valid until zero.
            if c.resolution==r.resolution || abs(f32(c.resolution)-f32(r.resolution))<=f32(r.resolution)*params.tuning.z {
                r.fade_target=u32(c.fade*65535.0);
            }
        }
        if r.fade_target==0u&&r.strength<=0.0 {r=empty;}
        // Invisible cube faces can be reclaimed without changing visible faces.
        if keep && r.owner>0u && r.owner<=params.rows {
            for(var f=0u;f<6u;f++) {if (candidates[r.owner-1u].mask&(1u<<f))==0u {r.tiles[f]=vec4u(0u);}}
        }
    }
    proposed.slots[t]=r;
    var missing=0u;var tiled=0u;
    if r.owner!=0u {
        for(var f=0u;f<6u;f++){
            mark_tile(r.tiles[f]);
            if r.tiles[f].z==0u {missing|=1u<<f;} else {tiled=1u;}
        }
    }
    s_owner[t]=r.owner;s_fade[t]=r.fade_target;s_score[t]=r.score;s_tiled[t]=tiled;s_missing[t]=missing;
    workgroupBarrier();storageBarrier();
    // Which slot (if any) already holds each selected light, and which faces it still needs.
    if t<count {
        let owner=sorted_owner[t];var slot=NONE;
        for(var s=0u;s<cap;s++){if s_owner[s]==owner {slot=s;break;}}
        slot_found[t]=slot;
        let mask=candidates[owner-1u].mask&63u;
        need_faces[t]=select(mask,mask&s_missing[slot],slot!=NONE);
    }
    if t<cap && s_owner[t]!=0u && s_fade[t]!=0u && s_tiled[t]!=0u {
        var rank=0u;
        for(var j=0u;j<cap;j++) {
            if s_owner[j]!=0u && s_fade[j]!=0u && s_tiled[j]!=0u && (s_score[j]<s_score[t]||(s_score[j]==s_score[t]&&j<t)) {rank++;}
        }
        victims[rank]=t;
    }
    workgroupBarrier();
    // Serial: only lights with a new slot or a missing tile get here.
    if t==0u {
        var next_free=0u;var vp=0u;var full=false;
        var nv=0u;
        for(var s=0u;s<cap;s++){if s_owner[s]!=0u && s_fade[s]!=0u && s_tiled[s]!=0u {nv++;}}
        for(var i=0u;i<count;i++) {
            var slot=slot_found[i];let found=slot!=NONE;
            if found && need_faces[i]==0u {continue;}
            let owner=sorted_owner[i];
            if !found {
                while next_free<cap && s_owner[next_free]!=0u {next_free++;}
                if next_free>=cap {continue;}
                slot=next_free;next_free++;
            }
            let c=candidates[owner-1u];
            if !found {
                let l=lights[owner-1u];
                proposed.slots[slot]=empty;proposed.slots[slot].owner=owner;
                proposed.slots[slot].kind=l.light_type;proposed.slots[slot].flags=l._pad;
                proposed.slots[slot].fade_target=u32(c.fade*65535.0);proposed.slots[slot].resolution=c.resolution;
                proposed.slots[slot].score=bitcast<f32>(c.key);
                s_owner[slot]=owner;s_fade[slot]=proposed.slots[slot].fade_target;s_score[slot]=proposed.slots[slot].score;s_tiled[slot]=0u;
            }
            let resolution=proposed.slots[slot].resolution;
            for(var f=0u;f<6u;f++) {
                if (need_faces[i]&(1u<<f))==0u {continue;}
                var tile=vec4u(0u);
                if !full { tile=allocate_tile(resolution); if tile.z==0u {full=true;} }
                if tile.z!=0u { proposed.slots[slot].tiles[f]=tile; s_tiled[slot]=1u; continue; }
                // Atlas is full (any free 128px cell would have fit): retire the
                // lowest-priority resident that ranks below this light, keeping its
                // map while it fades. Residents are walked lowest score first, so
                // this is a pointer step, not a scan.
                while vp<nv && s_fade[victims[vp]]==0u {vp++;}
                if vp<nv && s_score[victims[vp]]<bitcast<f32>(c.key) {
                    let victim=victims[vp];vp++;
                    proposed.slots[victim].fade_target=0u;s_fade[victim]=0u;
                }
            }
        }
        proposed.header.y=params.nonce;
    }
}
@compute @workgroup_size(64)
fn commit_lights(@builtin(global_invocation_id) gid:vec3u) {
    let i=gid.x;if i>=min(params.rows,arrayLength(&lights)){return;}
    let l=lights[i];if !(l.color_intensity.w>0.0){return;}
    let wants=requested(l);var flags=(l._pad&~4u)|8u|select(0u,4u,wants);
    if (flags&1u)==0u {flags|=1u|select(0u,2u,wants);}
    lights[i]._pad=flags;
    var index=NONE;
    if wants {for(var s=0u;s<params.capacity;s++){if committed.slots[s].owner==i+1u {index=s*6u;break;}}}
    lights[i].shadow_index=index;
}

