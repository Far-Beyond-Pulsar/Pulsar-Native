use crate::common::{
    cube_mesh, directional_light, insert_object, make_material, plane_mesh, point_light,
    spawn_light, spawn_material, spawn_mesh,
};
use glam::Vec3;
use helio::{Camera, Renderer};
use helio_wasm::{HelioWasmApp, InputState, KeyCode};
use pulsar_scenedb::SceneDb;
use std::sync::Arc;

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 5.0;

pub struct Demo {
    // This host's update callback has no World argument. Retain the authored
    // database here so interactive controls can update its persistent rows.
    scene_db: SceneDb,
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    hdr_mode: u32,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — HDR Display Output"
    }

    fn render_scale() -> f32 {
        1.0
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        _device: Arc<wgpu::Device>,
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
        let emissive_sun = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.9, 0.7, 1.0], 0.2, 0.0, [50.0, 45.0, 35.0], 50.0),
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

        let sun_sphere = spawn_mesh(&mut scene_db.world, cube_mesh([3.0, 4.0, -3.0], 0.4));
        let _ = insert_object(
            &mut scene_db.world,
            sun_sphere,
            emissive_sun,
            glam::Mat4::IDENTITY,
            0.4,
        );

        spawn_light(
            &mut scene_db.world,
            directional_light([0.3, -0.8, 0.5], [1.0, 0.95, 0.85], 15.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([3.0, 4.0, -3.0], [1.0, 0.9, 0.7], 20.0, 15.0),
        );

        Self {
            scene_db: std::mem::take(scene_db),
            cam_pos: Vec3::new(0.0, 3.0, 6.0),
            cam_yaw: 0.0,
            cam_pitch: -0.3,
            hdr_mode: 0,
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

        if input.keys.contains(&KeyCode::KeyH) {
            self.hdr_mode = (self.hdr_mode + 1) % 4;
            let name = match self.hdr_mode {
                0 => "LDR (sRGB)",
                1 => "HDR10 (PQ)",
                2 => "scRGB",
                _ => "Passthrough",
            };
            log::info!("[HDR Demo] Switched to {}", name);
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
        match self.hdr_mode {
            0 => {
                settings.hdr_output_mode = helio::HdrOutputMode::Ldr;
                settings.tonemap_operator = helio::TonemapOperator::Aces;
            }
            1 => {
                settings.hdr_output_mode = helio::HdrOutputMode::Hdr10;
                settings.tonemap_operator = helio::TonemapOperator::None;
            }
            2 => {
                settings.hdr_output_mode = helio::HdrOutputMode::ScRgb;
                settings.tonemap_operator = helio::TonemapOperator::None;
            }
            _ => {
                settings.hdr_output_mode = helio::HdrOutputMode::Passthrough;
                settings.tonemap_operator = helio::TonemapOperator::None;
            }
        }
        // The host's post settings: the tone map's baseline.
        renderer.set_scene_payload(&helio_pass_postprocess::POST_BASELINE, Arc::new(settings));
        self.scene_db.world.flush_gpu_mirror(renderer.queue());
        camera
    }
}
