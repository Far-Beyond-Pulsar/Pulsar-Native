mod support;

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn make_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

fn read_u32(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<u32> {
    let readback = make_buffer(
        device,
        "Range compaction readback",
        source.size(),
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, source.size());
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let result = bytemuck::cast_slice::<u8, u32>(&mapped).to_vec();
    drop(mapped);
    readback.unmap();
    result
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// One indirect record: `instance_count` 0 or 1, tagged by `first_instance`.
fn record(alive: bool, tag: u32) -> [u32; 5] {
    [3, alive as u32, 0, 0, tag]
}

/// A range of `material_class` (graph hash 0) over `count` records at `start`.
fn range(material_class: u32, start: u32, count: u32) -> [u32; 5] {
    [material_class, 0, 0, start, count]
}

/// An opaque segment of `material_class` (graph hash 0).
fn segment(material_class: u32, first: u32, capacity: u32) -> [u32; 8] {
    [material_class, 0, 0, 0, first, capacity, 0, 0]
}

struct Output {
    compacted: Vec<u32>,
    draw_counts: Vec<u32>,
    segment_indirect: Vec<u32>,
    segment_counts: Vec<u32>,
}

impl Output {
    /// `first_instance` tags of `segment_indirect[first..first + len]`,
    /// sorted (order within a segment has no meaning).
    fn segment_tags(&self, first: u32, len: u32) -> Vec<u32> {
        let mut tags: Vec<u32> = (first..first + len)
            .map(|i| self.segment_indirect[i as usize * 5 + 4])
            .collect();
        tags.sort_unstable();
        tags
    }
}

/// Runs `compact_ranges.wgsl` over opaque `ranges` of `records`, with the
/// given segment table.
fn compact(records: &[[u32; 5]], ranges: &[[u32; 5]], segments: &[[u32; 8]]) -> Option<Output> {
    pollster::block_on(async {
        let (device, queue) = support::request_test_device("GPU Range Compaction").await?;

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Range compaction test BGL"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, false),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(8, true),
                storage_entry(9, false),
                storage_entry(10, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Range compaction test PL"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Range compaction test shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/compact_ranges.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Range compaction test pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compact_ranges"),
            compilation_options: Default::default(),
            cache: None,
        });

        let rw = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST;
        let commands: Vec<u32> = records.iter().flatten().copied().collect();
        let source = make_buffer(
            &device,
            "Compaction source indirect",
            commands.len() as u64 * 4,
            rw,
        );
        let compacted = make_buffer(&device, "Compaction output indirect", source.size(), rw);
        let range_counts = make_buffer(&device, "Compaction range counts", 16, rw);
        let range_words: Vec<u32> = ranges.iter().flatten().copied().collect();
        let range_table = make_buffer(
            &device,
            "Compaction range tables",
            range_words.len() as u64 * 4,
            rw,
        );
        let slots = ranges.len() as u32;
        let draw_counts = make_buffer(
            &device,
            "Compaction draw counts",
            (4 + 3 * slots as u64) * 4,
            rw,
        );
        let params = make_buffer(
            &device,
            "Compaction params",
            3 * 256,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let segment_words: Vec<u32> = segments.iter().flatten().copied().collect();
        let segment_table = make_buffer(
            &device,
            "Segment table",
            (segment_words.len().max(8) * 4) as u64,
            rw,
        );
        let segment_records: u32 = segments.iter().map(|s| s[4] + s[5]).max().unwrap_or(1);
        let segment_indirect =
            make_buffer(&device, "Segment indirect", segment_records as u64 * 20, rw);
        let segment_counts = make_buffer(
            &device,
            "Segment counts",
            (segments.len().max(4) * 4) as u64,
            rw,
        );

        queue.write_buffer(&source, 0, bytemuck::cast_slice(&commands));
        queue.write_buffer(
            &range_counts,
            0,
            bytemuck::cast_slice(&[slots, 0, 0, slots]),
        );
        queue.write_buffer(&range_table, 0, bytemuck::cast_slice(&range_words));
        if !segment_words.is_empty() {
            queue.write_buffer(&segment_table, 0, bytemuck::cast_slice(&segment_words));
        }
        for bucket in 0..3u32 {
            queue.write_buffer(
                &params,
                bucket as u64 * 256,
                bytemuck::cast_slice(&[slots, bucket, segments.len() as u32, 0]),
            );
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Range compaction test BG"),
            layout: &layout,
            entries: &[
                entry(0, &source),
                entry(1, &compacted),
                entry(2, &range_counts),
                entry(3, &range_table),
                entry(4, &range_table),
                entry(5, &range_table),
                entry(6, &draw_counts),
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params,
                        offset: 0,
                        size: std::num::NonZeroU64::new(16),
                    }),
                },
                entry(8, &segment_table),
                entry(9, &segment_indirect),
                entry(10, &segment_counts),
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(&source, 0, &compacted, 0, source.size());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            for bucket in 0..3u32 {
                pass.set_bind_group(0, &bg, &[bucket * 256]);
                // Only opaque ranges exist; the other buckets return at once.
                pass.dispatch_workgroups(if bucket == 0 { slots } else { 1 }, 1, 1);
            }
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

        Some(Output {
            compacted: read_u32(&device, &queue, &compacted),
            draw_counts: read_u32(&device, &queue, &draw_counts),
            segment_indirect: read_u32(&device, &queue, &segment_indirect),
            segment_counts: read_u32(&device, &queue, &segment_counts),
        })
    })
}

#[test]
fn compacts_surviving_indirect_draws_and_writes_per_range_count() {
    let records = [
        record(true, 10),
        record(false, 11),
        record(true, 12),
        record(false, 13),
    ];
    let Some(out) = compact(&records, &[range(0, 0, 4)], &[]) else {
        eprintln!("skipping range compaction test: no GPU adapter available");
        return;
    };
    let args = &out.compacted;
    assert_eq!(args[1], 1);
    assert_eq!(args[6], 1);
    assert_eq!(args[11], 0);
    assert_eq!(args[16], 0);
    let mut first_instances = [args[4], args[9]];
    first_instances.sort_unstable();
    assert_eq!(
        first_instances,
        [10, 12],
        "surviving args are packed at range head"
    );
    assert_eq!(out.draw_counts[4], 2);
    assert_eq!(
        out.segment_counts[0], 0,
        "no segment table: nothing is appended"
    );
}

/// Every range's survivors land in their material key's segment at the
/// segment's fixed offset, whatever this frame's range layout is: ranges that
/// share a key append to one segment, and a key the table does not know yet is
/// skipped rather than drawn under another key's pipeline.
#[test]
fn survivors_land_in_their_key_segment_at_its_fixed_offset() {
    let records = [
        // Range 0, class 7: two of three survive.
        record(true, 70),
        record(false, 71),
        record(true, 72),
        // Range 1, class 3: one survives.
        record(true, 30),
        // Range 2, class 7 again (the sort interleaves keys by mesh).
        record(true, 73),
        // Range 3, class 9: not in the table yet.
        record(true, 90),
    ];
    let ranges = [
        range(7, 0, 3),
        range(3, 3, 1),
        range(7, 4, 1),
        range(9, 5, 1),
    ];
    // Table order differs from range order on purpose.
    let segments = [segment(3, 0, 64), segment(7, 64, 64)];
    let Some(out) = compact(&records, &ranges, &segments) else {
        eprintln!("skipping segment test: no GPU adapter available");
        return;
    };

    assert_eq!(out.segment_counts[0], 1, "class 3 segment count");
    assert_eq!(
        out.segment_counts[1], 3,
        "class 7 gathers both of its ranges"
    );
    assert_eq!(out.segment_tags(0, 1), [30]);
    assert_eq!(out.segment_tags(64, 3), [70, 72, 73]);
    for segment_first in [0u32, 64] {
        let len = if segment_first == 0 { 1 } else { 3 };
        for i in segment_first..segment_first + len {
            assert_eq!(
                out.segment_indirect[i as usize * 5 + 1],
                1,
                "copied records stay alive"
            );
        }
    }
    // Everything past each count stays empty: class 9 went nowhere.
    let written: Vec<u32> = (0..128u32)
        .filter(|&i| out.segment_indirect[i as usize * 5 + 1] != 0)
        .map(|i| out.segment_indirect[i as usize * 5 + 4])
        .collect();
    assert!(
        !written.contains(&90),
        "an unknown key is skipped: {written:?}"
    );
    assert_eq!(written.len(), 4);
    // In-place compaction and per-range counts are unchanged.
    assert_eq!(&out.draw_counts[4..8], &[2, 1, 1, 1]);
}

/// Survivors beyond a segment's capacity are dropped instead of spilling
/// into the next segment; the count still reports them, and draws read at
/// most `capacity` records.
#[test]
fn a_full_segment_never_spills_into_the_next() {
    let records: Vec<[u32; 5]> = (0..5).map(|i| record(true, 100 + i)).collect();
    let segments = [segment(1, 0, 2), segment(2, 2, 64)];
    let Some(out) = compact(&records, &[range(1, 0, 5)], &segments) else {
        eprintln!("skipping segment overflow test: no GPU adapter available");
        return;
    };
    assert_eq!(out.segment_counts[0], 5);
    assert_eq!(out.segment_counts[1], 0);
    for i in 2..66usize {
        assert_eq!(
            out.segment_indirect[i * 5 + 1],
            0,
            "record {i} of the next segment"
        );
    }
    let kept = out.segment_tags(0, 2);
    assert!(kept.iter().all(|tag| (100..105).contains(tag)), "{kept:?}");
}
