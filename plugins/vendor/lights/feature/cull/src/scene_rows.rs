//! The lights' scene rows: one `GpuLight` row per placed light instance,
//! positioned and oriented by its owner, published as the frame's light list
//! ([`crate::LIGHT_LIST`]), with its directional lights
//! ([`crate::DIRECTIONAL_LIGHTS`]); and, in editor mode, a
//! billboard per light as `"billboard_instances"`. A light is placed when it
//! is attached, enabled, and its owner current and visible.

use helio_scene_join::{
    entry, origin_bits, origin_pair, rows_of, signature, BufferHandle, BufferKey, JoinOutput,
    SceneBufferProjection, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
    GENERATION_ROW_BYTES, HIDDEN_ROW_BYTES, OWNER_ROW_BYTES, TRANSFORM_ROW_BYTES,
};

/// A `GpuLight` in light space, bit 31 of its `_pad` word carrying `enabled`.
pub const LIGHT_SOURCE_ROW_BYTES: u64 = 128;
const LIGHT_ROW_BYTES: u64 = 128;
const BILLBOARD_ROW_BYTES: u64 = std::mem::size_of::<helio_pass_billboard::BillboardRow>() as u64;
const WORKGROUP: u32 = 64;

pub const LIGHTS_KEY: BufferKey = BufferKey::of(crate::LIGHT_LIST);
pub const DIRECTIONAL_LIGHTS_KEY: BufferKey = BufferKey::of(crate::DIRECTIONAL_LIGHTS);
pub const BILLBOARDS_KEY: BufferKey = helio_pass_billboard::BILLBOARD_INSTANCES.key();

/// The light join's shader as compiled (shader validation without a device).
pub fn light_join_source() -> String {
    helio_scene_join::join_source(include_str!("../shaders/scene_join_lights.wgsl"))
}

/// Where the rows the light join reads live.
#[derive(Clone, Copy, Debug)]
pub struct LightJoinKeys {
    pub owners: BufferKey,
    pub generations: BufferKey,
    pub hidden: BufferKey,
    /// World transforms, keyed by the owner entity.
    pub transforms: BufferKey,
    /// Light instances' light-space rows, keyed by the instance entity.
    pub light_sources: BufferKey,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    rows: u32,
    /// Also write a billboard per light.
    billboards: u32,
    _pad: [u32; 2],
    /// The frame's world origin as an f32 pair, subtracted pair-wise from
    /// the world positions of the rows.
    origin_hi: [f32; 4],
    origin_lo: [f32; 4],
}

/// The lights' scene derivation. See the module doc.
pub struct LightJoin {
    keys: LightJoinKeys,
    billboards: bool,
    pipeline: wgpu::ComputePipeline,
    uniforms: wgpu::Buffer,
    /// Bound in place of the hidden flags while none are registered.
    empty: wgpu::Buffer,
    lights: JoinOutput,
    directional: crate::DirectionalLightsPick,
    billboard_rows: JoinOutput,
    bind: Option<(Vec<wgpu::Buffer>, wgpu::BindGroup)>,
    last_inputs: Option<u64>,
    reported: Vec<BufferKey>,
}

impl LightJoin {
    /// `billboards`: also publish a billboard per placed light (editor
    /// light icons).
    pub fn new(device: &wgpu::Device, keys: LightJoinKeys, billboards: bool) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Light Join"),
            source: wgpu::ShaderSource::Wgsl(light_join_source().into()),
        });
        let pipeline = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Light Join"),
                layout: None,
                module: &module,
                entry_point: Some("cs_join_lights"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        Self {
            keys,
            billboards,
            pipeline,
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Light Join Uniforms"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            empty: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Light Join Empty Input"),
                size: 256,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            lights: JoinOutput::new(device, "Light Join Lights", LIGHT_ROW_BYTES),
            directional: crate::DirectionalLightsPick::new(device),
            billboard_rows: JoinOutput::new(device, "Light Join Billboards", BILLBOARD_ROW_BYTES),
            bind: None,
            last_inputs: None,
            reported: Vec::new(),
        }
    }

    fn input<'a>(
        &mut self,
        inputs: &'a SceneBufferProjection,
        key: BufferKey,
        row_bytes: u64,
    ) -> Option<&'a BufferHandle> {
        let handle = inputs.get(key)?;
        if handle.row_bytes != 0 && handle.row_bytes != row_bytes {
            if !self.reported.contains(&key) {
                self.reported.push(key);
                log::error!(
                    "light join: buffer {key:?} has {}-byte rows, expected {row_bytes}; \
                     no light is placed",
                    handle.row_bytes
                );
            }
            return None;
        }
        Some(handle)
    }
}

impl SceneDerivation for LightJoin {
    fn name(&self) -> &'static str {
        "LightJoin"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        let keys = self.keys;
        let inputs = ctx.inputs;
        let owners = self.input(inputs, keys.owners, OWNER_ROW_BYTES);
        let generations = self.input(inputs, keys.generations, GENERATION_ROW_BYTES);
        let hidden = self.input(inputs, keys.hidden, HIDDEN_ROW_BYTES);
        let transforms = self.input(inputs, keys.transforms, TRANSFORM_ROW_BYTES);
        let sources = self.input(inputs, keys.light_sources, LIGHT_SOURCE_ROW_BYTES);
        let (origin_hi, origin_lo) = origin_pair(ctx.world_origin);
        let [ox, oy, oz] = origin_bits(ctx.world_origin);
        let mut recorded = false;

        let light_rows = sources.map_or(0, |h| rows_of(h, LIGHT_SOURCE_ROW_BYTES));
        let grew = self.lights.ensure(ctx.device, light_rows)
            | (self.billboards && self.billboard_rows.ensure(ctx.device, light_rows));
        let light_signature = signature(
            &[owners, generations, hidden, transforms, sources],
            &[
                self.lights.epoch,
                self.billboard_rows.epoch,
                u64::from(self.billboards),
                ox,
                oy,
                oz,
            ],
        );
        if grew || self.last_inputs != Some(light_signature) {
            helio_core::cpu_scope!("LightJoin");
            self.last_inputs = Some(light_signature);
            encoder.clear_buffer(&self.lights.buffer, 0, None);
            if self.billboards {
                encoder.clear_buffer(&self.billboard_rows.buffer, 0, None);
            }
            if let (Some(owners), Some(generations), Some(transforms), Some(sources)) =
                (owners, generations, transforms, sources)
            {
                ctx.queue.write_buffer(
                    &self.uniforms,
                    0,
                    bytemuck::bytes_of(&Uniforms {
                        rows: light_rows.min(self.lights.rows),
                        billboards: u32::from(self.billboards),
                        _pad: [0; 2],
                        origin_hi,
                        origin_lo,
                    }),
                );
                let bound = [
                    &owners.buffer,
                    &generations.buffer,
                    hidden.map_or(&self.empty, |h| &h.buffer),
                    &transforms.buffer,
                    &sources.buffer,
                    &self.lights.buffer,
                    &self.billboard_rows.buffer,
                ];
                let reusable = self.bind.as_ref().is_some_and(|(buffers, _)| {
                    buffers.iter().zip(bound).all(|(cached, now)| cached == now)
                });
                if !reusable {
                    let layout = self.pipeline.get_bind_group_layout(0);
                    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Light Join"),
                        layout: &layout,
                        entries: &[
                            entry(0, &self.uniforms),
                            entry(1, bound[0]),
                            entry(2, bound[1]),
                            entry(3, bound[2]),
                            entry(4, bound[3]),
                            entry(5, bound[4]),
                            entry(6, bound[5]),
                            entry(7, bound[6]),
                        ],
                    });
                    self.bind = Some((bound.iter().map(|b| (*b).clone()).collect(), bind_group));
                }
                let bind_group = &self.bind.as_ref().expect("bound above").1;
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Light Join"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.dispatch_workgroups(light_rows.div_ceil(WORKGROUP).max(1), 1, 1);
                drop(pass);
                self.directional
                    .record(ctx.device, encoder, &self.lights.buffer);
            } else {
                self.directional.clear(encoder);
            }
            self.lights.content_generation += 1;
            self.billboard_rows.content_generation += 1;
            recorded = true;
        }

        let mut buffers = vec![
            (LIGHTS_KEY, self.lights.handle()),
            (DIRECTIONAL_LIGHTS_KEY, self.directional.handle()),
        ];
        if self.billboards {
            buffers.push((BILLBOARDS_KEY, self.billboard_rows.handle()));
        }
        SceneDerivationOutput {
            buffers,
            flushed: Vec::new(),
            recorded,
        }
    }
}
