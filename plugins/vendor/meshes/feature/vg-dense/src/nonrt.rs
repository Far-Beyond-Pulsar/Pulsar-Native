//! Tier 3-NonRT: tiled depth-peel compute raster (runs everywhere, no int64).

use crate::common::*;
use crate::config::{self, DenseConfig};

/// Why a [`DenseNonRtPass`] could not be created.
#[derive(Debug, PartialEq, Eq)]
pub enum DenseError {
    InvalidConfig(String),
    /// The device offers less workgroup storage than the peel kernel declares.
    InsufficientWorkgroupStorage {
        needed: u32,
        available: u32,
    },
}

impl std::fmt::Display for DenseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(m) => write!(f, "invalid dense config: {m}"),
            Self::InsufficientWorkgroupStorage { needed, available } => {
                write!(
                    f,
                    "peel kernel needs {needed} B workgroup storage, device has {available} B"
                )
            }
        }
    }
}
impl std::error::Error for DenseError {}

/// Buffers the NonRT kernels read (see `shaders/inputs.wgsl`).
pub struct DenseNonRtInputs<'a> {
    /// T7 `tile_classify`: one `u32` per tile, nonzero = dense.
    pub tile_mask: &'a wgpu::Buffer,
    /// The cull's `state` buffer; its per-phase/bin `{first, count}` ranges
    /// (word `CULL_STATE_RANGES + phase * 12 + bin * 2`) say which entries of
    /// `visible` are live. Both phases (the late one is the delta) are binned.
    pub cull_state: &'a wgpu::Buffer,
    /// The cull's `visible_clusters`: `GpuVisibleCluster { instance, cluster_slot }`
    /// rows indexed by the unified visible index.
    pub visible: &'a wgpu::Buffer,
    /// `helio-vg-stream` page pool.
    pub page_pool: &'a wgpu::Buffer,
    /// `vg_asset_quant` by asset slot.
    pub asset_quant: &'a wgpu::Buffer,
    /// `GpuVgInstance` rows.
    pub vg_instances: &'a wgpu::Buffer,
    /// The scene join's `static_objects` rows (252 B).
    pub objects: &'a wgpu::Buffer,
}

pub struct DenseNonRtPass {
    cfg: DenseConfig,
    compact: wgpu::ComputePipeline,
    args: wgpu::ComputePipeline,
    bin: wgpu::ComputePipeline,
    bin_scan: wgpu::ComputePipeline,
    bin_fill: wgpu::ComputePipeline,
    peel: wgpu::ComputePipeline,
    compact_bgl: wgpu::BindGroupLayout,
    args_bgl: wgpu::BindGroupLayout,
    bin_bgl: wgpu::BindGroupLayout,
    peel_bgl: wgpu::BindGroupLayout,
    buffers: Option<DenseBuffers>,
    binding: helio_mats::MaterialBindingConfig,
    /// Alpha-masked peel, built on first use (`None` inside: the device has too
    /// little workgroup storage for it).
    masked_peel: std::sync::OnceLock<Option<MaskedPeel>>,
    masked: Option<MaskedState>,
    /// Set around [`Self::record_masked_into_scoped`].
    masked_only: bool,
    dense_bin_only: bool,
    out_override: Option<wgpu::Buffer>,
}

struct MaskedPeel {
    pipeline: wgpu::ComputePipeline,
    bgl1: wgpu::BindGroupLayout,
}

/// The material tables the masked peel samples, owned (cheap `Arc` clones) so
/// callers do not thread a lifetime through [`DenseNonRtInputs`].
struct MaskedState {
    materials: wgpu::Buffer,
    material_textures: wgpu::Buffer,
    views: Vec<wgpu::TextureView>,
    samplers: Vec<wgpu::Sampler>,
}

impl DenseNonRtPass {
    pub fn new(device: &wgpu::Device, cfg: DenseConfig) -> Result<Self, DenseError> {
        cfg.validate().map_err(DenseError::InvalidConfig)?;
        let available = device.limits().max_compute_workgroup_storage_size;
        if cfg.shared_memory_bytes() > available {
            return Err(DenseError::InsufficientWorkgroupStorage {
                needed: cfg.shared_memory_bytes(),
                available,
            });
        }
        let compact_bgl = bgl(
            device,
            "vg_dense compact",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(true)),
                bgl_entry(2, storage(false)),
                bgl_entry(3, storage(false)),
                bgl_entry(4, storage(false)),
            ],
        );
        let args_bgl = bgl(
            device,
            "vg_dense args",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(false)),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(false)),
            ],
        );
        let bin_bgl = bgl(
            device,
            "vg_dense bin",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(true)),
                bgl_entry(5, storage(true)),
                bgl_entry(6, storage(true)),
                bgl_entry(7, storage(false)),
                bgl_entry(8, storage(false)),
                bgl_entry(10, storage(true)),
                bgl_entry(11, storage(true)),
            ],
        );
        let peel_bgl = bgl(
            device,
            "vg_dense peel",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(true)),
                bgl_entry(4, storage(true)),
                bgl_entry(5, storage(true)),
                bgl_entry(7, storage(false)),
                bgl_entry(8, storage(false)),
                bgl_entry(9, storage(false)),
                bgl_entry(10, storage(true)),
            ],
        );
        let tiles = config::tiles_source(&cfg);
        let args = config::args_source(&cfg);
        let bin = config::bin_source(&cfg);
        let peel = config::peel_source(&cfg);
        Ok(Self {
            compact: pipeline(device, "vg_dense compact", &tiles, "compact", &compact_bgl),
            args: pipeline(device, "vg_dense make_args", &args, "make_args", &args_bgl),
            bin: pipeline(device, "vg_dense bin", &bin, "bin", &bin_bgl),
            bin_scan: pipeline(device, "vg_dense bin_scan", &bin, "bin_scan", &bin_bgl),
            bin_fill: pipeline(device, "vg_dense bin_fill", &bin, "bin_fill", &bin_bgl),
            peel: pipeline(device, "vg_dense peel", &peel, "peel", &peel_bgl),
            compact_bgl,
            args_bgl,
            bin_bgl,
            peel_bgl,
            cfg,
            buffers: None,
            binding: helio_mats::MaterialBindingConfig::for_device(device),
            masked_peel: std::sync::OnceLock::new(),
            masked: None,
            masked_only: false,
            dense_bin_only: false,
            out_override: None,
        })
    }

    /// Provide (or, with `None`, withdraw) the scene's material tables for
    /// alpha-masked clusters (cull bins 6 and 7). While set, every dense tile is
    /// also peeled with the masked kernel, which evaluates the alpha test per
    /// candidate fragment. Without it masked clusters are ignored by Tier 3.
    pub fn set_masked(
        &mut self,
        materials: Option<(&wgpu::Buffer, helio_mats::MaterialTextureBindings<'_>)>,
    ) {
        self.masked = materials.map(|(m, t)| MaskedState {
            materials: m.clone(),
            material_textures: t.material_textures.clone(),
            views: t.texture_views.iter().map(|v| (*v).clone()).collect(),
            samplers: t.samplers.iter().map(|s| (*s).clone()).collect(),
        });
    }

    fn masked_pipeline(&self, device: &wgpu::Device) -> Option<&MaskedPeel> {
        self.masked_peel
            .get_or_init(|| {
                let needed = self.cfg.shared_memory_bytes_masked();
                let available = device.limits().max_compute_workgroup_storage_size;
                if needed > available {
                    log::warn!("vg dense: masked peel needs {needed} B workgroup storage, device has {available} B; alpha-masked clusters are skipped in dense tiles");
                    return None;
                }
                let bgl1 = bgl(
                    device,
                    "vg_dense masked alpha",
                    &helio_mats::vg_alpha::layout_entries(self.binding, 0, wgpu::ShaderStages::COMPUTE),
                );
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("vg_dense peel masked"),
                    source: wgpu::ShaderSource::Wgsl(config::peel_masked_source(&self.cfg, self.binding).into()),
                });
                let layouts = [Some(&self.peel_bgl), Some(&bgl1)];
                let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("vg_dense peel masked"),
                    bind_group_layouts: &layouts,
                    immediate_size: 0,
                });
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("vg_dense peel masked"),
                    layout: Some(&pl),
                    module: &module,
                    entry_point: Some("peel"),
                    compilation_options: Default::default(),
                    cache: None,
                });
                Some(MaskedPeel { pipeline, bgl1 })
            })
            .as_ref()
    }

    /// Bin (and, for the ray-traced backend, collect) only the clusters the cull
    /// diverted to the dense bin. Tier 3 then adds just those clusters to the
    /// Tier 1/2 result, which must be depth-MERGED with it (`VgDenseFrameData::merge`)
    /// instead of replaced inside dense tiles; every other cluster costs Tier 3 nothing.
    pub fn set_dense_bin_only(&mut self, on: bool) {
        self.dense_bin_only = on;
    }

    pub fn config(&self) -> &DenseConfig {
        &self.cfg
    }

    /// (Re)allocate the owned buffers for `viewport`. Done lazily by
    /// [`DenseNonRtPass::record`]; exposed so callers can pre-size.
    pub fn ensure_buffers(&mut self, device: &wgpu::Device, viewport: [u32; 2]) -> &DenseBuffers {
        if !self
            .buffers
            .as_ref()
            .is_some_and(|b| b.matches(&self.cfg, viewport))
        {
            self.buffers = Some(DenseBuffers::new(device, &self.cfg, viewport, true));
        }
        self.buffers.as_ref().unwrap()
    }

    pub fn buffers(&self) -> Option<&DenseBuffers> {
        self.buffers.as_ref()
    }

    /// Record the whole NonRT dense pass: compaction, indirect args, cluster
    /// binning and the per-tile peel. Output lands in `buffers().dense_key`.
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
    ) {
        let mut rec = helio_core::CommandRecorder::from_encoder(encoder);
        self.record_rec(device, queue, &mut rec, frame, inputs);
    }

    /// As [`Self::record`], into a render-graph command stream.
    pub fn record_rec(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
    ) {
        self.record_stages(
            device,
            queue,
            encoder,
            frame,
            inputs,
            true,
            true,
            &mut GpuScopes::none(),
        );
    }

    /// Only the cheap front half: counter reset, dense-tile compaction and the
    /// indirect args (so `tile_slot` and the bin ranges are valid). The
    /// ray-traced backend runs this to find dense instances while it skips the
    /// expensive peel.
    pub fn record_setup_rec(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
    ) {
        self.record_stages(
            device,
            queue,
            encoder,
            frame,
            inputs,
            true,
            false,
            &mut GpuScopes::none(),
        );
    }

    /// [`Self::record_setup_rec`] with per-kernel GPU timestamp scopes.
    pub fn record_setup_scoped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
        scopes: &mut GpuScopes<'_>,
    ) {
        self.record_stages(device, queue, encoder, frame, inputs, true, false, scopes);
    }

    /// [`Self::record_work_rec`] with per-kernel GPU timestamp scopes.
    pub fn record_work_scoped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
        scopes: &mut GpuScopes<'_>,
    ) {
        self.record_stages(device, queue, encoder, frame, inputs, false, true, scopes);
    }

    /// The back half for the alpha-masked class only, merged by depth into
    /// `out_key` (the ray-traced backend's key buffer): the opaque class is
    /// resolved by the rays. [`Self::record_setup_scoped`] must have run.
    #[allow(clippy::too_many_arguments)]
    pub fn record_masked_into_scoped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
        out_key: &wgpu::Buffer,
        scopes: &mut GpuScopes<'_>,
    ) {
        self.masked_only = true;
        self.out_override = Some(out_key.clone());
        self.record_stages(device, queue, encoder, frame, inputs, false, true, scopes);
        self.masked_only = false;
        self.out_override = None;
    }

    /// [`Self::record_rec`] with per-kernel GPU timestamp scopes.
    pub fn record_scoped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
        scopes: &mut GpuScopes<'_>,
    ) {
        self.record_stages(device, queue, encoder, frame, inputs, true, true, scopes);
    }

    /// The back half (`bin` + `peel`); [`Self::record_setup_rec`] must have
    /// been recorded earlier in the same frame.
    pub fn record_work_rec(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
    ) {
        self.record_stages(
            device,
            queue,
            encoder,
            frame,
            inputs,
            false,
            true,
            &mut GpuScopes::none(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_stages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
        setup: bool,
        work: bool,
        scopes: &mut GpuScopes<'_>,
    ) {
        self.ensure_buffers(device, frame.viewport);
        let masked_only = self.masked_only;
        let out_key = self.out_override.clone();
        let b = self.buffers.as_ref().unwrap();
        b.write_params_flags(
            queue,
            frame,
            u32::from(masked_only) | (u32::from(self.dense_bin_only) << 1),
        );
        let key_buf = out_key.as_ref().unwrap_or(&b.dense_key);
        // Counters only; the tile list tail is rewritten by `compact`.
        if setup {
            encoder.clear_buffer(&b.state, 0, Some(STATE_HEADER_WORDS as u64 * 4));
            // Pixels outside the processed dense tiles must read as empty: the
            // resolve merges this buffer per pixel without consulting the tile
            // mask, so a tile that stopped being dense must not keep old keys.
            encoder.clear_buffer(&b.dense_key, 0, None);
        }

        let compact_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense compact"),
            layout: &self.compact_bgl,
            entries: &[
                bg_entry(0, &b.params),
                bg_entry(1, inputs.tile_mask),
                bg_entry(2, &b.tile_slot),
                bg_entry(3, &b.state),
                bg_entry(4, &b.slot_lists),
            ],
        });
        let args_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense args"),
            layout: &self.args_bgl,
            entries: &[
                bg_entry(0, &b.params),
                bg_entry(1, &b.state),
                bg_entry(2, inputs.cull_state),
                bg_entry(3, &b.args),
            ],
        });
        let bin_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense bin"),
            layout: &self.bin_bgl,
            entries: &[
                bg_entry(0, &b.params),
                bg_entry(2, inputs.visible),
                bg_entry(3, inputs.page_pool),
                bg_entry(5, inputs.vg_instances),
                bg_entry(6, &b.tile_slot),
                bg_entry(7, &b.slot_lists),
                bg_entry(8, &b.state),
                bg_entry(10, inputs.objects),
                bg_entry(11, &b.args),
            ],
        });
        let peel_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense peel"),
            layout: &self.peel_bgl,
            entries: &[
                bg_entry(0, &b.params),
                bg_entry(2, inputs.visible),
                bg_entry(3, inputs.page_pool),
                bg_entry(4, inputs.asset_quant),
                bg_entry(5, inputs.vg_instances),
                bg_entry(7, &b.slot_lists),
                bg_entry(8, &b.state),
                bg_entry(9, key_buf),
                bg_entry(10, inputs.objects),
            ],
        });

        let mut run = |label: &'static str, f: &dyn Fn(&mut helio_core::ComputeCmds<'_>)| {
            scopes.begin(encoder, label);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: None,
                });
                f(&mut pass);
            }
            scopes.end(encoder, label);
        };
        if setup {
            run("vg_dense compact", &|p| {
                p.set_pipeline(&self.compact);
                p.set_bind_group(0, &compact_bg, &[]);
                p.dispatch_workgroups(b.tile_count().div_ceil(64), 1, 1);
            });
            run("vg_dense make_args", &|p| {
                p.set_pipeline(&self.args);
                p.set_bind_group(0, &args_bg, &[]);
                p.dispatch_workgroups(1, 1, 1);
            });
        }
        if work {
            run("vg_dense bin (count)", &|p| {
                p.set_pipeline(&self.bin);
                p.set_bind_group(0, &bin_bg, &[]);
                p.dispatch_workgroups_indirect(&b.args, 12);
            });
            run("vg_dense bin (scan)", &|p| {
                p.set_pipeline(&self.bin_scan);
                p.set_bind_group(0, &bin_bg, &[]);
                p.dispatch_workgroups(1, 1, 1);
            });
            run("vg_dense bin (fill)", &|p| {
                p.set_pipeline(&self.bin_fill);
                p.set_bind_group(0, &bin_bg, &[]);
                p.dispatch_workgroups_indirect(&b.args, 12);
            });
            if !masked_only {
                run("vg_dense peel", &|p| {
                    p.set_pipeline(&self.peel);
                    p.set_bind_group(0, &peel_bg, &[]);
                    p.dispatch_workgroups_indirect(&b.args, 0);
                });
            }
            // Alpha-masked clusters: same tiles, merged by depth into `dense_key`.
            if let (Some(ms), Some(mp)) = (&self.masked, self.masked_pipeline(device)) {
                let view_refs: Vec<&wgpu::TextureView> = ms.views.iter().collect();
                let sampler_refs: Vec<&wgpu::Sampler> = ms.samplers.iter().collect();
                let alpha_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("vg_dense masked alpha"),
                    layout: &mp.bgl1,
                    entries: &helio_mats::vg_alpha::bind_entries(
                        self.binding,
                        0,
                        &ms.materials,
                        helio_mats::MaterialTextureBindings {
                            material_textures: &ms.material_textures,
                            texture_views: &view_refs,
                            samplers: &sampler_refs,
                            version: 0,
                        },
                    ),
                });
                run("vg_dense peel masked", &|p| {
                    p.set_pipeline(&mp.pipeline);
                    p.set_bind_group(0, &peel_bg, &[]);
                    p.set_bind_group(1, &alpha_bg, &[]);
                    p.dispatch_workgroups_indirect(&b.args, 0);
                });
            }
        }
    }
}
