//! The tile mask Tier 3 resolves.
//!
//! Tier 1/2 do not draw clusters the cull routed to the dense bin (it routes by
//! last frame's dense mask, with a neighbourhood), and routing a tile's
//! clusters away starves the raster counters that keep it dense. The raster's
//! own mask therefore alternates ON/OFF frame to frame, and the diverted
//! clusters cover more tiles than the dense ones. The tiles Tier 3 must
//! resolve every frame are:
//!
//! ```text
//! resolve = this frame's dense mask | the mask the cull routed by
//!           | the tile footprint of every dense-bin cluster
//! ```
//!
//! (`mask_union.wgsl`, `footprint.wgsl`). Without the footprint, pixels of a
//! diverted cluster outside the dense tiles stay empty on every other frame.

use crate::common::{
    bg_entry, bgl, bgl_entry, pipeline, storage, uniform, DenseParamsGpu, GpuScopes,
};
use crate::config::{self, DenseConfig};
use crate::nonrt::DenseNonRtInputs;
use crate::DenseFrame;

const UNION: &str = include_str!("../shaders/mask_union.wgsl");

pub struct MaskUnion {
    union_pipeline: wgpu::ComputePipeline,
    union_bgl: wgpu::BindGroupLayout,
    union_params: wgpu::Buffer,
    fp_args_pipeline: wgpu::ComputePipeline,
    fp_pipeline: wgpu::ComputePipeline,
    fp_bgl: wgpu::BindGroupLayout,
    fp_args_bgl: wgpu::BindGroupLayout,
    fp_total: wgpu::Buffer,
    fp_params: wgpu::Buffer,
    fp_args: wgpu::Buffer,
    out: wgpu::Buffer,
    capacity: u32,
    tile: u32,
}

impl MaskUnion {
    pub fn new(device: &wgpu::Device, cfg: &DenseConfig) -> Self {
        let union_bgl = bgl(
            device,
            "vg_dense mask_union",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(true)),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(false)),
            ],
        );
        let fp_bgl = bgl(
            device,
            "vg_dense footprint",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(true)),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(true)),
                bgl_entry(5, storage(true)),
                bgl_entry(10, storage(true)),
                bgl_entry(12, storage(false)),
                bgl_entry(14, storage(false)),
            ],
        );
        let fp_args_bgl = bgl(
            device,
            "vg_dense footprint_args",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(true)),
                bgl_entry(13, storage(false)),
                bgl_entry(14, storage(false)),
            ],
        );
        let fp_src = config::footprint_source(cfg);
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            union_pipeline: pipeline(
                device,
                "vg_dense mask_union",
                UNION,
                "union_masks",
                &union_bgl,
            ),
            fp_args_pipeline: pipeline(
                device,
                "vg_dense footprint_args",
                &fp_src,
                "footprint_args",
                &fp_args_bgl,
            ),
            fp_pipeline: pipeline(device, "vg_dense footprint", &fp_src, "footprint", &fp_bgl),
            union_params: buffer(
                "vg_dense mask_union params",
                16,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            fp_params: buffer(
                "vg_dense footprint params",
                std::mem::size_of::<DenseParamsGpu>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            fp_args: buffer(
                "vg_dense footprint args",
                16,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
            ),
            fp_total: buffer("vg_dense footprint total", 16, wgpu::BufferUsages::STORAGE),
            out: Self::alloc(device, 64),
            union_bgl,
            fp_bgl,
            fp_args_bgl,
            capacity: 64,
            tile: cfg.tile,
        }
    }

    fn alloc(device: &wgpu::Device, tiles: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vg_dense tile mask (resolve)"),
            size: tiles as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    /// Record the resolve mask and return the buffer holding it (valid until
    /// the next call; reallocated on growth). `cur` is this frame's dense mask,
    /// `last` the mask the cull routed by.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rec: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        inputs: &DenseNonRtInputs<'_>,
        cur: &wgpu::Buffer,
        last: &wgpu::Buffer,
        scopes: &mut GpuScopes<'_>,
    ) -> &wgpu::Buffer {
        let tiles_x = frame.viewport[0].div_ceil(self.tile);
        let tiles_y = frame.viewport[1].div_ceil(self.tile);
        let tiles = tiles_x * tiles_y;
        if tiles > self.capacity {
            self.capacity = tiles.next_power_of_two();
            self.out = Self::alloc(device, self.capacity);
        }
        queue.write_buffer(
            &self.union_params,
            0,
            bytemuck::cast_slice(&[tiles, 0, 0, 0]),
        );
        let params = DenseParamsGpu {
            view_proj: frame.view_proj,
            inv_view_proj: frame.inv_view_proj,
            camera_pos: [
                frame.camera_pos[0],
                frame.camera_pos[1],
                frame.camera_pos[2],
                0.0,
            ],
            viewport: [frame.viewport[0], frame.viewport[1], tiles_x, tiles_y],
            limits: [tiles, 0, frame.visible_capacity, 0],
            misc: [frame.ray_t[0], frame.ray_t[1], 0.0, 0.0],
        };
        queue.write_buffer(&self.fp_params, 0, bytemuck::bytes_of(&params));

        let union_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense mask_union"),
            layout: &self.union_bgl,
            entries: &[
                bg_entry(0, &self.union_params),
                bg_entry(1, cur),
                bg_entry(2, last),
                bg_entry(3, &self.out),
            ],
        });
        let fp_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense footprint"),
            layout: &self.fp_bgl,
            entries: &[
                bg_entry(0, &self.fp_params),
                bg_entry(1, inputs.cull_state),
                bg_entry(2, inputs.visible),
                bg_entry(3, inputs.page_pool),
                bg_entry(5, inputs.vg_instances),
                bg_entry(10, inputs.objects),
                bg_entry(12, &self.out),
                bg_entry(14, &self.fp_total),
            ],
        });
        let fp_args_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense footprint_args"),
            layout: &self.fp_args_bgl,
            entries: &[
                bg_entry(0, &self.fp_params),
                bg_entry(1, inputs.cull_state),
                bg_entry(13, &self.fp_args),
                bg_entry(14, &self.fp_total),
            ],
        });
        let mut run = |label: &'static str, f: &dyn Fn(&mut helio_core::ComputeCmds<'_>)| {
            scopes.begin(rec, label);
            {
                let mut pass = rec.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: None,
                });
                f(&mut pass);
            }
            scopes.end(rec, label);
        };
        run("vg_dense mask_union", &|p| {
            p.set_pipeline(&self.union_pipeline);
            p.set_bind_group(0, &union_bg, &[]);
            p.dispatch_workgroups(tiles.div_ceil(64).max(1), 1, 1);
        });
        run("vg_dense footprint_args", &|p| {
            p.set_pipeline(&self.fp_args_pipeline);
            p.set_bind_group(0, &fp_args_bg, &[]);
            p.dispatch_workgroups(1, 1, 1);
        });
        run("vg_dense footprint", &|p| {
            p.set_pipeline(&self.fp_pipeline);
            p.set_bind_group(0, &fp_bg, &[]);
            p.dispatch_workgroups_indirect(&self.fp_args, 0);
        });
        &self.out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn union_shader_validates() {
        let module = naga::front::wgsl::parse_str(super::UNION).expect("parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate");
    }

    #[test]
    fn dense_bin_matches_the_cull() {
        let src = crate::config::footprint_source(&crate::DenseConfig::default());
        assert!(src.contains(&format!(
            "const FP_DENSE_BIN: u32 = {}u;",
            helio_pass_vg_cull::BIN_DENSE
        )));
    }
}
