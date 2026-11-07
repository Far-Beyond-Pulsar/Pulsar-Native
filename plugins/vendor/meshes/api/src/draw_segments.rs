//! Per-material draw segments (Helio#330, part 3).
//!
//! A draw pass needs one pipeline per `(material_class, graph_hash)` and a
//! CPU-side offset to draw from. The GPU range tables are current, but the CPU
//! only sees them through `helio-pass-object-batch`'s async readback, two or
//! more frames late. Drawing a late range table against current buffers draws
//! some groups with a neighbouring material's pipeline whenever the layout
//! shifts.
//!
//! Segments fix the offsets instead. `helio-pass-occlusion-cull` keeps a table
//! of every material key it has seen, each with a fixed region (`first`,
//! `capacity`) of [`DrawSegments::indirect`]. Range compaction appends each
//! range's surviving draws to its key's region and counts them in
//! [`DrawSegments::counts`], in the same frame. Passes draw each segment of
//! their bucket with its pipeline at its fixed offset, so the table only being
//! late means a key seen for the first time is skipped until it is added, never
//! drawn with the wrong pipeline. The table changes only when keys appear,
//! disappear or outgrow their region, so the recorded commands stay the same
//! frame to frame.

/// Which passes draw a segment, from the material's shading flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ShadingBucket {
    /// Deferred: GBuffer, or ForwardLit when it renders all opaque geometry.
    Opaque = 0,
    /// `FLAG_TRANSPARENT_ONLY`: the transparent pass.
    Transparent = 1,
    /// `FLAG_FORWARD_SHADING`: ForwardLit's forward-only mode.
    Forward = 2,
}

/// One material key's fixed region of [`DrawSegments::indirect`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawSegment {
    /// Packed template class (low 16 bits) and raster flags (upper 16).
    pub material_class: u32,
    pub graph_hash: u64,
    pub bucket: ShadingBucket,
    /// First indirect record of the region.
    pub first: u32,
    /// Records in the region; also the most a draw of it reads.
    pub capacity: u32,
}

impl DrawSegment {
    pub fn template_class(&self) -> u32 {
        self.material_class & 0xffff
    }
    pub fn raster_flags(&self) -> u32 {
        self.material_class >> 16
    }
}

/// This frame's per-material draw segments.
#[derive(Clone, Copy)]
pub struct DrawSegments<'a> {
    /// Indirect draw records, packed per segment. Records past a segment's
    /// count have zero `instance_count`, for adapters without indirect-count
    /// draws.
    pub indirect: &'a wgpu::Buffer,
    /// One `u32` survivor count per segment, written by the GPU this frame;
    /// `None` when the device lacks `MULTI_DRAW_INDIRECT_COUNT`.
    pub counts: Option<&'a wgpu::Buffer>,
    pub segments: &'a [DrawSegment],
    /// The GPU segment table (`GpuSegment` rows, one per segment), for passes that
    /// compact their own indirect records into the same segments.
    pub table: &'a wgpu::Buffer,
}

impl<'a> DrawSegments<'a> {
    /// Whether any key is known yet. Before the first readback there is none,
    /// and passes fall back to drawing every group with the default pipeline.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The segments of `bucket`, with their indices.
    pub fn in_bucket(
        &self,
        bucket: ShadingBucket,
    ) -> impl Iterator<Item = (usize, &'a DrawSegment)> + 'a {
        self.segments
            .iter()
            .enumerate()
            .filter(move |(_, segment)| segment.bucket == bucket)
    }

    /// GPU count for `segments[index]`.
    pub fn count_slot(&self, index: usize) -> Option<crate::GpuDrawCount<'a>> {
        self.counts.map(|buffer| crate::GpuDrawCount {
            buffer,
            offset: index as u64 * 4,
        })
    }

    /// Draws `segments[index]` with whatever pipeline the caller has set.
    pub fn draw(&self, pass: &mut helio_core::RenderCmds<'_>, index: usize) {
        let segment = &self.segments[index];
        crate::multi_draw_indexed_indirect(
            pass,
            self.indirect,
            segment.first,
            segment.capacity,
            self.count_slot(index),
        );
    }
}
