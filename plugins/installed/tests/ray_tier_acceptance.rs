//! The ray-traced tier through the engine path: the editor's headless
//! `HelioRenderer`, the shipped deferred graph and every installed plugin,
//! on a device with hardware ray queries (the default tier since r1; the
//! other render suites strip them). A mesh scene lit by a sun with GI:
//!
//! - **Materials at hits**: three walls over a white ground, one drawn with
//!   a material graph sampling a red texture, one with a graph computing a
//!   green colour, one with a plain grey surface. GI's probe rays hit them
//!   and shade each hit by its material's program (the meshes' surface
//!   passes): the bounce on the ground before each wall takes its colour,
//!   where a flat row colour or a texture's mean would not be the graph's.
//! - **Cut-outs**: a card whose graph cuts out its right half by its
//!   texture's alpha, between the sun and the ground: the ground behind its
//!   leaves is darker than behind its holes.
//!
//! On an adapter without ray queries every test prints `skipped` and
//! returns: this suite covers the ray-traced tier only. The skinned mesh on
//! the same path is `skinned_frames`. Set `RAY_TIER_DUMP_DIR` to write the
//! frames as PNGs.

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use installed_plugins as _;
use installed_plugins::meshes_plugin::helio_mesh_api::PackedVertex;
use installed_plugins::meshes_plugin::meshes_component::components::{
    MeshAssetPath, MeshGeometryKey, StaticMeshComponent, StaticMeshMaterialSlot,
    StaticMeshMaterialSlots,
};
use installed_plugins::meshes_plugin::meshes_component::mesh_cache::{
    ImportedSurfaceMaterial, MeshSection,
};
use ui_level_editor::LevelEditorState;

use glam::Vec3;
use std::path::{Path, PathBuf};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The deferred lighting's "Indirect Lighting" view (`helio_pass_deferred_light`).
const INDIRECT_VIEW: u32 = 23;
const EYE: [f64; 3] = [0.0, 3.0, 7.0];
const PITCH: f32 = -0.3;

fn shipped_pipeline() -> String {
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    if engine_state::get_project_path().is_none() {
        engine_state::EngineContext::new().set_global();
        engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    }
    pipelines::migrate_pipeline_id(pipelines::LEGACY_DEFAULT_PIPELINE.to_string())
}

/// A device with hardware ray queries, as the engine asks for one; `None`
/// (the suite skipped) without them.
fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let features = helio::required_wgpu_features(helio::usable_adapter_features(&adapter));
    if !features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
        eprintln!(
            "skipped: {} has no hardware ray queries (this suite covers the ray-traced tier)",
            adapter.get_info().name
        );
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()
}

// ── Assets ──────────────────────────────────────────────────────────────────

/// Where this suite writes its textures and graphs.
fn assets() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ray_tier_assets");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_png(path: &Path, size: u32, texel: impl Fn(u32, u32) -> [u8; 4]) {
    let pixels: Vec<u8> = (0..size * size)
        .flat_map(|i| texel(i % size, i / size))
        .collect();
    image::save_buffer(path, &pixels, size, size, image::ColorType::Rgba8).unwrap();
}

fn graph_node(id: &str, kind: &str) -> psgc::NodeInstance {
    use psgc::*;
    let meta = get_shader_nodes()
        .into_iter()
        .find(|m| m.name == kind)
        .unwrap_or_else(|| panic!("no shader node {kind}"));
    let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
    for p in meta.params {
        node.inputs.push(PinInstance::new(
            &p.name,
            Pin::new(
                &p.name,
                &p.name,
                DataType::Data(TypeInfo::new(&p.param_type)),
                PinType::Input,
            ),
        ));
    }
    node
}

fn connect(graph: &mut psgc::GraphDescription, from: &str, to: &str, pin: &str) {
    graph.add_connection(psgc::Connection::new(
        from,
        "result",
        to,
        pin,
        psgc::ConnectionType::Data,
    ));
}

fn save_graph(folder: &Path, graph: &psgc::GraphDescription) -> String {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(
        folder.join("shader_graph_save.json"),
        serde_json::json!({ "main_graph": graph }).to_string(),
    )
    .unwrap();
    folder.to_string_lossy().replace('\\', "/")
}

/// A graph whose base colour is `texture` at the mesh UV (and, `masked`,
/// cut out by its alpha).
fn textured_graph(folder: &Path, texture: &Path, masked: bool) -> String {
    let mut graph = psgc::GraphDescription::new("ray tier textured");
    graph.add_node(graph_node("output", "fragment_output"));
    graph.add_node(graph_node("uv", "frag_uv"));
    let mut sample = graph_node("texture", "sample_texture");
    sample.properties.insert(
        "texture".into(),
        serde_json::json!(texture.to_string_lossy().replace('\\', "/")),
    );
    graph.add_node(sample);
    connect(&mut graph, "uv", "texture", "uv");
    connect(&mut graph, "texture", "output", "base_color");
    if masked {
        graph.add_node(graph_node("alpha", "component_a"));
        connect(&mut graph, "texture", "alpha", "vec");
        connect(&mut graph, "alpha", "output", "opacity_mask");
    }
    save_graph(folder, &graph)
}

/// A graph whose base colour is `colour`, computed by the graph.
fn colour_graph(folder: &Path, colour: [f32; 3]) -> String {
    let mut graph = psgc::GraphDescription::new("ray tier colour");
    graph.add_node(graph_node("output", "fragment_output"));
    let mut constant = graph_node("colour", "constant_vec4");
    for (name, value) in ["x", "y", "z", "w"]
        .into_iter()
        .zip([colour[0], colour[1], colour[2], 1.0])
    {
        constant
            .properties
            .insert(name.into(), serde_json::json!(value.to_string()));
    }
    graph.add_node(constant);
    connect(&mut graph, "colour", "output", "base_color");
    save_graph(folder, &graph)
}

// ── Scene ───────────────────────────────────────────────────────────────────

/// An axis-aligned box (centre, half extent) as one static mesh, each face
/// UV 0..1, drawn with `material` (a material asset path; empty: `colour`).
fn mesh_box(
    key: u128,
    centre: Vec3,
    half: Vec3,
    material: &str,
    colour: [f32; 3],
) -> StaticMeshComponent {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let n = Vec3::from_array(std::array::from_fn(|k| if k == axis { sign } else { 0.0 }));
            let u = Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 1) % 3)));
            let v = Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 2) % 3)));
            let (u, v) = if sign > 0.0 { (u, v) } else { (v, u) };
            let base = vertices.len() as u32;
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = centre + (n + u * a + v * b) * half;
                vertices.push(PackedVertex::from_components(
                    p.to_array(),
                    n.to_array(),
                    [(a + 1.0) * 0.5, (1.0 - b) * 0.5],
                    u.to_array(),
                    1.0,
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let radius = vertices
        .iter()
        .map(|v| Vec3::from(v.position).length())
        .fold(0.0f32, f32::max);
    let slot = if material.is_empty() {
        StaticMeshMaterialSlot {
            surface_override: Some(ImportedSurfaceMaterial {
                base_color: [colour[0], colour[1], colour[2], 1.0],
                roughness: 0.8,
                metallic: 0.0,
                emissive: [0.0; 3],
                emissive_intensity: 0.0,
                alpha: 1.0,
            }),
            ..Default::default()
        }
    } else {
        StaticMeshMaterialSlot {
            material_asset: material.into(),
            ..Default::default()
        }
    };
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::new(""),
        geometry_key: MeshGeometryKey(key),
        material_slots: StaticMeshMaterialSlots { slots: vec![slot] },
        mesh_sections: vec![MeshSection {
            first_index: 0,
            index_count: indices.len() as u32,
            material_slot: 0,
        }],
        vertices: vertices.into(),
        indices: indices.into(),
        bounds_local: [0.0, 0.0, 0.0, radius],
        ..Default::default()
    }
}

/// Places `mesh` on an object of its own at `position`.
fn place(
    world: &mut pulsar_scenedb::World,
    name: &str,
    position: [f64; 3],
    mesh: StaticMeshComponent,
) {
    let object = world
        .spawn_object(engine_backend::scene::SpawnObject::new(name))
        .expect("spawn");
    world
        .get_mut::<engine_backend::scene::Transform>(object)
        .unwrap()
        .position = position;
    pulsar_world_registry::attach_value(world, object, mesh).unwrap();
}

fn sun(world: &mut pulsar_scenedb::World, rotation: [f64; 3]) {
    let object = world
        .spawn_object(engine_backend::scene::SpawnObject::new("sun"))
        .expect("spawn");
    let mut light = installed_plugins::lights_plugin::lights_component::LightComponent::default();
    light.general.light_type =
        installed_plugins::lights_plugin::lights_component::LightType::Directional;
    light.intensity.intensity = 6.0;
    pulsar_world_registry::attach_value(world, object, light).unwrap();
    world
        .get_mut::<engine_backend::scene::Transform>(object)
        .unwrap()
        .rotation = rotation.map(|r| r as _);
}

// ── Frames ──────────────────────────────────────────────────────────────────

struct Frames {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Frames {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ray tier"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            device,
            queue,
            texture,
            view,
        }
    }

    /// `frames` frames of `renderer` (waiting, after each, for the
    /// pipelines it compiles off the frame), then the last one's pixels.
    fn render(&self, renderer: &mut HelioRenderer, frames: usize) -> Vec<u8> {
        for _ in 0..frames {
            renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
            let _ =
                renderer.render_frame(&self.device, &self.queue, &self.view, WIDTH, HEIGHT, FORMAT);
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            while helio_core::compile::in_flight() > 0 {
                std::thread::yield_now();
            }
        }
        self.read()
    }

    fn read(&self) -> Vec<u8> {
        let row = WIDTH * 4;
        let padded = row.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * HEIGHT),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(HEIGHT),
                },
            },
            self.texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let data = slice.get_mapped_range().unwrap();
        data.chunks_exact(padded as usize)
            .flat_map(|line| line[..row as usize].iter().copied())
            .collect()
    }
}

fn dump(name: &str, rgba: &[u8]) {
    if let Ok(dir) = std::env::var("RAY_TIER_DUMP_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_raw(WIDTH, HEIGHT, rgba.to_vec())
            .unwrap()
            .save(Path::new(&dir).join(format!("{name}.png")))
            .unwrap();
    }
}

/// The pixel where world point `p` is seen (the editor camera at `EYE`
/// looking down by `PITCH`, 60 degrees vertical).
fn pixel_of(p: Vec3) -> (u32, u32) {
    let eye = Vec3::new(EYE[0] as f32, EYE[1] as f32, EYE[2] as f32);
    let forward = Vec3::new(0.0, PITCH.sin(), -PITCH.cos());
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let d = p - eye;
    let (x, y, z) = (d.dot(right), d.dot(up), d.dot(forward));
    let f = 1.0 / (30f32.to_radians()).tan();
    let aspect = WIDTH as f32 / HEIGHT as f32;
    let nx = x / z * f / aspect;
    let ny = y / z * f;
    (
        ((nx * 0.5 + 0.5) * WIDTH as f32) as u32,
        ((0.5 - ny * 0.5) * HEIGHT as f32) as u32,
    )
}

/// Mean linear RGB of an 9x9 window around the pixel of `p`.
fn mean_at(rgba: &[u8], p: Vec3) -> Vec3 {
    let (cx, cy) = pixel_of(p);
    let mut sum = Vec3::ZERO;
    let mut n = 0.0;
    for y in cy.saturating_sub(4)..(cy + 5).min(HEIGHT) {
        for x in cx.saturating_sub(4)..(cx + 5).min(WIDTH) {
            let at = ((y * WIDTH + x) * 4) as usize;
            sum += Vec3::new(rgba[at] as f32, rgba[at + 1] as f32, rgba[at + 2] as f32) / 255.0;
            n += 1.0;
        }
    }
    sum / n
}

fn renderer_for(state: &LevelEditorState) -> HelioRenderer {
    let mut renderer = HelioRenderer::new(state.scene.shared_scene());
    renderer.set_editor_camera_state(EditorCameraState {
        position: EYE,
        yaw: 0.0,
        pitch: PITCH,
    });
    renderer
        .editor_mailbox()
        .set_render_pipeline(Some(shipped_pipeline()));
    renderer
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// The graphs this suite draws compile (the materials' graph compiler) and
/// compose into the meshes' hit program (`program_source`: the drawers'
/// composition, lowered to explicit gradients) in both binding modes: what
/// the hit programs compile off the frame is valid WGSL.
#[test]
fn material_graphs_compose_into_the_hit_program() {
    use installed_plugins::materials_plugin::materials_component::graph::compile_material_graph;
    let dir = assets();
    let texture = dir.join("red.png");
    write_png(&texture, 16, |_, _| [230, 20, 20, 255]);
    let read = |folder: String| -> psgc::GraphDescription {
        let save: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(&folder).join("shader_graph_save.json")).unwrap(),
        )
        .unwrap();
        serde_json::from_value(save["main_graph"].clone()).unwrap()
    };
    let graphs = [
        (
            read(colour_graph(&dir.join("compose_colour"), [0.1, 0.85, 0.1])),
            Default::default(),
        ),
        (
            read(textured_graph(
                &dir.join("compose_textured"),
                &texture,
                true,
            )),
            std::collections::HashMap::from([(texture.to_string_lossy().replace('\\', "/"), 0u32)]),
        ),
    ];
    let template = helio_mats::RadiantTemplateRegistry::new()
        .get(0)
        .cloned()
        .unwrap();
    for (graph, bindings) in graphs {
        let wgsl = compile_material_graph(&graph, &bindings).expect("the graph compiles");
        for binding in [
            helio_mats::MaterialBindingConfig {
                mode: helio_mats::MaterialBindingMode::BindingArray,
                max_textures: 256,
            },
            helio_mats::MaterialBindingConfig {
                mode: helio_mats::MaterialBindingMode::Expanded,
                max_textures: 16,
            },
        ] {
            let text = helio_pass_mesh_rays::surfaces::program_source(&template, &wgsl, binding);
            let module = wgpu::naga::front::wgsl::parse_str(&text)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&text)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&text)));
        }
    }
}

/// Mean linear RGB of the window `x0..x1`, `y0..y1`.
fn mean_in(rgba: &[u8], (x0, x1): (u32, u32), (y0, y1): (u32, u32)) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for y in y0..y1 {
        for x in x0..x1 {
            let at = ((y * WIDTH + x) * 4) as usize;
            sum += Vec3::new(rgba[at] as f32, rgba[at + 1] as f32, rgba[at + 2] as f32) / 255.0;
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f32
}

/// GI's bounce off a textured graph wall is red, off a procedural graph
/// wall green, and off a grey wall grey, on the ground before each: the
/// probes' hits shaded by each material's program (a procedural graph's
/// colour is in no row and no texture: only its program gives it).
#[test]
fn gi_bounce_takes_each_material_s_colour() {
    // The compile jobs' failures are `log` errors: shown.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let Some((device, queue)) = device() else {
        return;
    };
    let frames = Frames::new(device, queue);
    let dir = assets();
    let red = dir.join("red.png");
    write_png(&red, 16, |_, _| [230, 20, 20, 255]);
    let textured = textured_graph(&dir.join("red_wall"), &red, false);
    let green = colour_graph(&dir.join("green_wall"), [0.1, 0.85, 0.1]);
    let state = LevelEditorState::new();
    {
        let mut world = state.scene.world_mut();
        place(
            &mut world,
            "ground",
            [0.0, 0.0, 0.0],
            mesh_box(
                0x7a7e_0001,
                Vec3::new(0.0, -0.1, 0.0),
                Vec3::new(12.0, 0.1, 12.0),
                "",
                [0.8; 3],
            ),
        );
        for (k, (material, colour)) in [
            (textured.as_str(), [0.5; 3]),
            (green.as_str(), [0.5; 3]),
            ("", [0.5; 3]),
        ]
        .into_iter()
        .enumerate()
        {
            let x = (k as f32 - 1.0) * 3.5;
            place(
                &mut world,
                &format!("wall {k}"),
                [0.0, 0.0, 0.0],
                mesh_box(
                    0x7a7e_0010 + k as u128,
                    Vec3::new(x, 1.5, -2.0),
                    Vec3::new(1.4, 1.5, 0.15),
                    material,
                    colour,
                ),
            );
        }
        // A high sun from behind the eye: the walls' faces toward the eye
        // lit, their bounce on the ground before them.
        sun(&mut world, [-55.0, 160.0, 0.0]);
    }
    let mut renderer = renderer_for(&state);
    renderer
        .editor_mailbox()
        .set_viewport_debug_mode(INDIRECT_VIEW);
    let indirect = frames.render(&mut renderer, 240);
    dump("indirect", &indirect);
    let stat = |name: &str| {
        renderer
            .frame_stats()
            .and_then(|stats| stats.get(name))
            .unwrap_or(0)
    };
    let programs = stat("meshes.ray_programs");
    eprintln!(
        "the ray-traced scene's programs: {programs}; hit surfaces handed {}, bins {}, compiling {}, waiting for a graph {}, failed {}",
        stat("meshes.hit_programs_seen"),
        stat("meshes.hit_bins"),
        stat("meshes.hit_programs_pending"),
        stat("meshes.hit_programs_waiting"),
        stat("meshes.hit_programs_failed"),
    );
    assert!(
        programs >= 3,
        "the graphs' programs are not the rows' (only {programs} programs)"
    );
    // The ground just before each wall's foot: the editor camera at `EYE`
    // sees the walls' feet at row 95, their middles at columns 72, 160 and
    // 248 of this 320 x 180 view (measured on the frames).
    let before = |k: usize| {
        let x = 72 + 88 * k as u32;
        mean_in(&indirect, (x - 30, x + 30), (99, 111))
    };
    let (textured, graph, grey) = (before(0), before(1), before(2));
    eprintln!("bounce before the walls: textured {textured}, graph {graph}, grey {grey}");
    let chroma = |c: Vec3| c / c.max_element().max(1e-4);
    assert!(
        grey.max_element() > 0.02,
        "no indirect light on the ground: {grey}"
    );
    assert!(
        chroma(textured).x - chroma(textured).y > chroma(grey).x - chroma(grey).y + 0.05,
        "the textured wall's bounce is not its texture's red: {textured} (grey {grey})"
    );
    assert!(
        chroma(graph).y - chroma(graph).x > chroma(grey).y - chroma(grey).x + 0.05,
        "the graph wall's bounce is not the graph's green: {graph} (grey {grey})"
    );
}

/// A card cut out by its graph's texture alpha (left half leaves, right
/// half holes), upright between the sun and the ground: the ground in its
/// shadow is darker behind the leaves than behind the holes, in the lit
/// frame and in GI's sun visibility.
#[test]
fn a_cut_out_casts_the_shadow_of_its_leaves_only() {
    let Some((device, queue)) = device() else {
        return;
    };
    let frames = Frames::new(device, queue);
    let dir = assets();
    let leaf = dir.join("half_leaf.png");
    write_png(&leaf, 64, |x, _| {
        if x < 32 {
            [40, 160, 40, 255]
        } else {
            [40, 160, 40, 0]
        }
    });
    let card = textured_graph(&dir.join("half_card"), &leaf, true);
    let state = LevelEditorState::new();
    {
        let mut world = state.scene.world_mut();
        place(
            &mut world,
            "ground",
            [0.0, 0.0, 0.0],
            mesh_box(
                0x7a7e_0101,
                Vec3::new(0.0, -0.1, 0.0),
                Vec3::new(12.0, 0.1, 12.0),
                "",
                [0.8; 3],
            ),
        );
        // The card: 4 m wide, 3 m tall, facing the eye; its UV u runs
        // along +x, so its left half (x < 0) is leaves.
        place(
            &mut world,
            "card",
            [0.0, 0.0, 0.0],
            mesh_box(
                0x7a7e_0102,
                Vec3::new(0.0, 2.0, -1.0),
                Vec3::new(2.0, 1.5, 0.01),
                &card,
                [0.5; 3],
            ),
        );
        // A sun from behind the card, high: its shadow falls toward the eye.
        sun(&mut world, [-50.0, 0.0, 0.0]);
    }
    let mut renderer = renderer_for(&state);
    let lit = frames.render(&mut renderer, 240);
    dump("cut_out_lit", &lit);
    // The shadow falls about 1.7 m toward the eye from the card's middle.
    let leaves = mean_at(&lit, Vec3::new(-1.0, 0.0, 0.9));
    let holes = mean_at(&lit, Vec3::new(1.0, 0.0, 0.9));
    eprintln!("ground behind the leaves {leaves}, behind the holes {holes}");
    assert!(
        leaves.length() < holes.length() * 0.8,
        "the cut-out's holes cast a shadow too: leaves {leaves}, holes {holes}"
    );
}

/// #1744: a static mesh that can deform (`Dynamic`) and whose vertices
/// change in place (no content key: a private GPU range) is refit in the
/// engine world's TLAS, its BLAS kept; a change of its triangles rebuilds
/// it. A rigid mesh beside it is built once and never again.
#[test]
fn a_deforming_static_mesh_is_refit_when_its_vertices_change_in_place() {
    let Some((device, queue)) = device() else {
        return;
    };
    let frames = Frames::new(device, queue);
    let state = LevelEditorState::new();
    {
        let mut world = state.scene.world_mut();
        sun(&mut world, [-0.9, 0.3, 0.0]);
        place(
            &mut world,
            "ground",
            [0.0, -0.5, 0.0],
            mesh_box(
                0xd5_0001,
                Vec3::ZERO,
                Vec3::new(8.0, 0.5, 8.0),
                "",
                [0.8; 3],
            ),
        );
        let mut cloth = mesh_box(0, Vec3::ZERO, Vec3::splat(0.5), "", [0.6, 0.3, 0.2]);
        cloth.geometry_key = MeshGeometryKey::NONE;
        cloth.movability = helio_component::components::ObjectMovability::Dynamic;
        place(&mut world, "cloth", [0.0, 1.0, 0.0], cloth);
    }
    let cloth = {
        let world = state.scene.world();
        world
            .query::<&StaticMeshComponent>()
            .find(|(_, mesh)| mesh.geometry_key == MeshGeometryKey::NONE)
            .map(|(entity, _)| entity)
            .expect("the deforming mesh")
    };
    let mut renderer = renderer_for(&state);
    let counters = |renderer: &HelioRenderer| {
        renderer
            .host_part::<installed_plugins::meshes_plugin::meshes_component::host_part::MeshesHostPart>()
            .and_then(|part| part.ray_scene())
            .map(|rays| (rays.blas_counters(), rays.instance_count()))
            .expect("the ray-traced scene")
    };
    frames.render(&mut renderer, 4);
    let (before, instances) = counters(&renderer);
    assert_eq!(instances, 2, "the ground and the deforming mesh are traced");

    // Its vertices move in place: a refit, not a new build.
    {
        let mut world = state.scene.world_mut();
        let mut mesh = world.get_mut::<StaticMeshComponent>(cloth).unwrap();
        let mut moved = mesh.vertices.to_vec();
        for vertex in &mut moved {
            vertex.position[1] += 0.25;
        }
        mesh.vertices = moved.into();
    }
    frames.render(&mut renderer, 2);
    let (moved, instances) = counters(&renderer);
    println!("before {before:?}, after the vertices moved {moved:?}");
    assert_eq!(instances, 2, "it stays traced");
    assert!(moved.refits > before.refits, "{before:?} -> {moved:?}");
    assert_eq!(moved.builds, before.builds, "no BLAS was built again");

    // Nothing changed: nothing is refit.
    frames.render(&mut renderer, 2);
    let (idle, _) = counters(&renderer);
    assert_eq!((idle.refits, idle.builds), (moved.refits, moved.builds));

    // Its triangles change: a rebuild.
    {
        let mut world = state.scene.world_mut();
        let mut mesh = world.get_mut::<StaticMeshComponent>(cloth).unwrap();
        let mut swapped = mesh.indices.to_vec();
        swapped.swap(0, 1);
        mesh.indices = swapped.into();
    }
    frames.render(&mut renderer, 2);
    let (rebuilt, instances) = counters(&renderer);
    assert_eq!(instances, 2);
    assert!(rebuilt.builds > idle.builds, "{idle:?} -> {rebuilt:?}");
}
