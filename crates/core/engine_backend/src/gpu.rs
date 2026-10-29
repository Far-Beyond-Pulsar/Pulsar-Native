//! Persistent GPU objects for Pulsar's own render work.
//!
//! Every frame a viewport renders may upload, read back and bind; none of
//! that may create GPU objects in steady state. Each created wgpu object is
//! a driver object (a Vulkan buffer, a descriptor set, a D3D12 heap range)
//! that the driver tracks until the GPU is done with it, so objects made and
//! dropped every frame grow driver memory and cost CPU in submit and poll.
//!
//! - [`BindGroupCache`]: a bind group kept while the resources it binds are
//!   the same ones; rebuilt only when one of them is replaced.
//! - [`ReadbackRing`]: GPU-to-CPU copies through a ring of reused
//!   `MAP_READ` buffers, mapped asynchronously; the frame never waits.
//! - [`Uploads`]: a staging pool for CPU-to-GPU writes, recorded into an
//!   encoder from reused `MAP_WRITE` chunks ([`wgpu::util::StagingBelt`])
//!   instead of `Queue::write_buffer`, which makes a new staging buffer for
//!   every call. One pool serves every writer into the same encoder (a
//!   renderer's scene derivations share theirs).
//!
//! The cache and the ring count what they create (`created`), so tests can
//! show that a steady frame creates nothing.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// A bind group kept while the resources it binds are the same ones.
///
/// `K` names those resources (wgpu objects compare by identity): the cache
/// holds the key, so a replaced resource is not freed, and its id not
/// reused, while the cache still compares against it. Whatever else the
/// bind group needs (a texture view of a keyed texture) is kept with it.
pub struct BindGroupCache<K, Extra = ()> {
    built: Option<(K, wgpu::BindGroup, Extra)>,
    created: u64,
}

impl<K, Extra> Default for BindGroupCache<K, Extra> {
    fn default() -> Self {
        Self {
            built: None,
            created: 0,
        }
    }
}

impl<K: PartialEq, Extra> BindGroupCache<K, Extra> {
    /// The bind group for `key`, built by `build` only when `key` differs
    /// from the last one.
    pub fn get_or_build(
        &mut self,
        key: K,
        build: impl FnOnce(&K) -> (wgpu::BindGroup, Extra),
    ) -> &wgpu::BindGroup {
        if self
            .built
            .as_ref()
            .is_none_or(|(built, _, _)| *built != key)
        {
            let (group, extra) = build(&key);
            self.built = Some((key, group, extra));
            self.created += 1;
        }
        &self.built.as_ref().expect("bind group built above").1
    }

    /// Bind groups built so far.
    pub fn created(&self) -> u64 {
        self.created
    }
}

impl BindGroupCache<Vec<wgpu::Buffer>> {
    /// The bind group binding whole `buffers[i]` at binding `i`.
    pub fn buffers(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        label: &str,
        buffers: &[&wgpu::Buffer],
    ) -> &wgpu::BindGroup {
        let same = self.built.as_ref().is_some_and(|(bound, _, _)| {
            bound.len() == buffers.len() && bound.iter().zip(buffers).all(|(a, b)| a == *b)
        });
        if same {
            return &self.built.as_ref().expect("checked above").1;
        }
        let key = buffers.iter().map(|b| (*b).clone()).collect();
        self.get_or_build(key, |bound| {
            let entries: Vec<_> = bound
                .iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            });
            (group, ())
        })
    }
}

const FREE: u8 = 0;
const RECORDED: u8 = 1;
const MAPPING: u8 = 2;
const READY: u8 = 3;
const FAILED: u8 = 4;

struct ReadbackSlot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    len: u64,
    sequence: u64,
}

/// GPU-to-CPU readbacks through `depth` reused `MAP_READ` buffers.
///
/// [`copy`](Self::copy) records a copy into a free slot (or reports that
/// every slot is still in flight), [`after_submit`](Self::after_submit)
/// starts mapping what was recorded, and [`read_latest`](Self::read_latest)
/// hands over the newest finished copy and recycles its slot. Nothing
/// blocks: a slot is read only once its map callback has fired, which the
/// renderer's own device polls drive. A slot's buffer is replaced only when
/// a copy outgrows it (to the next power of two).
pub struct ReadbackRing {
    label: &'static str,
    depth: usize,
    slots: Vec<ReadbackSlot>,
    next_sequence: u64,
    /// The newest sequence handed out: older copies that finish later are
    /// stale and are recycled unread.
    read_sequence: u64,
    created: u64,
}

impl ReadbackRing {
    pub fn new(label: &'static str, depth: usize) -> Self {
        Self {
            label,
            depth: depth.max(1),
            slots: Vec::new(),
            next_sequence: 1,
            read_sequence: 0,
            created: 0,
        }
    }

    /// Record a copy of `source[..len]` into a free slot. False (nothing
    /// recorded) while every slot is still in flight.
    pub fn copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Buffer,
        len: u64,
    ) -> bool {
        let len = len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        let free = self
            .slots
            .iter()
            .position(|slot| slot.state.load(Ordering::Acquire) == FREE);
        let index = match free {
            Some(index) => index,
            None if self.slots.len() < self.depth => {
                let buffer = self.make_buffer(device, len);
                self.slots.push(ReadbackSlot {
                    buffer,
                    state: Arc::new(AtomicU8::new(FREE)),
                    len: 0,
                    sequence: 0,
                });
                self.slots.len() - 1
            }
            None => return false,
        };
        if self.slots[index].buffer.size() < len {
            let buffer = self.make_buffer(device, len);
            self.slots[index].buffer = buffer;
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let slot = &mut self.slots[index];
        encoder.copy_buffer_to_buffer(source, 0, &slot.buffer, 0, len);
        slot.len = len;
        slot.sequence = sequence;
        slot.state.store(RECORDED, Ordering::Release);
        true
    }

    fn make_buffer(&mut self, device: &wgpu::Device, len: u64) -> wgpu::Buffer {
        self.created += 1;
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: len.max(wgpu::COPY_BUFFER_ALIGNMENT).next_power_of_two(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Start mapping the copies recorded since the last call. Call once the
    /// encoder that recorded them has been submitted.
    pub fn after_submit(&mut self) {
        for slot in &self.slots {
            if slot.state.load(Ordering::Acquire) != RECORDED {
                continue;
            }
            slot.state.store(MAPPING, Ordering::Release);
            let state = slot.state.clone();
            slot.buffer
                .slice(..slot.len)
                .map_async(wgpu::MapMode::Read, move |result| {
                    state.store(
                        if result.is_ok() { READY } else { FAILED },
                        Ordering::Release,
                    );
                });
        }
    }

    /// The newest finished copy, handed to `read`; its slot (and any older
    /// finished one) is recycled. `None` while none has finished.
    pub fn read_latest<R>(&mut self, read: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let mut newest: Option<usize> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            match slot.state.load(Ordering::Acquire) {
                READY if slot.sequence > self.read_sequence => {
                    if newest.is_none_or(|n| self.slots[n].sequence < slot.sequence) {
                        newest = Some(index);
                    }
                }
                READY => {
                    slot.buffer.unmap();
                    slot.state.store(FREE, Ordering::Release);
                }
                FAILED => {
                    tracing::warn!("{}: readback map failed", self.label);
                    slot.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
        let index = newest?;
        // Older finished copies are stale once a newer one is read.
        self.read_sequence = self.slots[index].sequence;
        for slot in &self.slots {
            if slot.sequence < self.read_sequence && slot.state.load(Ordering::Acquire) == READY {
                slot.buffer.unmap();
                slot.state.store(FREE, Ordering::Release);
            }
        }
        let slot = &self.slots[index];
        let result = {
            let view = slot
                .buffer
                .slice(..slot.len)
                .get_mapped_range()
                .expect("a ready readback slot is mapped");
            read(&view)
        };
        slot.buffer.unmap();
        slot.state.store(FREE, Ordering::Release);
        Some(result)
    }

    /// Buffers created so far (one per slot, plus one per outgrown copy).
    pub fn created(&self) -> u64 {
        self.created
    }
}

/// A staging pool for CPU-to-GPU writes into one encoder at a time (clones
/// share it).
///
/// A write is recorded into the caller's encoder from a reused `MAP_WRITE`
/// chunk; [`finish`](Self::finish) before that encoder is submitted,
/// [`recall`](Self::recall) after, and the chunks come back for reuse once
/// the GPU has copied from them. Chunks are created only while the writes
/// in flight outgrow the ones the pool already has.
///
/// Writers that share a pool must write into the same encoder (as a
/// renderer's scene derivations do): `finish` closes every chunk written so
/// far, and a chunk recalled while another encoder still copies from it
/// would be mapped under that encoder.
#[derive(Clone)]
pub struct Uploads {
    belt: Arc<Mutex<wgpu::util::StagingBelt>>,
}

/// Bytes in one staging chunk: one chunk holds a frame's small writes.
const UPLOAD_CHUNK_BYTES: u64 = 256 * 1024;

impl Uploads {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            belt: Arc::new(Mutex::new(wgpu::util::StagingBelt::new(
                device.clone(),
                UPLOAD_CHUNK_BYTES,
            ))),
        }
    }

    /// Record a write of `data` to `target` at `offset` (both multiples of
    /// 4 bytes; empty data writes nothing).
    pub fn write(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Buffer,
        offset: u64,
        data: &[u8],
    ) {
        let Some(size) = wgpu::BufferSize::new(data.len() as u64) else {
            return;
        };
        let mut belt = self.belt.lock().expect("upload belt poisoned");
        belt.write_buffer(encoder, target, offset, size)
            .copy_from_slice(data);
    }

    /// Close the chunks written so far; call before submitting the encoder
    /// the writes went into.
    pub fn finish(&self) {
        self.belt.lock().expect("upload belt poisoned").finish();
    }

    /// Take back the closed chunks once the GPU is done with them; call
    /// after submitting.
    pub fn recall(&self) {
        self.belt.lock().expect("upload belt poisoned").recall();
    }
}

#[cfg(all(test, feature = "render"))]
mod tests {
    use super::*;

    fn gpu() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("GPU object tests require a local adapter");
        pollster::block_on(adapter.request_device(&Default::default())).expect("a GPU device")
    }

    fn storage(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu objects test"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Frames of writes and readbacks through the pool and the ring read
    /// every value back, newest first, from the ring's first buffers.
    #[test]
    fn readbacks_and_uploads_reuse_their_buffers() {
        let (device, queue) = gpu();
        let source = storage(&device, 16);
        let uploads = Uploads::new(&device);
        let mut ring = ReadbackRing::new("gpu objects test readback", 3);
        let mut read = Vec::new();
        for frame in 0..60u32 {
            let mut encoder = device.create_command_encoder(&Default::default());
            uploads.write(&mut encoder, &source, 0, bytemuck::cast_slice(&[frame; 4]));
            ring.copy(&device, &mut encoder, &source, 16);
            uploads.finish();
            queue.submit([encoder.finish()]);
            uploads.recall();
            ring.after_submit();
            let _ = device.poll(wgpu::PollType::Poll);
            if let Some(value) = ring.read_latest(|bytes| bytemuck::cast_slice::<u8, u32>(bytes)[0])
            {
                read.push(value);
            }
        }
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        read.extend(ring.read_latest(|bytes| bytemuck::cast_slice::<u8, u32>(bytes)[0]));
        assert_eq!(read.last(), Some(&59), "the last frame's value: {read:?}");
        assert!(
            read.windows(2).all(|w| w[0] < w[1]),
            "newest first, never stale: {read:?}"
        );
        assert!(ring.created() <= 3, "{} readback buffers", ring.created());
    }

    /// A bind group is rebuilt only when a bound buffer is replaced.
    #[test]
    fn bind_groups_follow_their_buffers() {
        let (device, _queue) = gpu();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let mut cache = BindGroupCache::default();
        let first = storage(&device, 16);
        for _ in 0..10 {
            cache.buffers(&device, &layout, "test", &[&first]);
        }
        assert_eq!(cache.created(), 1);
        let second = storage(&device, 16);
        cache.buffers(&device, &layout, "test", &[&second]);
        cache.buffers(&device, &layout, "test", &[&second]);
        assert_eq!(cache.created(), 2);
    }
}
