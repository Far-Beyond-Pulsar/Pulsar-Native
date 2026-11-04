use crate::common::{
    insert_object, make_material, plane_mesh, spawn_light, spawn_material, spawn_mesh,
};
use glam::Vec3;
use helio::{Camera, Renderer, TonemapOperator};
use helio_wasm::{HelioWasmApp, InputState, KeyCode};
use pulsar_scenedb::{Entity, SceneDb};
use std::sync::Arc;

const LOOK_SENS: f32 = 0.0024;
const FLY_SPEED: f32 = 5.0;

pub struct Demo {
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    light_ids: [Entity; 3],
    ies_enabled: [bool; 3],
    gobo_enabled: bool,
}

impl HelioWasmApp for Demo {
    meshes_app!();

    fn title() -> &'static str {
        "Helio — IES Light Profiles"
    }

    fn render_scale() -> f32 {
        1.0
    }

    fn init(
        renderer: &mut Renderer,
        scene_db: &mut SceneDb,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        _w: u32,
        _h: u32,
    ) -> Self {
        let floor_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.15, 0.15, 0.16, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 6.0));
        let _ = insert_object(
            &mut scene_db.world,
            ground,
            floor_mat,
            glam::Mat4::IDENTITY,
            6.0,
        );

        let light_ids: [Entity; 3] = std::array::from_fn(|i| {
            let mut light = helio::GpuLight::default();
            light.light_type = helio::LightType::Spot as u32;
            light.color_intensity = match i {
                0 => [1.0, 0.3, 0.2, 8.0],
                1 => [0.2, 1.0, 0.3, 8.0],
                _ => [0.3, 0.4, 1.0, 8.0],
            };
            light.direction_outer = match i {
                0 => [0.0, -1.0, 0.0, 0.85],
                1 => [0.0, -1.0, 0.0, 0.75],
                _ => [0.0, -1.0, 0.0, 0.50],
            };
            light.position_range = match i {
                0 => [-1.5, 3.0, -1.0, 8.0],
                1 => [1.5, 3.0, -1.0, 8.0],
                _ => [0.0, 3.0, 2.0, 8.0],
            };
            light.inner_angle = match i {
                0 => 0.98,
                1 => 0.92,
                _ => 0.80,
            };
            spawn_light(&mut scene_db.world, light)
        });

        // Upload a 2-layer IES texture array
        const IES_W: u32 = 64;
        const IES_H: u32 = 64;
        let mut ies_pixels = Vec::with_capacity((IES_W * IES_H * 2) as usize);
        for y in 0..IES_H {
            for x in 0..IES_W {
                let u = (x as f32 + 0.5) / IES_W as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / IES_H as f32 * 2.0 - 1.0;
                let dist = (u * u + v * v).sqrt();
                let val = (-dist * dist * 6.0).exp();
                ies_pixels.push((val.clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
        for y in 0..IES_H {
            for x in 0..IES_W {
                let tile = ((x / 6) + (y / 6)) & 1;
                ies_pixels.push(if tile == 0 { 235u8 } else { 30u8 });
            }
        }
        let ies_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Demo IES Textures"),
            size: wgpu::Extent3d {
                width: IES_W,
                height: IES_H,
                depth_or_array_layers: 2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &ies_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &ies_pixels[..(IES_W * IES_H) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IES_W),
                rows_per_image: Some(IES_H),
            },
            wgpu::Extent3d {
                width: IES_W,
                height: IES_H,
                depth_or_array_layers: 1,
            },
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &ies_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 1 },
                aspect: wgpu::TextureAspect::All,
            },
            &ies_pixels[(IES_W * IES_H) as usize..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IES_W),
                rows_per_image: Some(IES_H),
            },
            wgpu::Extent3d {
                width: IES_W,
                height: IES_H,
                depth_or_array_layers: 1,
            },
        );
        let ies_view = ies_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        renderer.set_ies_texture_view(ies_view);

        Self {
            cam_pos: Vec3::new(0.0, 2.0, 5.0),
            cam_yaw: 0.0,
            cam_pitch: -0.2,
            light_ids,
            ies_enabled: [false, false, false],
            gobo_enabled: false,
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

        // Toggle IES on keypress
        if input.keys.contains(&KeyCode::Digit1) {
            self.ies_enabled[0] = !self.ies_enabled[0];
        }
        if input.keys.contains(&KeyCode::Digit2) {
            self.ies_enabled[1] = !self.ies_enabled[1];
        }
        if input.keys.contains(&KeyCode::Digit3) {
            self.ies_enabled[2] = !self.ies_enabled[2];
        }
        if input.keys.contains(&KeyCode::KeyG) {
            self.gobo_enabled = !self.gobo_enabled;
        }

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + fwd,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            input.aspect_ratio(),
            0.1,
            200.0,
        );
        camera
    }
}
