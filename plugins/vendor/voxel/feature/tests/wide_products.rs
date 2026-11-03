//! The hardware 64-bit wide products (`wide64.wgsl`) give the same bits as
//! the 16-bit limb versions they replace (noise.wgsl's wide region).
mod common;
use common::{gpu, read_buffer};
use wgpu::util::DeviceExt;

#[test]
fn hardware_wide_products_match_the_limb_versions() {
    let Some(gpu) = gpu() else { return };
    if !gpu.device.features().contains(wgpu::Features::SHADER_INT64) {
        eprintln!("SKIP: no 64-bit shader integers");
        return;
    }
    let noise = helio_voxel_world::wgsl::NOISE;
    let (_, rest) = noise.split_once("// wide:begin").unwrap();
    let (limbs, _) = rest.split_once("// wide:end").unwrap();
    let hardware = include_str!("../shaders/wide64.wgsl")
        .replace("fn mul_wide(", "fn hw_wide(")
        .replace("fn mul_shr(", "fn hw_shr(")
        .replace("fn mul_shr_signed(", "fn hw_signed(");
    let source = format!(
        "{limbs}\n{hardware}\n
        @group(0) @binding(0) var<storage, read> inputs: array<vec4<u32>>;
        @group(0) @binding(1) var<storage, read_write> mismatches: array<atomic<u32>>;
        @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
            if id.x >= arrayLength(&inputs) {{ return; }}
            let v = inputs[id.x];
            if any(mul_wide(v.x, v.y) != hw_wide(v.x, v.y)) {{ atomicAdd(&mismatches[0], 1u); }}
            if mul_shr(v.x, v.y, v.z) != hw_shr(v.x, v.y, v.z) {{ atomicAdd(&mismatches[1], 1u); }}
            let signed = mul_shr_signed(bitcast<i32>(v.x), bitcast<i32>(v.y), v.z);
            if signed != hw_signed(bitcast<i32>(v.x), bitcast<i32>(v.y), v.z) {{ atomicAdd(&mismatches[2], 1u); }}
            if signed != bitcast<i32>(v.w) {{ atomicAdd(&mismatches[3], 1u); }}
        }}"
    );
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wide products"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    // Random operands, the extremes, and every shift the callers use.
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng as u32
    };
    let mut inputs: Vec<[u32; 4]> = (0..200_000).map(|n| [next(), next(), n % 64, 0]).collect();
    for (a, b) in [
        (0, 0),
        (u32::MAX, u32::MAX),
        (u32::MAX, 1),
        (1 << 31, 1 << 31),
        (0xffff, 0x10000),
    ] {
        for s in 0..64 {
            inputs.push([a, b, s, 0]);
        }
    }
    // The CPU's signed product (`noise::mul_shr_signed`), in the low word.
    for v in &mut inputs {
        v[3] =
            helio_pass_voxel_planet::noise::mul_shr_signed(v[0] as i32, v[1] as i32, v[2]) as u32;
    }
    let input = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&inputs),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let output = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0u8; 16],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
    let pipeline = gpu
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((inputs.len() as u32).div_ceil(64), 1, 1);
    }
    gpu.queue.submit([encoder.finish()]);
    let bytes = read_buffer(&gpu, &output, 16);
    let count = |k: usize| u32::from_le_bytes(bytes[4 * k..4 * k + 4].try_into().unwrap());
    assert_eq!(
        (count(0), count(1), count(2), count(3)),
        (0, 0, 0, 0),
        "mul_wide, mul_shr, mul_shr_signed (hardware, CPU) mismatches over {} inputs",
        inputs.len()
    );
}
