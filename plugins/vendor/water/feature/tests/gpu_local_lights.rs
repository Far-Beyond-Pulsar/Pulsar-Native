//! Exercise the actual water lighting WGSL without atmosphere or sunlight.
use wgpu::util::DeviceExt;

#[test]
fn local_lights_illuminate_water_without_a_sun() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: water local lighting");
            return;
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("test device");
        // The water's own lighting snippet over the lights' row and the
        // atmosphere frame (no atmosphere: planet.w 0), as the surface
        // shader composes them.
        let source = r#"//!use helio_prelude
//!use light_list
//!use atmosphere
//!use water_lights
@group(0) @binding(0) var<storage, read> water_lights: array<GpuLight>;
@group(0) @binding(1) var<storage, read_write> output: array<vec4f, 2>;
@group(0) @binding(2) var<uniform> atmosphere: AtmosphereFrame;
@compute @workgroup_size(1)
fn main() {
    // The primary sun straight up: a directional light along it is the
    // sun's own path, not a local light.
    let lit = water_local_lighting(vec3f(0.0), vec3f(0.0, 1.0, 0.0),
        vec3f(0.0, 1.0, 0.0), vec3f(0.0, -1.0, 0.0), 0.1, vec3f(0.0, 1.0, 0.0));
    output[0] = vec4f(lit.specular, 0.0);
    output[1] = vec4f(lit.diffuse, 0.0);
}
"#;
        let shader = helio_core::shader::module_with(
            &device,
            "Water local light regression",
            source,
            &[
                helio_light_api::LIGHT_LIST_SNIPPET,
                helio_frame_api::lighting::sky::ATMOSPHERE_SNIPPET,
                helio_pass_water_sim::pipeline::WATER_LIGHTS_SNIPPET,
            ],
        );
        let atmosphere = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: helio_frame_api::lighting::sky::ATMOSPHERE_FRAME_BYTES,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        // Two 128-byte GpuLight rows: an empty row first also verifies stride
        // and that sparse SceneDB slots don't hide later live lights.
        let lights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 256],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: atmosphere.as_entire_binding(),
                },
            ],
        });
        let evaluate = |row: [f32; 32]| -> [f32; 8] {
            queue.write_buffer(&lights, 128, bytemuck::cast_slice(&row));
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 32);
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let data = readback.slice(..).get_mapped_range().unwrap();
            let result = bytemuck::cast_slice::<u8, f32>(&data).try_into().unwrap();
            drop(data);
            readback.unmap();
            result
        };
        let mut point = [0.0; 32];
        point[1] = 2.0; // position.y
        point[3] = 10.0; // range
        point[5] = -1.0; // direction.y
        point[7] = 0.8; // outer cone cosine
        point[8] = 1.0; // red
        point[11] = 4.0; // intensity
        point[13] = f32::from_bits(1); // point light type
        point[14] = 0.9; // inner cone cosine
        let lit = evaluate(point);
        assert!(
            lit[0] > 0.0 && lit[4] > 0.0,
            "point light must light water and foam: {lit:?}"
        );
        assert_eq!([lit[1], lit[2], lit[5], lit[6]], [0.0; 4]);
        let mut farther = point;
        farther[1] = 4.0;
        let far = evaluate(farther);
        assert!(
            far[0] < lit[0] && far[4] < lit[4],
            "distance must attenuate light"
        );
        let mut spot = point;
        spot[13] = f32::from_bits(2);
        assert!(
            evaluate(spot)[0] > 0.0,
            "spotlight center must illuminate water"
        );
        spot[5] = 1.0;
        assert_eq!(
            evaluate(spot),
            [0.0; 8],
            "spot pointing away must not illuminate water"
        );
        for lane in [3, 11, 13] {
            // zero range, disabled, or along the primary sun (its own path)
            let mut dark = point;
            dark[lane] = 0.0;
            assert_eq!(evaluate(dark), [0.0; 8]);
        }
        point[1] = 11.0;
        assert_eq!(evaluate(point), [0.0; 8], "outside range");
        point[1] = 0.0;
        assert!(
            evaluate(point).iter().all(|v| v.is_finite()),
            "coincident light must remain finite"
        );
    });
}
