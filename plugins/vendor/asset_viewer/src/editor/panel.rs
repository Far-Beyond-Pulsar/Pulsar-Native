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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MeshRenderMode {
    #[default]
    Lit,
    Unlit,
    Wireframe,
    /// Object-space normals as colour.
    Normals,
    /// UV channel 1 (`tex_coords0`) sampled against the embedded UV reference grid.
    Uv0,
    /// UV channel 2 (`tex_coords1`, the lightmap channel) sampled against the UV grid.
    Uv1,
    /// Relative local vertex packing, from green (sparse) to red (dense).
    VertexDensity,
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

    pub wire_index_buffer: Option<wgpu::Buffer>,
    pub wire_index_count: u32,
    pub wire_pipeline: Option<wgpu::RenderPipeline>,

    pub depth_texture: Option<wgpu::Texture>,
    pub depth_view: Option<wgpu::TextureView>,

    pub mesh_vertex_buffer: Option<wgpu::Buffer>,
    pub mesh_index_buffer: Option<wgpu::Buffer>,
    pub mesh_index_count: u32,
    pub mesh_vertex_count: u32,
    pub mesh_props: Vec<MeshProps>,
    pub scene_stats: SceneStats,
    pub mesh_pipeline: Option<wgpu::RenderPipeline>,
    pub density_pipeline: Option<wgpu::RenderPipeline>,
    pub density_vertex_buffer: Option<wgpu::Buffer>,
    pub density_values: Option<Vec<f32>>,
    pub density_progress: Option<f32>,
    pub density_error: Option<String>,
    pub density_job_id: u64,
    pub density_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub density_task: Option<Task<()>>,
    pub mesh_bind_group: Option<wgpu::BindGroup>,
    pub uv_grid_texture: Option<wgpu::Texture>,
    pub uv_grid_view: Option<wgpu::TextureView>,
    pub uv_grid_sampler: Option<wgpu::Sampler>,
    pub uv_grid_bind_group: Option<wgpu::BindGroup>,
    pub uv_grid_bgl: Option<wgpu::BindGroupLayout>,
    pub mesh_uniform_buffer: Option<wgpu::Buffer>,
    pub render_mode: MeshRenderMode,

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

    /// Default-material pickers; set only for native `.mesh` files.
    pub mesh_materials: Option<super::materials::MeshMaterials>,
    /// `(first_index, index_count, material_slot)` per drawn section of a `.mesh`.
    pub mesh_sections: Vec<(u32, u32, usize)>,
    /// Imported surface of each `.mesh` slot (what an empty assignment draws).
    pub slot_surfaces: Vec<helio_component::mesh_cache::ImportedSurfaceMaterial>,
    /// Preview colour of each slot's current material.
    pub slot_colors: Vec<[f32; 4]>,
    /// Compiled shader-graph material per slot (None: plain colour).
    pub graph_draws: Vec<Option<super::graph_material::GraphDraw>>,
    /// Layout of the per-draw uniforms, shared with graph pipelines.
    pub mesh_bgl: Option<wgpu::BindGroupLayout>,
    pub empty_bind_group: Option<wgpu::BindGroup>,
    /// The template's `Globals` (frame + graph clock) for graph materials.
    pub globals_buffer: Option<wgpu::Buffer>,
    pub globals_layout: Option<wgpu::BindGroupLayout>,
    pub globals_bind_group: Option<wgpu::BindGroup>,
    pub frame_counter: u32,
}

impl AssetViewerPanel {
    pub fn set_render_mode(&mut self, mode: MeshRenderMode, cx: &mut Context<Self>) {
        if self.render_mode == mode {
            return;
        }
        if self.render_mode == MeshRenderMode::VertexDensity {
            self.cancel_density_view();
        }
        self.render_mode = mode;
        if mode == MeshRenderMode::VertexDensity {
            self.begin_density_view(cx);
        }
        cx.notify();
    }

    fn cancel_density_view(&mut self) {
        self.density_job_id = self.density_job_id.wrapping_add(1);
        if let Some(cancelled) = self.density_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.density_task.take();
        self.density_progress = None;
        self.density_error = None;
        self.density_values = None;
        self.density_vertex_buffer = None;
        self.density_pipeline = None;
    }

    fn begin_density_view(&mut self, cx: &mut Context<Self>) {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::sync::Arc;

        self.density_progress = Some(0.0);
        self.density_error = None;
        let Some(path) = self.current_path.clone() else {
            self.density_progress = None;
            self.density_error = Some("No mesh file is open".into());
            return;
        };
        let job_id = self.density_job_id.wrapping_add(1);
        self.density_job_id = job_id;
        let progress = Arc::new(AtomicU32::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.density_cancel = Some(Arc::clone(&cancelled));
        let worker_progress = Arc::clone(&progress);
        let worker_cancelled = Arc::clone(&cancelled);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = super::panel_render::load_vertex_density(
                &path,
                &worker_progress,
                &worker_cancelled,
            );
            let _ = tx.send(result);
        });

        self.density_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(80))
                .await;
            let finished_result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Density calculation stopped unexpectedly".into()))
                }
            };
            let percent = progress.load(Ordering::Relaxed).min(1000) as f32 / 1000.0;
            let update = this.update(cx, |panel, cx| {
                if panel.density_job_id != job_id
                    || panel.render_mode != MeshRenderMode::VertexDensity
                {
                    return true;
                }
                if let Some(result) = finished_result {
                    panel.density_progress = None;
                    panel.density_cancel = None;
                    match result {
                        Ok(values) => {
                            panel.density_values = Some(values);
                            panel.rebuild_density_resources();
                        }
                        Err(error) if error != "cancelled" => {
                            panel.density_error = Some(error);
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                    true
                } else {
                    panel.density_progress = Some(percent);
                    cx.notify();
                    false
                }
            });
            match update {
                Ok(true) | Err(_) => break,
                Ok(false) => {}
            }
        }));
    }

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

        let is_3d = ext == "fbx" || ext == "mesh";

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

        let mut panel = Self {
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
            wire_index_buffer: None,
            wire_index_count: 0,
            wire_pipeline: None,
            depth_texture: None,
            depth_view: None,
            mesh_vertex_buffer: None,
            mesh_index_buffer: None,
            mesh_index_count: 0,
            mesh_vertex_count: 0,
            mesh_props: Vec::new(),
            scene_stats: SceneStats::default(),
            mesh_pipeline: None,
            density_pipeline: None,
            density_vertex_buffer: None,
            density_values: None,
            density_progress: None,
            density_error: None,
            density_job_id: 0,
            density_cancel: None,
            density_task: None,
            mesh_bind_group: None,
            uv_grid_texture: None,
            uv_grid_view: None,
            uv_grid_sampler: None,
            uv_grid_bind_group: None,
            uv_grid_bgl: None,
            mesh_uniform_buffer: None,
            render_mode: MeshRenderMode::Lit,
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
            mesh_materials: None,
            mesh_sections: Vec::new(),
            slot_surfaces: Vec::new(),
            slot_colors: Vec::new(),
            graph_draws: Vec::new(),
            mesh_bgl: None,
            empty_bind_group: None,
            globals_buffer: None,
            globals_layout: None,
            globals_bind_group: None,
            frame_counter: 0,
        };
        panel.init_mesh_materials(window, cx);
        panel
    }
}
