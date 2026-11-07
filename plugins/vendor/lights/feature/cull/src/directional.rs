//! The lights' directional lights ([`crate::DIRECTIONAL_LIGHTS`]): every
//! sun of the light list, in list order (the first is the primary sun),
//! picked on the GPU by one kernel (`shaders/directional_lights.wgsl`, the
//! lights' `light_is_sun` rule) from whatever list the frame has. The
//! lights' join records it after joining ([`DirectionalLightsPick`]); a host
//! that writes its light list itself gets it from
//! [`DirectionalLightsDerivation`], which the binning feature brings to
//! every renderer (`FeatureRegistrar::scene_derivation`). So the sky, GI,
//! water and media read one row, and no reader scans the list for suns.

use helio_core::{SceneDerivation, SceneDerivationContext, SceneDerivationOutput};
use helio_scene_join::{entry, BufferHandle, JoinOutput};

use crate::scene_rows::{DIRECTIONAL_LIGHTS_KEY, LIGHTS_KEY};

/// The kernel and its one-row output.
pub struct DirectionalLightsPick {
    pipeline: wgpu::ComputePipeline,
    output: JoinOutput,
    bind: Option<(wgpu::Buffer, wgpu::BindGroup)>,
}

impl DirectionalLightsPick {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = helio_core::shader::module_with(
            device,
            "Directional Lights",
            helio_core::include_wgsl!("../shaders/directional_lights.wgsl"),
            &[crate::LIGHT_LIST_SNIPPET],
        );
        let pipeline = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Directional Lights"),
                layout: None,
                module: &module,
                entry_point: Some("cs_directional_lights"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let mut output = JoinOutput::new(
            device,
            "Directional Lights",
            std::mem::size_of::<crate::DirectionalLights>() as u64,
        );
        output.ensure(device, 1);
        Self {
            pipeline,
            output,
            bind: None,
        }
    }

    /// Records the pick from `lights` (rows of `GpuLight`) into the output.
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        lights: &wgpu::Buffer,
    ) {
        if self.bind.as_ref().is_none_or(|(bound, _)| bound != lights) {
            let layout = self.pipeline.get_bind_group_layout(0);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Directional Lights"),
                layout: &layout,
                entries: &[entry(0, lights), entry(1, &self.output.buffer)],
            });
            self.bind = Some((lights.clone(), bind_group));
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Directional Lights"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind.as_ref().expect("bound above").1, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        drop(pass);
        self.output.content_generation += 1;
    }

    /// Clears the output (no light list: no directional lights).
    pub fn clear(&mut self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.output.buffer, 0, None);
        self.output.content_generation += 1;
    }

    /// The output, published as [`crate::DIRECTIONAL_LIGHTS`].
    pub fn handle(&self) -> BufferHandle {
        self.output.handle()
    }
}

/// The directional lights of a light list a host wrote itself: published
/// unless a derivation before it (the lights' join) already did.
pub struct DirectionalLightsDerivation {
    pick: DirectionalLightsPick,
    /// The list it last picked from, and that list's content generation.
    last: Option<(wgpu::Buffer, u64)>,
    cleared: bool,
}

impl DirectionalLightsDerivation {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            pick: DirectionalLightsPick::new(device),
            last: None,
            cleared: false,
        }
    }

    /// For `FeatureRegistrar::scene_derivation`.
    pub const FACTORY: helio_core::SceneDerivationFactory = helio_core::SceneDerivationFactory {
        name: "DirectionalLights",
        make: |device| Box::new(DirectionalLightsDerivation::new(device)),
    };
}

impl SceneDerivation for DirectionalLightsDerivation {
    fn name(&self) -> &'static str {
        "DirectionalLights"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        if ctx.inputs.get(DIRECTIONAL_LIGHTS_KEY).is_some() {
            return SceneDerivationOutput::default();
        }
        let mut recorded = false;
        match ctx.inputs.get(LIGHTS_KEY) {
            Some(lights) => {
                let now = (lights.buffer.clone(), lights.content_generation);
                if self.last.as_ref() != Some(&now) {
                    self.pick.record(ctx.device, encoder, &lights.buffer);
                    self.last = Some(now);
                    self.cleared = false;
                    recorded = true;
                }
            }
            None if !self.cleared => {
                self.pick.clear(encoder);
                self.last = None;
                self.cleared = true;
                recorded = true;
            }
            None => {}
        }
        SceneDerivationOutput {
            buffers: vec![(DIRECTIONAL_LIGHTS_KEY, self.pick.handle())],
            flushed: Vec::new(),
            recorded,
        }
    }
}
