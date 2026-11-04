use crate::{
    pipelines::Pipelines,
    resources::{Fallbacks, Targets},
};

pub(crate) fn bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    resources: &[wgpu::BindingResource<'_>],
) -> wgpu::BindGroup {
    let entries: Vec<_> = resources
        .iter()
        .enumerate()
        .map(|(i, resource)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}
fn view(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::TextureView(v)
}

pub(crate) struct InternalBindings {
    pub depth_reduce: Vec<wgpu::BindGroup>,
    pub grid: wgpu::BindGroup,
    pub sample: [wgpu::BindGroup; 2],
    pub temporal: [wgpu::BindGroup; 2],
    pub spatial: [wgpu::BindGroup; 2],
    pub composite: [wgpu::BindGroup; 2],
}
impl InternalBindings {
    pub fn new(device: &wgpu::Device, p: &Pipelines, t: &Targets) -> Self {
        let grid = bind_group(
            device,
            "HLFS grid resources",
            &p.grid_bgl,
            &[
                t.coarse.as_entire_binding(),
                t.grid.as_entire_binding(),
                view(&t.depth_mips[0]),
                t.proposals.as_entire_binding(),
            ],
        );
        let depth_reduce = t
            .depth_mips
            .windows(2)
            .map(|mips| {
                bind_group(
                    device,
                    "HLFS depth reduction resources",
                    &p.depth_bgl,
                    &[view(&mips[0]), view(&mips[1])],
                )
            })
            .collect();
        let sample = std::array::from_fn(|write| {
            let previous = &t.history[1 - write];
            bind_group(
                device,
                "HLFS sample resources",
                &p.sample_bgl,
                &[
                    t.grid.as_entire_binding(),
                    previous.visible.as_entire_binding(),
                    t.history[write].visible.as_entire_binding(),
                    view(&t.raw_lighting.view),
                    view(&previous.geometry.view),
                    view(&t.depth_bounds.view),
                    t.proposals.as_entire_binding(),
                    view(&previous.reservoirs.view),
                    view(&t.history[write].reservoirs.view),
                ],
            )
        });
        let temporal = std::array::from_fn(|write| {
            let previous = &t.history[1 - write];
            let next = &t.history[write];
            bind_group(
                device,
                "HLFS temporal resources",
                &p.temporal_bgl,
                &[
                    view(&t.raw_lighting.view),
                    view(&previous.lighting.view),
                    view(&previous.geometry.view),
                    view(&next.lighting.view),
                    view(&next.geometry.view),
                    t.grid.as_entire_binding(),
                    next.visible.as_entire_binding(),
                ],
            )
        });
        let spatial = std::array::from_fn(|write| {
            bind_group(
                device,
                "HLFS spatial resources",
                &p.spatial_bgl,
                &[
                    view(&t.history[write].lighting.view),
                    view(&t.history[write].geometry.view),
                    view(&t.raw_lighting.view),
                ],
            )
        });
        let composite = std::array::from_fn(|write| {
            let next = &t.history[write];
            bind_group(
                device,
                "HLFS composite resources",
                &p.composite_bgl,
                &[
                    view(&t.raw_lighting.view),
                    view(&next.geometry.view),
                    view(&t.depth_bounds.view),
                    view(&t.history[1 - write].lighting.view),
                    view(&t.history[1 - write].geometry.view),
                    t.proposals.as_entire_binding(),
                    t.grid.as_entire_binding(),
                ],
            )
        });
        Self {
            depth_reduce,
            grid,
            sample,
            temporal,
            spatial,
            composite,
        }
    }
}

pub(crate) struct Inputs<'a> {
    pub camera: &'a wgpu::Buffer,
    pub lights: &'a wgpu::Buffer,
    pub compact_lights: &'a wgpu::Buffer,
    /// Live light rows: count, then row indices (Pulsar-Native#838).
    pub live_lights: &'a wgpu::Buffer,
    pub shadow_matrices: &'a wgpu::Buffer,
    pub shadow_atlas: &'a wgpu::TextureView,
    pub static_shadow_atlas: &'a wgpu::TextureView,
    /// Coloured glass transmittance, stored as 1 - T (zero is unfiltered).
    pub shadow_transmittance: &'a wgpu::TextureView,
    pub linear_sampler: &'a wgpu::Sampler,
    pub shadow_sampler: &'a wgpu::Sampler,
    /// albedo, normal, ORM, emissive, depth, lightmap UV, lightmap, pre-AA, velocity
    pub textures: [&'a wgpu::TextureView; 9],
    pub lightmap_sampler: &'a wgpu::Sampler,
    /// The frame's indirect diffuse at each pixel (`INDIRECT_DIFFUSE`), or a
    /// buffer holding none.
    pub gi_irradiance: &'a wgpu::Buffer,
}
struct CommonKey {
    buffers: [wgpu::Buffer; 4],
    shadow: [wgpu::TextureView; 3],
    sampler: wgpu::Sampler,
}
impl CommonKey {
    fn matches(&self, i: &Inputs<'_>) -> bool {
        &self.buffers[0] == i.camera
            && &self.buffers[1] == i.compact_lights
            && &self.buffers[2] == i.shadow_matrices
            && &self.buffers[3] == i.live_lights
            && &self.shadow[0] == i.shadow_atlas
            && &self.shadow[1] == i.static_shadow_atlas
            && &self.shadow[2] == i.shadow_transmittance
            && &self.sampler == i.shadow_sampler
    }
}
struct GBufferKey {
    textures: [wgpu::TextureView; 9],
    sampler: wgpu::Sampler,
    gi_irradiance: wgpu::Buffer,
}
impl GBufferKey {
    fn matches(&self, i: &Inputs<'_>) -> bool {
        self.textures
            .iter()
            .zip(i.textures)
            .all(|(old, new)| old == new)
            && &self.sampler == i.lightmap_sampler
            && &self.gi_irradiance == i.gi_irradiance
    }
}
#[derive(Default)]
pub(crate) struct ExternalBindings {
    pub compact: Option<wgpu::BindGroup>,
    compact_key: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer)>,
    pub ray: Option<wgpu::BindGroup>,
    pub transmission: bool,
    ray_key: Option<(wgpu::Tlas, wgpu::Buffer)>,
    pub common: Option<wgpu::BindGroup>,
    pub gbuffer: Option<wgpu::BindGroup>,
    common_key: Option<CommonKey>,
    gbuffer_key: Option<GBufferKey>,
}
impl ExternalBindings {
    pub fn clear_ray_binding(&mut self) {
        self.ray = None;
        self.ray_key = None;
    }
    pub fn update_ray(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        tlas: &wgpu::Tlas,
        transmission: &wgpu::Buffer,
    ) {
        if !self
            .ray_key
            .as_ref()
            .is_some_and(|(old_tlas, old_buffer)| old_tlas == tlas && old_buffer == transmission)
        {
            self.ray = Some(bind_group(
                device,
                "HLFS scene TLAS",
                layout,
                &[tlas.as_binding(), transmission.as_entire_binding()],
            ));
            self.ray_key = Some((tlas.clone(), transmission.clone()));
        }
    }
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        p: &Pipelines,
        f: &Fallbacks,
        globals: &wgpu::Buffer,
        shadows: &wgpu::Buffer,
        i: &Inputs<'_>,
    ) {
        if !self
            .compact_key
            .as_ref()
            .is_some_and(|(source, dest, live)| {
                source == i.lights && dest == i.compact_lights && live == i.live_lights
            })
        {
            self.compact = Some(bind_group(
                device,
                "HLFS compact light copy",
                &p.compact_bgl,
                &[
                    i.lights.as_entire_binding(),
                    i.compact_lights.as_entire_binding(),
                    i.live_lights.as_entire_binding(),
                    globals.as_entire_binding(),
                ],
            ));
            self.compact_key = Some((
                i.lights.clone(),
                i.compact_lights.clone(),
                i.live_lights.clone(),
            ));
        }
        // Compare actual wgpu handles. Wrapper addresses can remain unchanged when
        // a growable SceneDB buffer reallocates and must never be cache keys.
        if !self.common_key.as_ref().is_some_and(|k| k.matches(i)) {
            self.common = Some(bind_group(
                device,
                "HLFS scene inputs",
                &p.common_bgl,
                &[
                    globals.as_entire_binding(),
                    i.camera.as_entire_binding(),
                    i.compact_lights.as_entire_binding(),
                    shadows.as_entire_binding(),
                    view(i.shadow_atlas),
                    wgpu::BindingResource::Sampler(i.shadow_sampler),
                    i.shadow_matrices.as_entire_binding(),
                    view(&f.noise_view),
                    view(i.static_shadow_atlas),
                    view(i.shadow_transmittance),
                    wgpu::BindingResource::Sampler(i.linear_sampler),
                    i.live_lights.as_entire_binding(),
                ],
            ));
            self.common_key = Some(CommonKey {
                buffers: [
                    i.camera.clone(),
                    i.compact_lights.clone(),
                    i.shadow_matrices.clone(),
                    i.live_lights.clone(),
                ],
                shadow: [
                    i.shadow_atlas.clone(),
                    i.static_shadow_atlas.clone(),
                    i.shadow_transmittance.clone(),
                ],
                sampler: i.shadow_sampler.clone(),
            });
        }
        if !self.gbuffer_key.as_ref().is_some_and(|k| k.matches(i)) {
            self.gbuffer = Some(bind_group(
                device,
                "HLFS GBuffer inputs",
                &p.gbuffer_bgl,
                &[
                    view(i.textures[0]),
                    view(i.textures[1]),
                    view(i.textures[2]),
                    view(i.textures[3]),
                    view(i.textures[4]),
                    view(i.textures[5]),
                    view(i.textures[6]),
                    wgpu::BindingResource::Sampler(i.lightmap_sampler),
                    view(i.textures[7]),
                    view(i.textures[8]),
                    i.gi_irradiance.as_entire_binding(),
                ],
            ));
            self.gbuffer_key = Some(GBufferKey {
                textures: i.textures.map(Clone::clone),
                sampler: i.lightmap_sampler.clone(),
                gi_irradiance: i.gi_irradiance.clone(),
            });
        }
    }
}
