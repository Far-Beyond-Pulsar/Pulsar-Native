//! Portal rows for the portal passes, without reserved entities.
//!
//! Helio's portal passes read three scene buffers (`portal_views`, the chain
//! handle table and the chain payload pool) that were designed as SceneDB rows
//! indexed by entity: the host had to reserve a contiguous range of entities at
//! the bottom of the world. An editor world cannot do that, and nothing about
//! the rows is really entity data: they are derived each frame from the
//! authored portals.
//!
//! This derivation takes them from the host instead. The host builds a
//! [`PortalProjectionFrame`] (from the authored portals, through
//! `PortalProjectionBridge`) and hands it to the shared [`PortalProjectionInput`];
//! the derivation uploads the rows into buffers it owns and publishes them under
//! the keys the passes already resolve, exactly as the scene join publishes
//! `static_objects`. The host also publishes how many views and chains are
//! live ([`crate::PORTAL_COUNTS`]) and puts the projection's coordinate
//! spaces in the one table (`PortalProjectionBridge::build_into`).
//!
//! With no portals nothing is published, and the passes skip themselves.

use std::sync::{Arc, Mutex};

use crate::{
    components::PortalViewRow, PortalProjectionFrame, PORTAL_CHAIN_HANDLE_BUFFER,
    PORTAL_CHAIN_PORTAL_POOL_BUFFER,
};

pub use helio_core::{
    BufferHandle, BufferKey, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
};

/// One portal view row (`GpuPortalView`).
pub const PORTAL_VIEW_ROW_BYTES: u64 = std::mem::size_of::<PortalViewRow>() as u64;
/// One chain handle: `offset` and `count` into the payload pool, `u32` each.
pub const PORTAL_CHAIN_HANDLE_ROW_BYTES: u64 = 8;
/// One chain's content-from-host matrix (`mat4x4<f32>`).
pub const PORTAL_CHAIN_UNMAP_ROW_BYTES: u64 = 64;
/// Per chain: the matrix taking a point in the host world back into the world
/// of the content that chain shows, published as the frame's view remaps
/// (`helio_frame_api::lighting::VIEW_REMAPS`). Lighting uses it to shade a
/// portal pixel in the content's own world (its sun, sky, reflections and
/// lights), not the host's. Fixed size, since lighting binds it as a uniform
/// of exactly this size.
pub const PORTAL_CHAIN_UNMAP_CAPACITY: u64 = helio_frame_api::lighting::VIEW_REMAPS_BYTES;
pub const PORTAL_CHAIN_UNMAP_KEY: BufferKey = BufferKey::of(helio_frame_api::lighting::VIEW_REMAPS);

#[derive(Default)]
struct Rows {
    /// Moves whenever the rows change.
    generation: u64,
    views: Vec<PortalViewRow>,
    /// `[offset, count]` per chain.
    handles: Vec<[u32; 2]>,
    /// Every chain's portal view indices, back to back.
    pool: Vec<u32>,
    /// Per chain, host world -> content world (column-major).
    unmaps: Vec<[f32; 16]>,
}

/// Where the host hands the derivation its rows. Cloning shares them.
#[derive(Clone, Default)]
pub struct PortalProjectionInput(Arc<Mutex<Rows>>);

impl PortalProjectionInput {
    /// Make `frame` the portals to draw. Unchanged rows do not count as a
    /// change, so a still scene uploads nothing.
    ///
    /// `spaces` is the host's coordinate-space table (slot -> matrix) that the
    /// views index; it gives each chain the matrix back into its content's world.
    pub fn set(&self, frame: &PortalProjectionFrame, spaces: &[glam::Mat4]) {
        let mut handles = Vec::with_capacity(frame.portal_chains.len());
        let mut pool = Vec::new();
        for chain in &frame.portal_chains {
            handles.push([pool.len() as u32, chain.portals.len() as u32]);
            pool.extend_from_slice(&chain.portals);
        }
        // Host world -> content world: undo each stage's placement, outermost first.
        let unmaps: Vec<[f32; 16]> = frame
            .portal_chains
            .iter()
            .map(|chain| {
                let mut unmap = glam::Mat4::IDENTITY;
                for &portal in &chain.portals {
                    let slot = frame.portal_views[portal as usize].coordinate_space as usize;
                    let space = spaces.get(slot).copied().unwrap_or(glam::Mat4::IDENTITY);
                    unmap = space.inverse() * unmap;
                }
                unmap.to_cols_array()
            })
            .collect();
        let mut rows = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if rows.views == frame.portal_views
            && rows.handles == handles
            && rows.pool == pool
            && rows.unmaps == unmaps
        {
            return;
        }
        rows.views = frame.portal_views.clone();
        rows.handles = handles;
        rows.pool = pool;
        rows.unmaps = unmaps;
        rows.generation += 1;
    }

    /// No portals.
    pub fn clear(&self) {
        let mut rows = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if rows.views.is_empty() && rows.handles.is_empty() {
            return;
        }
        rows.views.clear();
        rows.handles.clear();
        rows.pool.clear();
        rows.unmaps.clear();
        rows.generation += 1;
    }
}

struct Upload {
    buffer: wgpu::Buffer,
    capacity: u64,
    epoch: u64,
}

impl Upload {
    fn new(device: &wgpu::Device, label: &'static str, capacity: u64) -> Self {
        Self {
            buffer: Self::allocate(device, label, capacity),
            capacity,
            epoch: 0,
        }
    }

    fn allocate(device: &wgpu::Device, label: &'static str, bytes: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.max(16),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn write(&mut self, ctx: &SceneDerivationContext<'_>, label: &'static str, bytes: &[u8]) {
        let needed = (bytes.len() as u64).max(16);
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = Self::allocate(ctx.device, label, self.capacity);
            self.epoch += 1;
        }
        if !bytes.is_empty() {
            ctx.queue.write_buffer(&self.buffer, 0, bytes);
        }
    }

    fn handle(&self, row_bytes: u64, content_generation: u64) -> BufferHandle {
        BufferHandle {
            buffer: self.buffer.clone(),
            epoch: self.epoch,
            row_bytes,
            content_generation,
        }
    }
}

/// See the module doc.
pub struct PortalProjection {
    input: PortalProjectionInput,
    views: Upload,
    handles: Upload,
    pool: Upload,
    unmaps: Upload,
    uploaded: u64,
    live: bool,
}

impl PortalProjection {
    pub fn new(device: &wgpu::Device, input: PortalProjectionInput) -> Self {
        Self {
            input,
            views: Upload::new(device, "Portal Views", 16),
            handles: Upload::new(device, "Portal Chain Handles", 16),
            pool: Upload::new(device, "Portal Chain Payload", 16),
            unmaps: Upload::new(device, "Portal Chain Unmap", PORTAL_CHAIN_UNMAP_CAPACITY),
            uploaded: 0,
            live: false,
        }
    }
}

impl SceneDerivation for PortalProjection {
    fn name(&self) -> &'static str {
        "PortalProjection"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        _encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        let (generation, views_empty) = {
            let rows = self.input.0.lock().unwrap_or_else(|p| p.into_inner());
            if rows.generation != self.uploaded {
                self.views
                    .write(ctx, "Portal Views", bytemuck::cast_slice(&rows.views));
                self.handles.write(
                    ctx,
                    "Portal Chain Handles",
                    bytemuck::cast_slice(&rows.handles),
                );
                self.pool.write(
                    ctx,
                    "Portal Chain Payload",
                    bytemuck::cast_slice(&rows.pool),
                );
                self.unmaps.write(
                    ctx,
                    "Portal Chain Unmap",
                    bytemuck::cast_slice(&rows.unmaps),
                );
                self.uploaded = rows.generation;
            }
            (rows.generation, rows.views.is_empty())
        };
        self.live = !views_empty;
        let mut output = SceneDerivationOutput::default();
        // No portals: publish nothing, and the portal passes skip themselves.
        if self.live {
            output.buffers = vec![
                (
                    BufferKey::of("portal_views"),
                    self.views.handle(PORTAL_VIEW_ROW_BYTES, generation),
                ),
                (
                    BufferKey::of(PORTAL_CHAIN_HANDLE_BUFFER),
                    self.handles
                        .handle(PORTAL_CHAIN_HANDLE_ROW_BYTES, generation),
                ),
                (
                    BufferKey::of(PORTAL_CHAIN_PORTAL_POOL_BUFFER),
                    self.pool.handle(4, generation),
                ),
                (
                    PORTAL_CHAIN_UNMAP_KEY,
                    self.unmaps.handle(PORTAL_CHAIN_UNMAP_ROW_BYTES, generation),
                ),
            ];
        }
        output
    }
}
