//! Headless thumbnail rendering for saved material assets.

use super::{mesh, renderer::PreviewRenderer};
use crate::{features::compilation::lowering::compile_graph_to_wgsl, io::formats::deserialize_shader};
use image::RgbaImage;
use std::path::Path;

const THUMBNAIL_SIZE: u32 = 128;

/// Render a saved `.material` file using the first frame of its shader on the
/// editor's sphere preview mesh. Registered with `engine_fs` by the plugin.
pub fn render_material_thumbnail(path: &Path) -> Option<RgbaImage> {
    let graph_path = if path.is_dir() {
        path.join("shader_graph_save.json")
    } else {
        path.to_path_buf()
    };
    let contents = std::fs::read_to_string(&graph_path).ok()?;
    let asset = deserialize_shader(&contents).ok()?;
    let wgsl = compile_graph_to_wgsl(&asset.main_graph)
        .map_err(|error| {
            tracing::warn!(
                "Could not compile material thumbnail for {:?}: {}",
                path,
                error
            );
            error
        })
        .ok()?;
    render_first_frame(&wgsl)
}

fn render_first_frame(wgsl: &str) -> Option<RgbaImage> {
    match smol::block_on(async { render_first_frame_async(wgsl).await }) {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::warn!("Material thumbnail render failed: {error}");
            None
        }
    }
}

async fn render_first_frame_async(wgsl: &str) -> Result<RgbaImage, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|error| format!("No GPU adapter for material thumbnail: {error}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("material thumbnail device"),
            ..Default::default()
        })
        .await
        .map_err(|error| format!("Could not create material thumbnail device: {error}"))?;

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8Unorm,
        width: THUMBNAIL_SIZE,
        height: THUMBNAIL_SIZE,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 1,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: Vec::new(),
        color_space: wgpu::SurfaceColorSpace::Auto,
    };

    let mut renderer = PreviewRenderer::new();
    renderer.set_plain_black_background(true);
    renderer.initialize(&device, &queue, &config);
    renderer.update_shader(wgsl);
    if !renderer.has_material_pipeline() {
        return Err("Material shader did not produce a preview pipeline".into());
    }
    let sphere = mesh::generate_sphere(1.0, 32, 24);
    renderer
        .camera
        .frame_bounding_radius(sphere.bounding_radius());
    renderer.update_mesh(&sphere.vertices, &sphere.indices, sphere.index_count);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("material thumbnail target"),
        size: wgpu::Extent3d {
            width: THUMBNAIL_SIZE,
            height: THUMBNAIL_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_at_time(&target_view, 0.0);

    let bytes_per_row = THUMBNAIL_SIZE * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("material thumbnail readback"),
        size: (bytes_per_row * THUMBNAIL_SIZE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("material thumbnail readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(THUMBNAIL_SIZE),
            },
        },
        config_size(&config),
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| format!("Could not wait for material thumbnail: {error}"))?;
    rx.recv()
        .map_err(|error| format!("Material thumbnail readback failed: {error}"))?
        .map_err(|error| format!("Could not map material thumbnail pixels: {error}"))?;

    let mapped = slice
        .get_mapped_range()
        .map_err(|error| format!("Could not read material thumbnail pixels: {error}"))?;
    let mut rgba = Vec::with_capacity((THUMBNAIL_SIZE * THUMBNAIL_SIZE * 4) as usize);
    for pixel in mapped.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    drop(mapped);
    readback.unmap();

    RgbaImage::from_raw(THUMBNAIL_SIZE, THUMBNAIL_SIZE, rgba)
        .ok_or_else(|| "Material thumbnail produced an invalid pixel buffer".to_string())
}

fn config_size(config: &wgpu::SurfaceConfiguration) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: config.width,
        height: config.height,
        depth_or_array_layers: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::render_material_thumbnail;
    use crate::definitions::NodeDefinitions;
    use crate::io::formats::{serialize_shader_with_header, ShaderAsset};
    use psgc::{
        Connection, ConnectionType, GraphDescription, NodeInstance, Pin, PinInstance,
        PinType, Position,
    };
    use std::path::PathBuf;

    fn graph_node(id: &str) -> NodeInstance {
        let definition = NodeDefinitions::load()
            .get_node_definition(id)
            .unwrap_or_else(|| panic!("missing test node definition: {id}"));
        let mut node = NodeInstance::new(id, id, Position { x: 0.0, y: 0.0 });
        for input in &definition.inputs {
            let data_type = input.data_type.clone();
            node.inputs.push(PinInstance::new(
                input.id.clone(),
                Pin::new(
                    input.id.clone(),
                    input.name.clone(),
                    data_type,
                    PinType::Input,
                ),
            ));
        }
        for output in &definition.outputs {
            let data_type = output.data_type.clone();
            node.outputs.push(PinInstance::new(
                output.id.clone(),
                Pin::new(
                    output.id.clone(),
                    output.name.clone(),
                    data_type,
                    PinType::Output,
                ),
            ));
        }
        node
    }

    #[test]
    #[ignore = "requires a headless WGPU adapter; run explicitly for material thumbnail smoke validation"]
    fn saved_material_renders_its_first_frame_on_the_preview_sphere() {
        let mut graph = GraphDescription::new("thumbnail smoke test");
        let mut color = graph_node("constant_vec4");
        color.properties.insert("x".into(), serde_json::json!(1.0));
        color.properties.insert("y".into(), serde_json::json!(0.0));
        color.properties.insert("z".into(), serde_json::json!(1.0));
        color.properties.insert("w".into(), serde_json::json!(1.0));
        graph.add_node(color);
        graph.add_node(graph_node("fragment_output"));
        graph.add_connection(Connection::new(
            "constant_vec4",
            "result",
            "fragment_output",
            "base_color",
            ConnectionType::Data,
        ));

        let asset = ShaderAsset::with_graph(graph);
        let serialized = serialize_shader_with_header(&asset).expect("serialize test material");
        let path = std::env::temp_dir().join(format!(
            "pulsar-material-thumbnail-{}.material",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, serialized).expect("write test material");

        let thumbnail = render_material_thumbnail(&path);
        let _ = std::fs::remove_file(PathBuf::from(&path));
        let thumbnail = thumbnail.expect("material hook should render the test material");
        assert_eq!(thumbnail.dimensions(), (128, 128));
        assert!(thumbnail
            .pixels()
            .any(|pixel| pixel[0] > 180 && pixel[2] > 180 && pixel[1] < 80));
    }
}
