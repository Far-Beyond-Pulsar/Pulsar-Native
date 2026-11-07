use crate::common::{
    cube_mesh, insert_object, make_material, plane_mesh, point_light, spawn_light, spawn_material,
    spawn_mesh,
};
use glam::Vec3;
use helio::{Camera, Renderer, TonemapOperator};
use helio_pass_postprocess::LutBuilder;
use helio_wasm::{HelioWasmApp, InputState, KeyCode};
use pulsar_scenedb::SceneDb;
use std::sync::Arc;

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 5.0;

pub struct Demo {
    // This host's update callback has no World argument. Retain the authored
    // database here so interactive controls can update its persistent rows.
    scene_db: SceneDb,
    /// The settings the LUT is built from (this host's, as the tone map's
    /// baseline sees them).
    settings_buffer: wgpu::Buffer,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    grading_mode: u32,
    hue_shift: f32,
    lut_intensity: f32,
    lut_size: u32,
    lut_generation: u32,
    lut_builder: Option<LutBuilder>,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — Color Grading Demo"
    }

    fn render_scale() -> f32 {
        1.0
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let white = spawn_material(
            &mut scene_db.world,
            make_material([0.9, 0.9, 0.92, 1.0], 0.6, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let emissive_red = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.1, 0.1, 1.0], 0.3, 0.0, [10.0, 0.5, 0.5], 10.0),
        );
        let emissive_green = spawn_material(
            &mut scene_db.world,
            make_material([0.1, 1.0, 0.1, 1.0], 0.3, 0.0, [0.5, 10.0, 0.5], 10.0),
        );
        let emissive_blue = spawn_material(
            &mut scene_db.world,
            make_material([0.1, 0.1, 1.0, 1.0], 0.3, 0.0, [0.5, 0.5, 10.0], 10.0),
        );
        let metal = spawn_material(
            &mut scene_db.world,
            make_material([0.95, 0.93, 0.88, 1.0], 0.1, 1.0, [0.0, 0.0, 0.0], 0.0),
        );

        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 8.0));
        let _ = insert_object(
            &mut scene_db.world,
            ground,
            white,
            glam::Mat4::IDENTITY,
            8.0,
        );

        let red_cube = spawn_mesh(&mut scene_db.world, cube_mesh([-1.5, 0.5, -1.0], 0.5));
        let _ = insert_object(
            &mut scene_db.world,
            red_cube,
            emissive_red,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let green_cube = spawn_mesh(&mut scene_db.world, cube_mesh([1.5, 0.5, -1.0], 0.5));
        let _ = insert_object(
            &mut scene_db.world,
            green_cube,
            emissive_green,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let blue_cube = spawn_mesh(&mut scene_db.world, cube_mesh([0.0, 0.5, 1.5], 0.5));
        let _ = insert_object(
            &mut scene_db.world,
            blue_cube,
            emissive_blue,
            glam::Mat4::IDENTITY,
            0.5,
        );

        let metal_cube = spawn_mesh(&mut scene_db.world, cube_mesh([-1.5, 0.5, 2.5], 0.5));
        let _ = insert_object(
            &mut scene_db.world,
            metal_cube,
            metal,
            glam::Mat4::IDENTITY,
            0.5,
        );

        spawn_light(
            &mut scene_db.world,
            point_light([3.0, 4.0, -3.0], [1.0, 0.9, 0.7], 20.0, 15.0),
        );

        let lut_builder = LutBuilder::new(&device, 16);

        let settings_buffer = renderer.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("Color Grading Demo Settings"),
            size: std::mem::size_of::<helio_pass_postprocess::GpuPostProcessUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            settings_buffer,
            scene_db: std::mem::take(scene_db),
            cam_pos: Vec3::new(0.0, 3.0, 6.0),
            cam_yaw: 0.0,
            cam_pitch: -0.3,
            grading_mode: 0,
            hue_shift: 0.0,
            lut_intensity: 1.0,
            lut_size: 16,
            lut_generation: 1,
            lut_builder: Some(lut_builder),
        }
    }

    fn update(
        &mut self,
        renderer: &mut Renderer,
        dt: f32,
        _elapsed: f32,
        input: &InputState,
    ) -> Camera {
        self.cam_yaw += input.mouse_delta.0 * LOOK_SENS;
        self.cam_pitch = (self.cam_pitch - input.mouse_delta.1 * LOOK_SENS).clamp(-1.55, 1.55);
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

        if input.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += fwd * FLY_SPEED * dt;
        }
        if input.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= fwd * FLY_SPEED * dt;
        }
        if input.keys.contains(&KeyCode::KeyA) {
            self.cam_pos -= right * FLY_SPEED * dt;
        }
        if input.keys.contains(&KeyCode::KeyD) {
            self.cam_pos += right * FLY_SPEED * dt;
        }
        if input.keys.contains(&KeyCode::Space) {
            self.cam_pos.y += FLY_SPEED * dt;
        }
        if input.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos.y -= FLY_SPEED * dt;
        }

        // Mode / parameter changes
        if input.keys.contains(&KeyCode::Digit1) {
            self.grading_mode = 0;
        }
        if input.keys.contains(&KeyCode::Digit2) {
            self.grading_mode = 1;
        }
        if input.keys.contains(&KeyCode::Digit3) {
            self.grading_mode = 2;
            self.lut_generation += 1;
        }
        if input.keys.contains(&KeyCode::KeyQ) {
            self.hue_shift = (self.hue_shift - 5.0).clamp(-180.0, 180.0);
        }
        if input.keys.contains(&KeyCode::KeyE) {
            self.hue_shift = (self.hue_shift + 5.0).clamp(-180.0, 180.0);
        }
        if input.keys.contains(&KeyCode::KeyR) {
            self.lut_intensity = (self.lut_intensity - 0.1).clamp(0.0, 1.0);
        }
        if input.keys.contains(&KeyCode::KeyF) {
            self.lut_intensity = (self.lut_intensity + 0.1).clamp(0.0, 1.0);
        }

        let mut settings = helio_pass_postprocess::PostProcessSettings::default();
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.1,
            200.0,
        );
        settings.tonemap_operator = TonemapOperator::Aces;

        match self.grading_mode {
            0 => {
                settings.lut_platform = 0;
                settings.color_contrast = [1.2, 1.2, 1.2];
                settings.color_saturation = [1.3, 1.3, 1.3];
            }
            1 => {
                settings.lut_platform = 0;
                settings.lift_color = [0.02, 0.01, 0.04];
                settings.gamma_color = [0.95, 1.0, 1.05];
                settings.gain_color = [1.1, 1.0, 0.95];
                settings.hue_shift = self.hue_shift;
            }
            _ => {
                settings.lut_platform = 1;
                settings.lut_intensity = self.lut_intensity;
                settings.hue_shift = self.hue_shift;
            }
        }

        renderer.queue().write_buffer(
            &self.settings_buffer,
            0,
            bytemuck::bytes_of(&settings.to_gpu()),
        );
        let resolved = self.settings_buffer.clone();
        if let Some(ref mut builder) = self.lut_builder {
            let rebuilt = builder.build_if_needed(
                renderer.device(),
                renderer.queue(),
                &resolved,
                self.lut_generation,
            );
            // One texture, rewritten in place: given to the tone map once.
            let _ = rebuilt;
            if renderer
                .feature::<helio_pass_postprocess::ColorGradingLut>()
                .is_none()
            {
                let view = builder.lut_view().clone();
                renderer.set_feature(helio_pass_postprocess::ColorGradingLut(view));
            }
        }

        // The host's post settings: the tone map's baseline.
        renderer.set_scene_payload(&helio_pass_postprocess::POST_BASELINE, Arc::new(settings));
        self.scene_db.world.flush_gpu_mirror(renderer.queue());
        camera
    }
}
