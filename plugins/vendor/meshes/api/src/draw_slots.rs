//! The draw slot a GPU-driven vertex shader reads, the same on every backend.
//!
//! A GPU-driven draw puts the slot of its first instance in the indirect
//! args' `first_instance`, and its vertex shader used `@builtin(instance_index)`
//! to find it. On DX12 that builtin is `SV_InstanceID` (which starts at 0 for
//! every draw) plus a root constant wgpu writes per draw, and wgpu writes it
//! only for single indirect draws with indirect-call validation on. A
//! `multi_draw_indexed_indirect_count` draw, or any indirect draw with
//! validation off (Helio's release default), gets 0: every draw reads slot 0
//! onward, so copies collapse onto one object and shadows vanish.
//!
//! Per-instance vertex data, by contrast, is offset by `first_instance` on
//! every backend (`StartInstanceLocation` on DX12). So these shaders take
//! their slot as a per-instance vertex attribute at [`DRAW_SLOT_LOCATION`],
//! read from an identity buffer (`[0, 1, 2, …]`) that [`DrawSlots`] keeps
//! large enough for the array the slot indexes.

/// Bytes per `GpuInstanceData` (helio-pass-object-batch), for passes whose
/// slot indexes the instance buffer itself.
pub const INSTANCE_BYTES: u64 = 224;

/// The vertex location every GPU-driven vertex shader reads its slot from.
pub const DRAW_SLOT_LOCATION: u32 = 15;

const DRAW_SLOT_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Uint32,
    offset: 0,
    shader_location: DRAW_SLOT_LOCATION,
}];

/// The per-instance vertex buffer layout of [`DrawSlots::buffer`].
pub const DRAW_SLOT_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 4,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &DRAW_SLOT_ATTRIBUTES,
};

/// An identity `u32` buffer, `[0, 1, 2, …]`, bound as [`DRAW_SLOT_LAYOUT`].
#[derive(Default)]
pub struct DrawSlots {
    buffer: Option<wgpu::Buffer>,
}

impl DrawSlots {
    /// The buffer, holding at least `len` slots (and at least one, so it can
    /// always be bound). It grows by doubling and is never shrunk.
    pub fn buffer(&mut self, device: &wgpu::Device, len: u32) -> &wgpu::Buffer {
        let len = len.max(1);
        let current = self.buffer.as_ref().map_or(0, |b| (b.size() / 4) as u32);
        if current < len {
            let len = len.next_power_of_two().max(64);
            let slots: Vec<u32> = (0..len).collect();
            self.buffer = Some(wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("Draw slots"),
                    contents: bytemuck::cast_slice(&slots),
                    usage: wgpu::BufferUsages::VERTEX,
                },
            ));
        }
        self.buffer.as_ref().expect("allocated above")
    }

    /// The slots an array of `bytes` bytes with `stride`-byte elements holds.
    pub fn len_of(bytes: u64, stride: u64) -> u32 {
        (bytes / stride.max(1)).min(u32::MAX as u64) as u32
    }
}
