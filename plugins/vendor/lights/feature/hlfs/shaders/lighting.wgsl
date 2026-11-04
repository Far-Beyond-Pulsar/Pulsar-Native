struct Incident { direction: vec3<f32>, radiance: vec3<f32>, }
struct Lighting { diffuse: vec3<f32>, specular: vec3<f32>, }
struct Surface {
    position: vec3<f32>, normal: vec3<f32>, view: vec3<f32>,
    albedo: vec3<f32>, f0: vec3<f32>, roughness: f32, metallic: f32,
    specular_factor: vec3<f32>,
}
fn specular_modulation(s: Surface) -> vec3<f32> {
    // The engine's analytic split-sum DFG fit factors view angle and roughness
    // out of the filtered specular signal along with the material reflectance.
    let c0=vec4<f32>(-1.0,-0.0275,-0.572,0.022);
    let c1=vec4<f32>(1.0,0.0425,1.04,-0.04);
    let r=s.roughness*c0+c1;
    let a004=min(r.x*r.x,exp2(-9.28*max(dot(s.normal,s.view),0.0)))*r.x+r.y;
    let ab=vec2<f32>(-1.04,1.04)*a004+r.zw;
    return max(s.f0*ab.x+ab.y,vec3<f32>(0.01));
}
fn surface_at(pixel: vec2<u32>) -> Surface {
    let p=vec2<i32>(pixel);
    let position=world_position(vec2<f32>(pixel)+0.5,textureLoad(gbuf_depth,p,0));
    let albedo=max(textureLoad(gbuf_albedo,p,0).rgb,vec3<f32>(0.0));
    let orm=textureLoad(gbuf_orm,p,0).rgb;
    let metallic=clamp(orm.b,0.0,1.0);
    var s=Surface(position,safe_normalize(textureLoad(gbuf_normal,p,0).xyz),
        safe_normalize(cameras[0].position_near.xyz-position),albedo,
        mix(vec3<f32>(0.04),albedo,metallic),clamp(orm.g,0.02,1.0),metallic,vec3<f32>(1.0));
    s.specular_factor=specular_modulation(s);
    return s;
}
fn incident(light: GpuLight, position: vec3<f32>) -> Incident {
    if light.light_type==0u {
        return Incident(safe_normalize(-light.direction_outer.xyz),max(light.color_intensity.rgb*light.color_intensity.w,vec3<f32>(0.0)));
    }
    let delta=light.position_range.xyz-position;
    let d2=dot(delta,delta); let range=light.position_range.w;
    if range<=0.0 || d2>=range*range { return Incident(vec3<f32>(0.0),vec3<f32>(0.0)); }
    let direction=safe_normalize(delta);
    let normalized_d2=d2/(range*range);
    var attenuation=max(0.0,1.0-normalized_d2*normalized_d2)/max(d2,0.0001);
    if light.light_type==2u {
        let cosine=dot(-direction,light.direction_outer.xyz);
        // Equal inner/outer angles are a hard cone, not smoothstep(0,0,x).
        let cone=clamp((cosine-light.direction_outer.w)/max(light.inner_angle-light.direction_outer.w,1e-5),0.0,1.0);
        attenuation*=cone*cone*(3.0-2.0*cone);
    }
    return Incident(direction,max(light.color_intensity.rgb*light.color_intensity.w*attenuation,vec3<f32>(0.0)));
}
// Necessary support test for direct illumination, without evaluating the BRDF.
// Exact light loops only need to reject guaranteed zero contribution here.
fn can_illuminate(id: u32, s: Surface) -> bool {
    let inc=incident(lights[id],s.position);
    return dot(s.normal,inc.direction)>0.0 && any(inc.radiance>vec3<f32>(0.0)) && globals.exposure>0.0;
}

fn importance(id: u32, s: Surface) -> f32 {
    if USE_TILE_PRESAMPLING {
        // Current-frame RIS uses the actual unshadowed luminance target.
        // Absolute color retains support for the existing HDR-albedo range.
        let light=evaluate_light(id,s,Visibility(1.0));
        let target_weight=luminance(abs(light.diffuse*s.albedo+light.specular*s.specular_factor));
        // Positive support is needed when a reused sample moves from an unlit
        // surface to one this light can illuminate. Uniform discovery covers IDs.
        return max(target_weight,select(0.0,1e-8,(globals.surface_flags&8u)!=0u));
    }
    let inc=incident(lights[id],s.position);
    let ndl=max(dot(s.normal,inc.direction),0.0);
    // Cheap, positive proxy. The PDF correction below does not require an exact BRDF.
    // Log weighting prevents one very bright/occluded light monopolizing candidates.
    let proxy=log2(1.0+max(luminance(inc.radiance)*ndl*globals.exposure,0.0));
    if proxy<0.0001 && globals.debug_mode!=1u {
        // Only a tiny proxy pays for the exact unshadowed bound. Dividing the
        // exposure-space error budget by population bounds aggregate loss even
        // when a scene contains thousands of individually dim lights.
        let bound=evaluate_light(id,s,Visibility(1.0));
        let color=bound.diffuse*s.albedo+bound.specular*s.specular_factor;
        let budget=0.00001*min(globals.exposure,1.0);
        if max(color.r,max(color.g,color.b))*f32(live_light_count())<budget { return 0.0; }
    }
    return proxy;
}
fn evaluate_incident(s: Surface, inc: Incident, visibility: Visibility) -> Lighting {
    let ndl=max(dot(s.normal,inc.direction),0.0);
    let ndv=max(dot(s.normal,s.view),0.0);
    let h=safe_normalize(s.view+inc.direction);
    let ndh=max(dot(s.normal,h),0.0);
    let a=s.roughness*s.roughness; let a2=a*a;
    let denominator=ndh*ndh*(a2-1.0)+1.0;
    let d=a2/(PI*denominator*denominator+0.0001);
    let k=(s.roughness+1.0)*(s.roughness+1.0)/8.0;
    let g=(ndv/(ndv*(1.0-k)+k+0.0001))*(ndl/(ndl*(1.0-k)+k+0.0001));
    let f=s.f0+(1.0-s.f0)*pow5(1.0-clamp(dot(h,s.view),0.0,1.0));
    let energy=inc.radiance*(ndl*visibility*globals.exposure);
    // Demodulate before filtering; material response returns at full resolution.
    return Lighting((1.0-f)*(1.0-s.metallic)*energy/PI,
        d*g*f*energy/((4.0*ndv*ndl+0.0001)*s.specular_factor));
}
fn evaluate_light(id: u32, s: Surface, visibility: Visibility) -> Lighting {
    return evaluate_incident(s,incident(lights[id],s.position),visibility);
}
