use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct SceneStats {
    pub name: String,
    pub generator: String,
    pub mesh_count: usize,
    pub total_vertices: u32,
    pub total_indices: u32,
    pub material_count: usize,
    pub texture_count: usize,
    pub image_count: usize,
    pub light_count: usize,
    pub camera_count: usize,
    pub animation_count: usize,
    pub skin_count: usize,
    pub morph_target_count: usize,
    pub has_skin: bool,
    pub has_animations: bool,
    pub total_joints: usize,
    pub meshes: Vec<MeshProps>,
}

#[derive(Debug, Clone)]
pub struct MeshProps {
    pub name: String,
    pub vertex_count: u32,
    pub index_count: u32,
    pub triangle_count: u32,
    pub primitive_count: usize,
    pub morph_count: usize,
    pub has_normals: bool,
    pub has_tangents: bool,
    pub has_uvs: bool,
    pub has_vertex_colors: bool,
    pub has_skin: bool,
    pub material_name: String,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

pub struct AssetViewerPanel {
    pub focus_handle: FocusHandle,
    pub current_path: Option<PathBuf>,
    pub is_3d: bool,
    pub image_data: Option<(u32, u32, Vec<u8>)>,
    pub modified: bool,
    pub save_path: Option<PathBuf>,
    pub tab_title: Option<String>,
    pub workspace: Option<Entity<ui::workspace::Workspace>>,
    pub subscriptions: Vec<Subscription>,

    pub device: Option<wgpu::Device>,
    pub queue: Option<wgpu::Queue>,
    pub surface_config: Option<wgpu::SurfaceConfiguration>,
    pub surface_handle: Option<gpui::WgpuSurfaceHandle>,

    pub wire_vertex_buffer: Option<wgpu::Buffer>,
    pub wire_index_count: u32,
    pub wire_pipeline: Option<wgpu::RenderPipeline>,
    pub wire_bind_group: Option<wgpu::BindGroup>,
    pub wire_uniform_buffer: Option<wgpu::Buffer>,

    pub depth_texture: Option<wgpu::Texture>,
    pub depth_view: Option<wgpu::TextureView>,

    pub mesh_vertex_buffer: Option<wgpu::Buffer>,
    pub mesh_index_buffer: Option<wgpu::Buffer>,
    pub mesh_index_count: u32,
    pub mesh_props: Vec<MeshProps>,
    pub scene_stats: SceneStats,
    pub mesh_pipeline: Option<wgpu::RenderPipeline>,
    pub mesh_bind_group: Option<wgpu::BindGroup>,
    pub mesh_uniform_buffer: Option<wgpu::Buffer>,

    pub quad_pipeline: Option<wgpu::RenderPipeline>,
    pub quad_bind_group_layout: Option<wgpu::BindGroupLayout>,
    pub quad_bind_group: Option<wgpu::BindGroup>,
    pub quad_texture: Option<wgpu::Texture>,
    pub quad_sampler: Option<wgpu::Sampler>,
    pub quad_vertex_buffer: Option<wgpu::Buffer>,

    pub checker_pipeline: Option<wgpu::RenderPipeline>,
    pub checker_bind_group_layout: Option<wgpu::BindGroupLayout>,
    pub checker_bind_group: Option<wgpu::BindGroup>,
    pub checker_uniform_buffer: Option<wgpu::Buffer>,

    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub orbiting: bool,
    pub last_drag_pos: Option<Point<Pixels>>,
    pub orbit_target: [f32; 3],
    pub move_speed: f32,
    pub keys: [bool; 6],
    pub needs_rebuild: bool,

    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub panning: bool,
    pub last_pan_pos: Option<Point<Pixels>>,

    pub undo_stack: Vec<(u32, u32, Vec<u8>)>,
    pub redo_stack: Vec<(u32, u32, Vec<u8>)>,
}

impl AssetViewerPanel {
    pub fn save_image(&self) -> Result<(), String> {
        let Some((w, h, ref pixels)) = self.image_data else {
            return Err("No image loaded".into());
        };
        let path = self.save_path.as_ref().ok_or("No save path")?;
        let img =
            image::RgbaImage::from_raw(w, h, pixels.clone()).ok_or("Failed to create image")?;
        img.save(path).map_err(|e| format!("Save failed: {e}"))
    }

    pub fn zoom_to_fit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((img_w, img_h, _)) = self.image_data else {
            return;
        };
        let Some(surface) = &self.surface_handle else {
            return;
        };
        let (vw, vh) = surface.size();
        if vw == 0 || vh == 0 {
            return;
        }
        let fit = (vw as f32 / img_w as f32).min(vh as f32 / img_h as f32);
        self.zoom = 1.0;
        self.pan_x = ((vw as f32 - img_w as f32 * fit) * 0.5).max(0.0);
        self.pan_y = ((vh as f32 - img_h as f32 * fit) * 0.5).max(0.0);
        cx.notify();
    }

    pub fn commit_edit(&mut self) {
        if let Some(d) = self.image_data.as_ref() {
            self.undo_stack.push(d.clone());
            self.redo_stack.clear();
            self.modified = true;
        }
    }

    fn edit_apply(&mut self) {
        self.reupload_texture();
    }

    pub fn undo(&mut self) {
        let Some(d) = self.undo_stack.pop() else {
            return;
        };
        if let Some(current) = self.image_data.take() {
            self.redo_stack.push(current);
        }
        self.image_data = Some(d);
        self.modified = true;
        self.edit_apply();
    }

    pub fn redo(&mut self) {
        let Some(d) = self.redo_stack.pop() else {
            return;
        };
        if let Some(current) = self.image_data.take() {
            self.undo_stack.push(current);
        }
        self.image_data = Some(d);
        self.modified = true;
        self.edit_apply();
    }

    pub fn rotate_ccw(&mut self) {
        self.commit_edit();
        let Some((w, h, ref pixels)) = self.image_data else {
            return;
        };
        let mut out = Vec::with_capacity(pixels.len());
        for x in (0..w).rev() {
            for y in 0..h {
                let i = ((y * w + x) * 4) as usize;
                out.extend_from_slice(&pixels[i..i + 4]);
            }
        }
        self.image_data = Some((h, w, out));
        self.edit_apply();
    }

    pub fn rotate_90(&mut self) {
        self.commit_edit();
        let Some((w, h, ref pixels)) = self.image_data else {
            return;
        };
        let mut out = Vec::with_capacity(pixels.len());
        for x in 0..w {
            for y in (0..h).rev() {
                let i = ((y * w + x) * 4) as usize;
                out.extend_from_slice(&pixels[i..i + 4]);
            }
        }
        self.image_data = Some((h, w, out));
        self.edit_apply();
    }

    pub fn flip_h(&mut self) {
        self.commit_edit();
        let Some((w, h, ref pixels)) = self.image_data else {
            return;
        };
        let mut out = pixels.clone();
        for y in 0..h {
            for x in 0..w / 2 {
                let a = ((y * w + x) * 4) as usize;
                let b = ((y * w + (w - 1 - x)) * 4) as usize;
                for c in 0..4 {
                    out.swap(a + c, b + c);
                }
            }
        }
        self.image_data = Some((w, h, out));
        self.edit_apply();
    }

    pub fn flip_v(&mut self) {
        self.commit_edit();
        let Some((w, h, ref pixels)) = self.image_data else {
            return;
        };
        let mut out = pixels.clone();
        for y in 0..h / 2 {
            for x in 0..w {
                let a = ((y * w + x) * 4) as usize;
                let b = (((h - 1 - y) * w + x) * 4) as usize;
                for c in 0..4 {
                    out.swap(a + c, b + c);
                }
            }
        }
        self.image_data = Some((w, h, out));
        self.edit_apply();
    }

    pub fn grayscale(&mut self) {
        self.commit_edit();
        let Some((_w, _h, ref mut pixels)) = self.image_data else {
            return;
        };
        for pixel in pixels.chunks_exact_mut(4) {
            let l =
                (pixel[0] as f32 * 0.299 + pixel[1] as f32 * 0.587 + pixel[2] as f32 * 0.114) as u8;
            pixel[0] = l;
            pixel[1] = l;
            pixel[2] = l;
        }
        self.edit_apply();
    }

    pub fn invert(&mut self) {
        self.commit_edit();
        let Some((_w, _h, ref mut pixels)) = self.image_data else {
            return;
        };
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[0] = 255 - pixel[0];
            pixel[1] = 255 - pixel[1];
            pixel[2] = 255 - pixel[2];
        }
        self.edit_apply();
    }

    pub fn adjust_brightness(&mut self, delta: i16) {
        self.commit_edit();
        let Some((_w, _h, ref mut pixels)) = self.image_data else {
            return;
        };
        for pixel in pixels.chunks_exact_mut(4) {
            for c in 0..3 {
                let v = pixel[c] as i16 + delta;
                pixel[c] = v.clamp(0, 255) as u8;
            }
        }
        self.edit_apply();
    }

    pub fn adjust_contrast(&mut self, factor: f32) {
        self.commit_edit();
        let Some((_w, _h, ref mut pixels)) = self.image_data else {
            return;
        };
        for pixel in pixels.chunks_exact_mut(4) {
            for c in 0..3 {
                let v = ((pixel[c] as f32 - 128.0) * factor + 128.0).clamp(0.0, 255.0) as u8;
                pixel[c] = v;
            }
        }
        self.edit_apply();
    }

    pub fn resize(&mut self, new_w: u32, new_h: u32) {
        self.commit_edit();
        let Some((w, h, ref pixels)) = self.image_data else {
            return;
        };
        if w == new_w && h == new_h {
            return;
        }
        let img = image::RgbaImage::from_raw(w, h, pixels.clone()).unwrap();
        let resized =
            image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Lanczos3);
        self.image_data = Some((new_w, new_h, resized.into_raw()));
        self.edit_apply();
    }

    fn zoom_to_fit_after_edit(&mut self) {
        if let (Some((w, h, _)), Some(surface)) = (&self.image_data, &self.surface_handle) {
            let (vw, vh) = surface.size();
            if vw > 0 && vh > 0 {
                let fit = (vw as f32 / *w as f32).min(vh as f32 / *h as f32);
                let prev_fit =
                    (vw as f32 / (*w as f32 / self.zoom)).min(vh as f32 / (*h as f32 / self.zoom));
                let zoom_ratio = if prev_fit > 0.0 { fit / prev_fit } else { 1.0 };
                self.zoom = (self.zoom * zoom_ratio).clamp(0.01, 100.0);
                let dw = *w as f32 * fit * self.zoom;
                let dh = *h as f32 * fit * self.zoom;
                self.pan_x = ((vw as f32 - dw) * 0.5).max(0.0);
                self.pan_y = ((vh as f32 - dh) * 0.5).max(0.0);
            }
        }
    }

    pub fn new(file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        let tab_title = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());

        Self {
            focus_handle: cx.focus_handle(),
            current_path: Some(file_path.clone()),
            is_3d,
            image_data,
            modified: false,
            save_path: Some(file_path),
            tab_title,
            workspace: None,
            subscriptions: Vec::new(),
            device: None,
            queue: None,
            surface_config: None,
            surface_handle: None,
            wire_vertex_buffer: None,
            wire_index_count: 0,
            wire_pipeline: None,
            wire_bind_group: None,
            wire_uniform_buffer: None,
            depth_texture: None,
            depth_view: None,
            mesh_vertex_buffer: None,
            mesh_index_buffer: None,
            mesh_index_count: 0,
            mesh_props: Vec::new(),
            scene_stats: SceneStats::default(),
            mesh_pipeline: None,
            mesh_bind_group: None,
            mesh_uniform_buffer: None,
            quad_pipeline: None,
            quad_bind_group_layout: None,
            quad_bind_group: None,
            quad_texture: None,
            quad_sampler: None,
            quad_vertex_buffer: None,
            checker_pipeline: None,
            checker_bind_group_layout: None,
            checker_bind_group: None,
            checker_uniform_buffer: None,
            yaw: std::f32::consts::PI,
            pitch: 0.0,
            distance: 4.0,
            orbiting: false,
            last_drag_pos: None,
            orbit_target: [0.0, 0.0, 0.0],
            move_speed: 0.5,
            keys: [false; 6],
            needs_rebuild: true,
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            panning: false,
            last_pan_pos: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}
