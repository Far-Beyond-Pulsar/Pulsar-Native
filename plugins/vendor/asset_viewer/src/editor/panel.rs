use gpui::*;
use std::path::PathBuf;

pub struct AssetViewerPanel {
    pub focus_handle: FocusHandle,
    pub current_path: Option<PathBuf>,
    pub is_3d: bool,
    pub image_data: Option<(u32, u32, Vec<u8>)>,

    pub device: Option<wgpu::Device>,
    pub queue: Option<wgpu::Queue>,
    pub surface_config: Option<wgpu::SurfaceConfiguration>,
    pub surface_handle: Option<gpui::WgpuSurfaceHandle>,
    pub bind_group_layout: Option<wgpu::BindGroupLayout>,

    pub wire_pipeline: Option<wgpu::RenderPipeline>,
    pub wire_bind_group: Option<wgpu::BindGroup>,
    pub wire_uniform_buffer: Option<wgpu::Buffer>,
    pub wire_vertex_buffer: Option<wgpu::Buffer>,
    pub wire_index_count: u32,

    pub quad_pipeline: Option<wgpu::RenderPipeline>,
    pub quad_bind_group_layout: Option<wgpu::BindGroupLayout>,
    pub quad_bind_group: Option<wgpu::BindGroup>,
    pub quad_texture: Option<wgpu::Texture>,
    pub quad_sampler: Option<wgpu::Sampler>,
    pub quad_vertex_buffer: Option<wgpu::Buffer>,

    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub orbiting: bool,
    pub last_drag_pos: Option<Point<Pixels>>,
    pub needs_rebuild: bool,
}

impl AssetViewerPanel {
    pub fn new(
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let is_3d = ext == "fbx";

        let image_data = if ext == "png" {
            match image::open(&file_path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    Some((w, h, rgba.into_raw()))
                }
                Err(e) => {
                    log::error!("Failed to load PNG {:?}: {}", file_path, e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            focus_handle: cx.focus_handle(),
            current_path: Some(file_path),
            is_3d,
            image_data,
            device: None,
            queue: None,
            surface_config: None,
            surface_handle: None,
            bind_group_layout: None,
            wire_pipeline: None,
            wire_bind_group: None,
            wire_uniform_buffer: None,
            wire_vertex_buffer: None,
            wire_index_count: 0,
            quad_pipeline: None,
            quad_bind_group_layout: None,
            quad_bind_group: None,
            quad_texture: None,
            quad_sampler: None,
            quad_vertex_buffer: None,
            yaw: 0.0,
            pitch: 0.4,
            distance: 4.0,
            orbiting: false,
            last_drag_pos: None,
            needs_rebuild: true,
        }
    }
}
