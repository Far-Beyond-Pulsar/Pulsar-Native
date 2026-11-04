//! Ultra-Water integrated with Helio's voxel terrain, meadow foliage, sky and haze.
//!
//! Run from the Helio workspace root:
//! `cargo run -p helio-voxel-examples --release --bin water_world -- coast`
//! A deterministic one-shot capture (after voxel streaming warms up) can be
//! rendered and closed with:
//! `cargo run -p helio-voxel-examples --bin water_world -- shoreline --capture-and-exit`
//!
//! Presets: `coast` (wide island-and-ocean view), `shoreline` (low camera at
//! the breakers), and `underwater` (looking through the
//! surface toward the island). Keys 1/2/3 switch views, Space triggers a
//! shoreline splash, and C saves a PNG to `target/water-world/`. Click to
//! capture the mouse, WASD flies, Q/E descend/ascend, and Escape releases the
//! pointer (Escape again closes). The terrain, foliage, clouds and shoreline
//! are seeded so the views stay comparable.
//! Optional CLI: pass `--capture` to save the startup preset after the local
//! voxel cut and foliage tiles settle (up to 900 frames);
//! add `--capture-and-exit` to close after that capture. Without either flag,
//! this runs as a live window and never writes a file automatically.

#[path = "voxel_flight/worlds.rs"]
#[allow(dead_code)]
mod worlds;

use glam::{DVec3, Vec3};
use helio::{Camera, Renderer, RendererBuilder, RendererConfig};
use helio_components::WorldRows;
use helio_light_host::LightComponent;
use helio_pass_foliage_place::host_rows::{FoliageTypeComponent, FoliageWindComponent};
use helio_pass_foliage_place::FoliagePlacePass;
use helio_pass_foliage_place::{
    pack_kind_and_flags, FoliageKind, FoliageMaterial, GpuFoliageType, FOLIAGE_FLAG_GROUND_COLOUR,
    FOLIAGE_FLAG_TWO_SIDED,
};
use helio_pass_post_volumes::host_rows::CameraPostProcessComponent;
use helio_pass_voxel_planet::chunks::pass::ChunkPass;
use helio_pass_voxel_planet::engine::{PlanetFrame, TerrainAppearance, VOXEL_FRAME};
use helio_pass_voxel_planet::{Brush, BrushOp, BrushShape, Planet, PlanetRecipe, VolumeTransform};
use helio_pass_water_sim::host_rows::WaterVolumeComponent;
use helio_pass_water_sim::{GpuWaterVolume, OceanSpectrum, WaterSimPass, WaterSpraySettings};
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{collections::HashSet, time::Instant};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, KeyEvent, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

const SEED: u64 = 34_120_261;
const REST_Y: f64 = 0.0;
const EXTENT: f32 = 1536.0;

struct Showcase {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    renderer: Renderer,
    planet: Arc<Planet>,
    /// The terrain's look, published with the planet each frame.
    appearance: TerrainAppearance,
    scene: pulsar_scenedb::SceneDb,
    sun_entity: pulsar_scenedb::Entity,
    /// The water volume and its row: its `sun_direction` is the scene's sun, which
    /// the engine's environment join writes and this demo must keep current.
    water_entity: pulsar_scenedb::Entity,
    water_row: GpuWaterVolume,
    sun_preset: String,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    size: [u32; 2],
    present_pipeline: wgpu::RenderPipeline,
    present_layout: wgpu::BindGroupLayout,
    present_sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
}

fn terrain(seed: u64) -> Planet {
    let mut planet = Planet::new(PlanetRecipe {
        shape: helio_pass_voxel_planet::grid::Shape::Plane,
        // The terrain square matches the shoreline-map domain. It extends far
        // enough beyond the authored island that the camera sees the beach and
        // seabed rather than the plane's edge, while chunk residency only
        // refines the camera cut.
        plane_size_m: f64::from(EXTENT) * 2.0,
        voxel_size_m: 0.5,
        terrain: {
            let mut seabed =
                helio_pass_voxel_planet::layers::TerrainLayers::flat_at(-42.0).heightfield();
            seabed.surface = "Sand".into();
            seabed.source(seed)
        },
        // Meadow dressing is rendered by the foliage pass below. Voxel-stamped
        // feature trees are intentionally omitted: populating the full plane
        // with them exhausts leaf-page residency before the nearby island cut
        // can finish refining.
        features: Default::default(),
        ..Default::default()
    })
    .expect("create deterministic island terrain");

    // Ultra's source profile has a roughly 360 m sea-level coastline and
    // 67 m summit. A single broad cap keeps that island scale without the
    // multi-kilometre tangent spheres that exhausted voxel leaf residency;
    // the surrounding authored heightfield remains a -42 m seabed. The shore
    // map samples this same edited voxel provider.
    let summit = island_height(0.0);
    let cap_radius = 1_010.0_f64;
    planet
        .apply(Brush {
            center: [0.0, summit - cap_radius, 0.0],
            radius: cap_radius,
            shape: BrushShape::Sphere,
            op: BrushOp::Add,
            material: helio_pass_voxel_planet::terrain::material::GRASS,
            height: 0.0,
        })
        .expect("sculpt broad seeded island cap");
    planet
}

fn island_height(radius: f64) -> f64 {
    let t = ((radius - 380.0) / 720.0).clamp(0.0, 1.0);
    let smoothstep = t * t * (3.0 - 2.0 * t);
    62.0 * (-((radius / 190.0).powi(2))).exp() + 15.0 * (-((radius / 430.0).powi(2))).exp()
        - 10.0
        - 32.0 * smoothstep
}

impl Showcase {
    fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        size: [u32; 2],
    ) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let planet = Arc::new(terrain(SEED));
        let context = EngineGpuContext::new(device.clone(), queue.clone());
        let mut store = SceneGpuStore::new(
            &context,
            SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        );
        helio_pass_sky::host_rows::AtmosphereComponent::register_gpu_columns_growable(
            &mut store, 4, &device,
        );
        helio_mesh_host::MeshComponent::register_gpu_columns_growable(&mut store, 16, &device);
        helio_components::MaterialComponent::register_gpu_columns_growable(&mut store, 16, &device);
        helio_mesh_host::StaticObjectComponent::register_gpu_columns_growable(
            &mut store, 16, &device,
        );
        helio_light_host::LightComponent::register_gpu_columns_growable(&mut store, 16, &device);
        helio_pass_volumetric_fog::host_rows::GlobalFogComponent::register_gpu_columns_growable(
            &mut store, 4, &device,
        );
        helio_pass_water_sim::host_rows::WaterVolumeComponent::register_gpu_columns_growable(
            &mut store, 8, &device,
        );
        FoliageTypeComponent::register_gpu_columns_growable(&mut store, 8, &device);
        FoliageWindComponent::register_gpu_columns_growable(&mut store, 4, &device);
        let mirror = GpuMirrorHandle::new(Arc::new(store), queue.clone());
        let mut scene = pulsar_scenedb::SceneDb::new();
        scene.world.attach_gpu_mirror(mirror.clone());

        // `water_volumes` rows are indexed by entity slot and the water pass
        // reads only the first `MAX_SIM_VOLUMES` rows, so the volume's entity
        // is spawned first; spawned after the meadow's types it lands past
        // them and the sea renders with no row (no absorption, no colour).
        let water_entity = scene.world.spawn();

        // A near-ground atmosphere centred so the plane's y=0 surface lies at
        // its ground radius; Earth's scattering and the same authored haze
        // recipe used by voxel_flight provide the air and horizon depth.
        // Earth's real radius: the air keeps its 100 km thickness around any
        // planet, so a smaller one shortens the sun's path at a low sun to a
        // fraction of the real column and the sunset never reddens.
        let world_radius = 6_360_000.0;
        let atmosphere = scene.world.spawn();
        scene.world.insert_row(
            atmosphere,
            worlds::Preset::Earth
                .sky(world_radius)
                .expect("Earth showcase atmosphere")
                .around_planet([0.0, -world_radius as f32, 0.0], world_radius),
        );
        // `WATER_WORLD_NO_HAZE=1` drops the global haze (diagnostics): it is
        // composited after the water, so it also veils the underwater far field.
        if std::env::var_os("WATER_WORLD_NO_HAZE").is_none() {
            let haze = scene.world.spawn();
            scene.world.insert_row(
                haze,
                worlds::Preset::Earth
                    .haze(world_radius)
                    .expect("Earth showcase haze")
                    .around_planet([0.0, -world_radius as f32, 0.0], world_radius),
            );
        }

        // Match the reference scene's directional sun. The same light drives
        // the terrain and Earth's atmospheric scattering; its angle changes
        // with the selected water showcase preset below.
        let sun_entity = scene.world.spawn();
        scene.world.insert(
            sun_entity,
            LightComponent::from(helio_light_api::LightRow::from(directional_sun(
                preset_sun("coast").0,
                preset_sun("coast").1,
            ))),
        );

        let meadow = worlds::Preset::Earth
            .meadow()
            .expect("Earth meadow recipe")
            .with_tall_stands()
            .with_wind([1.0, 0.35], 4.5, 0.8);
        for row in &meadow.types {
            let entity = scene.world.spawn();
            scene.world.insert_row(entity, *row);
        }
        // The authored voxel meadow is intentionally low and blocky. Add a
        // small band of native blade tufts so the close shoreline preset can
        // resolve individual wind motion at the camera's scale.
        let mut shoreline_blades = GpuFoliageType {
            density: 12.0,
            height_range: [1.2, 2.2],
            width_range: [0.035, 0.07],
            slope_range: [-1.0, 1.0],
            altitude_range: [f32::MIN, f32::MAX],
            // The source Breakers camera frames the meadow from tens to a few
            // hundred metres away. Hold the high-detail blade LOD long enough
            // for those individual tufts to read in the shot.
            lod_distances: [100.0, 220.0, 450.0, 900.0],
            wind_response: [0.7, 0.4, 0.8],
            interaction_stiffness: 1.0,
            ground_mask: 1 << helio_pass_voxel_planet::terrain::material::GRASS,
            density_layer: 0,
            kind_and_flags: pack_kind_and_flags(
                FoliageKind::Blade,
                FOLIAGE_FLAG_TWO_SIDED | FOLIAGE_FLAG_GROUND_COLOUR,
            ),
            lean_range: [0.08, 0.45],
            curve: 0.35,
            fall_chance: 0.08,
            ..GpuFoliageType::default()
        };
        shoreline_blades.set_blades_per_tuft([6, 10]);
        shoreline_blades.set_material(Some(FoliageMaterial {
            base_color: [0.24, 0.42, 0.12],
            roughness: 0.86,
            metallic: 0.0,
        }));
        let blade_entity = scene.world.spawn();
        scene.world.insert(
            blade_entity,
            FoliageTypeComponent::from(helio_pass_foliage_place::FoliageTypeRow::from(
                shoreline_blades,
            )),
        );
        let wind = scene.world.spawn();
        scene.world.insert_row(wind, meadow.wind);

        let mut water = GpuWaterVolume::default_ocean();
        water.bounds_min = [-EXTENT, -500.0, -EXTENT, 0.0];
        water.bounds_max = [EXTENT, 300.0, EXTENT, REST_Y as f32];
        water.wave_params = [0.55, 30.0, 1.0, 1.0];
        // Ultra-Water OceanSurface defaults. Helio's row maps medium scatter
        // to water_color.rgb, absorption to extinction.rgb, foam bias/scale
        // to water_color.w/extinction.w, refraction to reflection_refraction.y,
        // and foam brightness to reflection_refraction.w. wave_params carries
        // the nearest amplitude / wavelength / speed / steepness values for
        // uShoreWave; its contact film uses the JS .35 m band directly.
        water.water_color = [0.0, 0.05, 0.08, 0.05];
        water.extinction = [0.42, 0.075, 0.048, 1.3];
        water.reflection_refraction[1] = 0.06;
        water.reflection_refraction[3] = 0.85;
        water.fog_params[0] = 1.0 / 18_000.0;
        // No direct WaterVolume equivalents exist for uSSS, uOptics roughness
        // or underwater-window distortion, or the foam tint. Keep Helio's
        // independent caustics, refraction and Fresnel defaults.
        water.wind_params = [0.939_692_6, 0.342_020_15, 9.0, 1.0];
        // The row's sun is the scene's sun (`default_ocean`'s is a fixed noon
        // sun the sky never agrees with): caustics, shafts and the surface
        // light the water from it.
        let (coast_azimuth, coast_elevation) = preset_sun("coast");
        let coast_sun = sun_direction(coast_azimuth, coast_elevation);
        water.sun_direction = [coast_sun.x, coast_sun.y, coast_sun.z, 1.0];
        scene.world.insert(
            water_entity,
            WaterVolumeComponent::from(helio_pass_water_sim::WaterVolumeRow::from(water)),
        );

        let appearance = worlds::Preset::Earth.appearance(planet.field().appearance());
        let terrain = helio_pass_voxel_planet::VoxelSettings::default();
        let mut renderer_config = RendererConfig::new(size[0], size[1], format);
        renderer_config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .enabled = true;
        renderer_config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .tufts_per_m2 = Some(meadow.tufts_per_m2 * 1.4);
        renderer_config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .blades_per_tuft = Some(meadow.blades_per_tuft);
        renderer_config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .range = Some(meadow.range * 1.5);
        let renderer_config = renderer_config.with_feature(terrain);
        let mut renderer = RendererBuilder::new(renderer_config.clone(), mirror)
            .with_external_device()
            // The voxel world, the leaf kinds (particles, sprites, decals,
            // the lens and its flare, portals) and the media kinds (foliage,
            // water, fog, post-process volumes) and the lighting kinds (GI,
            // the sky) are features Helio's presets do not list: their plugins', which a host adds as the engine does.
            .with_graph(helio_mesh_host::presets::with(
                helio_presets::Preset::Deferred,
                &helio_feature_api::FeatureSet::new()
                    .with(helio_pass_voxel_planet::VoxelFeature)
                    .with(helio_pass_corona::CoronaFeature)
                    .with(helio_pass_sprite_batch::SpriteFeature)
                    .with(helio_pass_decal::DecalFeature)
                    .with(helio_pass_lens::LensFeature)
                    .with(helio_pass_flare::LensFlareFeature)
                    .with(helio_pass_portal_cull::PortalCullFeature)
                    .with(helio_pass_portal_instances::PortalFeature)
                    .with(helio_pass_foliage_gbuffer::FoliageFeature)
                    .with(helio_pass_water_sim::WaterFeature)
                    .with(helio_pass_volumetric_fog::FogFeature)
                    .with(helio_pass_post_volumes::PostVolumesFeature)
                    .with(helio_pass_sky::SkyFeature)
                    .with(helio_pass_gi::GiFeature),
            ))
            .build(
                device.clone(),
                queue.clone(),
                size[0],
                size[1],
                renderer_config.surface_format,
            );
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.45, 0.58, 0.8],
            intensity: 0.35,
        });
        let mut post = helio_pass_postprocess::PostProcessSettings::default();
        post.tonemap_operator = helio::TonemapOperator::AgxPunchy;
        post.tonemap_exposure = 2.0;
        post.color_contrast = [1.06; 3];
        let camera = scene.world.spawn();
        scene.world.insert(
            camera,
            CameraPostProcessComponent::from(helio_pass_post_volumes::CameraPostProcessRow::new(
                0, &post,
            )),
        );
        scene.world.flush_gpu_mirror(&queue);
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            sim.set_shore_map(Some(
                helio_pass_water_sim::WaterShoreMap::from_terrain(
                    [0.0, 0.0],
                    [EXTENT * 2.0, EXTENT * 2.0],
                    REST_Y,
                    &helio_pass_voxel_planet::VoxelShoreTerrain {
                        planet: &planet,
                        transform: VolumeTransform::default(),
                        rest_y: REST_Y,
                    },
                )
                .expect("bake shoreline from the edited voxel terrain"),
            ));
            sim.set_ocean_rest_height(REST_Y)
                .expect("match the ocean volume rest plane");
            // Match `defaultOceanParams()` and the JS Spray defaults exactly.
            sim.set_wind([0.939_692_6, 0.342_020_15], 9.0);
            sim.set_ocean_spectrum(OceanSpectrum::default());
            sim.set_spray_settings(WaterSpraySettings::default());
        } else {
            panic!("the default render graph did not build WaterSimPass");
        }

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water world showcase output"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water world present layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let present_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water world present sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water world present shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@builtin(vertex_index) index: u32) -> Out {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: Out;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}
@fragment fn fs(input: Out) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, input.uv);
}
"#
                .into(),
            ),
        });
        let present_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("water world present pipeline layout"),
                bind_group_layouts: &[Some(&present_layout)],
                immediate_size: 0,
            });
        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water world present pipeline"),
            layout: Some(&present_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &present_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &present_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            device,
            queue,
            renderer,
            planet,
            appearance,
            scene,
            sun_entity,
            water_entity,
            water_row: water,
            sun_preset: "coast".to_owned(),
            target,
            target_view,
            size,
            present_pipeline,
            present_layout,
            present_sampler,
            format,
        }
    }

    fn draw(&mut self, eye: DVec3, forward: Vec3, preset: &str) {
        if self.sun_preset != preset {
            let (azimuth, elevation) = preset_sun(preset);
            // Edited in place: re-inserting a row drops and re-adds it, and the
            // render graph joins passes by live content, so the water pass
            // (and the sea with it) would restart for a few frames.
            *self
                .scene
                .world
                .get_mut::<helio_light_host::LightComponent>(self.sun_entity)
                .expect("sun light component") = LightComponent::from(
                helio_light_api::LightRow::from(directional_sun(azimuth, elevation)),
            );
            let sun = sun_direction(azimuth, elevation);
            self.water_row.sun_direction = [sun.x, sun.y, sun.z, 1.0];
            *self
                .scene
                .world
                .get_mut::<WaterVolumeComponent>(self.water_entity)
                .expect("water volume component") = WaterVolumeComponent::from(
                helio_pass_water_sim::WaterVolumeRow::from(self.water_row),
            );
            self.scene.world.flush_gpu_mirror(&self.queue);
            self.sun_preset = preset.to_owned();
        }
        let (azimuth, elevation) = preset_sun(preset);
        let sun = sun_direction(azimuth, elevation);
        let mut frame = PlanetFrame::single(self.planet.clone(), sun);
        frame.volumes[0].appearance = Some(self.appearance);
        self.renderer
            .set_scene_payload(&VOXEL_FRAME, Arc::new(frame));
        let forward = forward.normalize();
        let up = if forward.y.abs() > 0.999 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let aspect = self.size[0] as f32 / self.size[1] as f32;
        let camera = Camera::perspective_look_to(
            eye,
            forward,
            up,
            48_f32.to_radians(),
            aspect,
            0.05,
            20_000.0,
        );
        self.renderer
            .render(&camera, &self.target_view)
            .expect("render water world showcase");
        let _ = self.device.poll(wgpu::PollType::Poll);
    }

    fn resize_target(&mut self, width: u32, height: u32) {
        let size = [width.max(1), height.max(1)];
        if self.size == size {
            return;
        }

        self.size = size;
        self.target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water world showcase output"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.target_view = self.target.create_view(&Default::default());
    }

    fn present(&self, view: &wgpu::TextureView) {
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water world present bind group"),
            layout: &self.present_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.target_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.present_sampler),
                },
            ],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("water world present"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.present_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        self.queue.submit([encoder.finish()]);
    }

    fn save(&self, output: &Path, preset: &str) {
        let row = (self.size[0] * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water world readback"),
            size: u64::from(row) * u64::from(self.size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(self.size[1]),
                },
            },
            self.target.size(),
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("wait for showcase capture");
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped showcase readback");
        let mut pixels: Vec<u8> = data
            .chunks(row as usize)
            .flat_map(|line| line[..self.size[0] as usize * 4].to_vec())
            .collect();
        if matches!(
            self.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm
        ) {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        let path = output.join(format!("water_world_{preset}.png"));
        image::save_buffer(
            &path,
            &pixels,
            self.size[0],
            self.size[1],
            image::ColorType::Rgba8,
        )
        .expect("write showcase PNG");
        eprintln!("WATER_WORLD_CAPTURE {}", path.display());
    }
}

struct App {
    state: Option<AppState>,
    initial_preset: String,
    output: PathBuf,
    capture_on_start: bool,
    exit_after_capture: bool,
}

struct AppState {
    window: Arc<Window>,
    graphics: helio::HelioGraphicsContext<'static>,
    showcase: Showcase,
    preset: String,
    capture_requested: bool,
    capture_countdown: Option<u32>,
    capture_stable_frames: u32,
    camera_eye: DVec3,
    camera_yaw: f32,
    camera_pitch: f32,
    move_keys: HashSet<KeyCode>,
    cursor_captured: bool,
    skip_mouse_delta: bool,
    last_frame: Instant,
}

impl AppState {
    fn resize(&mut self, width: u32, height: u32) {
        self.graphics
            .resize(&mut self.showcase.renderer, width, height);
        self.showcase.resize_target(width, height);
    }

    fn set_preset(&mut self, preset: &str) {
        self.preset = preset.to_owned();
        let (eye, forward) = preset_view(preset);
        let forward = forward.normalize();
        self.camera_eye = eye;
        self.camera_yaw = forward.z.atan2(forward.x);
        self.camera_pitch = forward.y.asin();
        self.window.set_title(&format!(
            "Helio Water World — {}  |  1/2/3 Views · Click Capture · WASD Fly · Q/E Vertical · Space Splash · C Capture",
            preset
        ));
    }

    fn draw(&mut self, output: &Path) -> bool {
        self.update_camera();
        self.showcase
            .draw(self.camera_eye, self.camera_forward(), &self.preset);
        let mut capture_now = self.capture_requested;
        if let Some(frames_left) = self.capture_countdown {
            let terrain_ready =
                self.showcase
                    .renderer
                    .find_pass::<ChunkPass>()
                    .is_some_and(|pass| {
                        pass.stats().requests == 0 && pass.stats().coarse_share <= 0.02
                    });
            let foliage_ready = self
                .showcase
                .renderer
                .find_pass::<FoliagePlacePass>()
                .is_some_and(|pass| pass.queued_tile_count() == 0 && pass.commands_recorded() > 0);
            if terrain_ready && foliage_ready {
                self.capture_stable_frames += 1;
            } else {
                self.capture_stable_frames = 0;
            }
            if self.capture_stable_frames >= 30 || frames_left <= 1 {
                self.capture_countdown = None;
                capture_now = true;
            } else {
                self.capture_countdown = Some(frames_left - 1);
            }
        }
        let frame = match self.graphics.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.graphics
                    .surface
                    .configure(&self.graphics.device, &self.graphics.surface_config);
                self.capture_requested |= capture_now;
                return false;
            }
            _ => {
                self.capture_requested |= capture_now;
                return false;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        self.showcase.present(&view);
        self.showcase.queue.present(frame);
        if capture_now {
            std::fs::create_dir_all(output).expect("create capture output directory");
            if let Some(foliage) = self.showcase.renderer.find_pass::<FoliagePlacePass>() {
                eprintln!(
                    "WATER_WORLD_FOLIAGE placed_tiles={} queued_tiles={} commands={} counters={:?}",
                    foliage.placed_tile_count(),
                    foliage.queued_tile_count(),
                    foliage.commands_recorded(),
                    foliage.debug_counters(),
                );
            }
            if let Some(chunks) = self.showcase.renderer.find_pass::<ChunkPass>() {
                eprintln!("WATER_WORLD_CHUNKS {}", chunks.stats_line());
            }
            self.showcase.save(output, &self.preset);
            self.capture_requested = false;
            return true;
        }
        false
    }

    fn camera_forward(&self) -> Vec3 {
        let (yaw, pitch) = (self.camera_yaw, self.camera_pitch);
        Vec3::new(
            yaw.cos() * pitch.cos(),
            pitch.sin(),
            yaw.sin() * pitch.cos(),
        )
    }

    fn update_camera(&mut self) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .clamp(0.0, 0.1);
        self.last_frame = now;
        let forward = self.camera_forward();
        let right = Vec3::Y.cross(forward).normalize_or_zero();
        let mut movement = Vec3::ZERO;
        if self.move_keys.contains(&KeyCode::KeyW) {
            movement += forward;
        }
        if self.move_keys.contains(&KeyCode::KeyS) {
            movement -= forward;
        }
        if self.move_keys.contains(&KeyCode::KeyD) {
            movement += right;
        }
        if self.move_keys.contains(&KeyCode::KeyA) {
            movement -= right;
        }
        if self.move_keys.contains(&KeyCode::KeyE) {
            movement.y += 1.0;
        }
        if self.move_keys.contains(&KeyCode::KeyQ) {
            movement.y -= 1.0;
        }
        if movement.length_squared() > 0.0 {
            let speed = if self.move_keys.contains(&KeyCode::ShiftLeft) {
                360.0
            } else {
                90.0
            };
            self.camera_eye += (movement.normalize() * speed * dt).as_dvec3();
        }
    }
}

fn preset_view(preset: &str) -> (DVec3, Vec3) {
    match preset {
        // Lagoon view: offshore and low (5 m, Ultra-Water's Golden hour camera) so the sea is seen at grazing angles; the island
        // silhouette, with a level sightline that leaves animated sea in front.
        "coast" => (
            DVec3::new(-1_060.7, 5.0, -1_060.7),
            Vec3::new(0.707, -0.03, 0.707),
        ),
        // Breakers source framing: place the camera 35 m offshore and 40 m
        // along the coast, then look 30 m back along shore. This keeps open
        // water, the beach line and the sloped meadow in one frame instead of
        // aiming through the island centre.
        "shoreline" => (
            DVec3::new(-229.4, 3.2, -286.0),
            Vec3::new(-0.317, -0.041, 0.948),
        ),
        // Ultra-Water's Underwater view, 5 m down over ~8 m of water (a wave trough
        // can reach 3 m, so 3.2 m would sometimes surface), looking along the sun's
        // bearing and slightly up into it (shafts).
        "underwater" => (
            DVec3::new(-353.6, -5.0, -353.6),
            Vec3::new(-0.42, 0.056, -0.91),
        ),
        // Underwater with the sun behind the camera, on the shore's slope (the
        // terrain is a -42 m floor with a spherical cap whose coast is at ~360 m):
        // 4 m down at ~385 m, looking up the slope at its seabed and caustics.
        "reef" => (
            DVec3::new(-272.2, -4.0, -272.2),
            Vec3::new(0.70, -0.12, 0.70),
        ),
        other => panic!("unknown preset {other}; use coast, shoreline, underwater or reef"),
    }
}

/// (azimuth, elevation) of the sun per preset. `coast` is Ultra-Water's golden
/// hour (3.5 degrees) placed ahead of the camera so the sun's path across the
/// swell is in frame; the other views keep a higher sun to light the reef.
fn preset_sun(preset: &str) -> (f32, f32) {
    match preset {
        "coast" => (45.0, 3.5),
        // Ultra-Water's Underwater view: a 48 degree sun, looked into from below.
        "underwater" | "reef" => (205.0, 48.0),
        _ => (205.0, 22.0),
    }
}

fn sun_direction(azimuth_degrees: f32, elevation_degrees: f32) -> Vec3 {
    let azimuth = azimuth_degrees.to_radians();
    let elevation = elevation_degrees.to_radians();
    Vec3::new(
        azimuth.sin() * elevation.cos(),
        elevation.sin(),
        azimuth.cos() * elevation.cos(),
    )
    .normalize()
}

fn directional_sun(azimuth_degrees: f32, elevation_degrees: f32) -> helio_light_api::GpuLight {
    let direction = sun_direction(azimuth_degrees, elevation_degrees);
    helio_light_api::GpuLight {
        position_range: [0.0, 0.0, 0.0, f32::MAX],
        direction_outer: [-direction.x, -direction.y, -direction.z, 0.0],
        color_intensity: [1.0, 1.0, 1.0, 3.0],
        shadow_index: u32::MAX,
        light_type: helio_light_api::LightType::Directional as u32,
        god_rays_enabled: 1,
        god_rays_density: 1.0,
        god_rays_weight: 1.0,
        god_rays_exposure: 1.0,
        god_rays_decay: 1.0,
        ..Default::default()
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio Water World")
                        .with_inner_size(winit::dpi::LogicalSize::new(1600u32, 900u32)),
                )
                .expect("create water-world window"),
        );
        let size = window.inner_size();
        let graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                application_name: "Water World Showcase".into(),
                ..Default::default()
            },
        ))
        .expect("create Helio graphics context");
        let surface_size = [
            graphics.surface_config.width,
            graphics.surface_config.height,
        ];
        let showcase = Showcase::new(
            graphics.device.clone(),
            graphics.queue.clone(),
            graphics.surface_config.format,
            surface_size,
        );
        let mut state = AppState {
            window,
            graphics,
            showcase,
            preset: "coast".to_owned(),
            capture_requested: false,
            capture_countdown: self.capture_on_start.then_some(900),
            capture_stable_frames: 0,
            camera_eye: DVec3::ZERO,
            camera_yaw: 0.0,
            camera_pitch: 0.0,
            move_keys: HashSet::new(),
            cursor_captured: false,
            skip_mouse_delta: false,
            last_frame: Instant::now(),
        };
        state.set_preset(&self.initial_preset);
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !state.cursor_captured => {
                if state
                    .window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .is_err()
                {
                    let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
                }
                state.window.set_cursor_visible(false);
                state.cursor_captured = true;
                state.skip_mouse_delta = true;
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                state.resize(size.width, size.height);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: key_state,
                        physical_key: PhysicalKey::Code(key),
                        repeat,
                        ..
                    },
                ..
            } => {
                if key_state == ElementState::Pressed {
                    state.move_keys.insert(key);
                    if !repeat {
                        match key {
                            KeyCode::Escape if state.cursor_captured => {
                                let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                                state.window.set_cursor_visible(true);
                                state.cursor_captured = false;
                                state.move_keys.clear();
                            }
                            KeyCode::Escape => event_loop.exit(),
                            KeyCode::Digit1 => state.set_preset("coast"),
                            KeyCode::Digit2 => state.set_preset("shoreline"),
                            KeyCode::Digit3 => state.set_preset("underwater"),
                            KeyCode::KeyC => state.capture_requested = true,
                            KeyCode::Space => {
                                if let Some(sim) =
                                    state.showcase.renderer.find_pass_mut::<WaterSimPass>()
                                {
                                    let _ = sim.add_spray_impact(
                                        DVec3::new(-350.0, REST_Y, -500.0),
                                        0.9,
                                        1.7,
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    state.move_keys.remove(&key);
                }
            }
            WindowEvent::RedrawRequested => {
                let captured = state.draw(&self.output);
                if captured && self.exit_after_capture {
                    event_loop.exit();
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        let Some(state) = &mut self.state else { return };
        let DeviceEvent::MouseMotion { delta } = event else {
            return;
        };
        if !state.cursor_captured {
            return;
        }
        if state.skip_mouse_delta {
            state.skip_mouse_delta = false;
            return;
        }
        state.camera_yaw += delta.0 as f32 * 0.0025;
        state.camera_pitch = (state.camera_pitch - delta.1 as f32 * 0.0025).clamp(-1.52, 1.52);
    }
}

fn main() {
    let mut initial_preset = None;
    let mut output = None;
    let mut capture_on_start = false;
    let mut exit_after_capture = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--capture" => capture_on_start = true,
            "--capture-and-exit" => {
                capture_on_start = true;
                exit_after_capture = true;
            }
            _ if initial_preset.is_none() => initial_preset = Some(arg),
            _ if output.is_none() => output = Some(PathBuf::from(arg)),
            _ => panic!("unexpected argument {arg}"),
        }
    }
    let initial_preset = initial_preset.unwrap_or_else(|| "coast".to_owned());
    let output = output.unwrap_or_else(|| PathBuf::from("target/water-world"));
    preset_view(&initial_preset);
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop
        .run_app(&mut App {
            state: None,
            initial_preset,
            output,
            capture_on_start,
            exit_after_capture,
        })
        .expect("run water-world showcase");
}
