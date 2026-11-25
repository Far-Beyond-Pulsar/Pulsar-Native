//! A render pipeline builds off the frame thread: a renderer asked for one
//! keeps drawing frames (the status screen saying it is building, or the
//! pipeline it already runs) and swaps the new graph in at the first frame
//! boundary after the build ends.
//!
//! The pipeline here is an empty graph whose build is held at its start by a
//! gate the test opens, so the frames in between prove nothing waited for it.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use helio::{
    Camera, IntoGraph, NamedPipeline, PassBuildContext, PipelineFailure, PipelineStatus, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_core::RenderGraph;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
}

/// A pipeline whose build starts when the gate opens.
struct Gated {
    gate: Arc<Gate>,
    started: Arc<Mutex<bool>>,
}

impl IntoGraph for Gated {
    fn into_graph(
        self: Box<Self>,
        ctx: PassBuildContext<'_>,
    ) -> Result<RenderGraph, PipelineFailure> {
        *self.started.lock().unwrap() = true;
        self.gate.wait();
        let mut graph = RenderGraph::new_with_external_device(ctx.device, ctx.queue);
        graph.lock(ctx.config.width, ctx.config.height);
        Ok(graph)
    }
}

fn gated(id: &str) -> (NamedPipeline, Arc<Gate>, Arc<Mutex<bool>>) {
    let gate = Arc::new(Gate::default());
    let started = Arc::new(Mutex::new(false));
    let pipeline = NamedPipeline::new(
        id,
        Gated {
            gate: gate.clone(),
            started: started.clone(),
        },
    );
    (pipeline, gate, started)
}

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(adapter.features()),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu {
        device: Arc::new(device),
        queue: Arc::new(queue),
    })
}

fn renderer(gpu: &Gpu) -> Renderer {
    let store = SceneGpuStore::new(
        &EngineGpuContext::new(gpu.device.clone(), gpu.queue.clone()),
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    let scene_db = GpuMirrorHandle::new(Arc::new(store), gpu.queue.clone());
    let config = RendererConfig::new(256, 256, FORMAT);
    RendererBuilder::new(config, scene_db)
        .with_external_device()
        .build(gpu.device.clone(), gpu.queue.clone(), 256, 256, FORMAT)
}

fn frame(gpu: &Gpu, renderer: &mut Renderer) {
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pipeline build test target"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let camera = Camera::perspective_look_at(
        glam::Vec3::new(0.0, 2.0, 5.0),
        glam::Vec3::ZERO,
        glam::Vec3::Y,
        1.0,
        1.0,
        0.1,
        100.0,
    );
    renderer.render(&camera, &view).expect("a frame draws");
}

/// Draws frames until the renderer has no pipeline change on its way.
fn frames_until_installed(gpu: &Gpu, renderer: &mut Renderer) -> usize {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut frames = 0;
    while renderer.pipeline_change_pending() {
        assert!(Instant::now() < deadline, "the pipeline never installed");
        frame(gpu, renderer);
        frames += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
    frames
}

#[test]
fn a_pipeline_builds_while_frames_draw_and_installs_when_done() {
    let Some(gpu) = gpu() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: pipeline build off the frame thread");
        return;
    };
    let mut renderer = renderer(&gpu);
    assert_eq!(renderer.pipeline_status(), &PipelineStatus::None);

    let (pipeline, gate, started) = gated("test.first");
    renderer.set_pipeline(Some(pipeline));
    assert!(renderer.pipeline_change_pending());

    // The build is held open, yet the frame returns: the renderer drew the
    // status screen, saying the pipeline is on its way.
    frame(&gpu, &mut renderer);
    assert_eq!(renderer.pipeline_building(), Some("test.first"));
    assert_eq!(
        renderer.pipeline_status(),
        &PipelineStatus::Building {
            id: "test.first".into()
        }
    );
    assert!(renderer.needs_frame(), "a host that draws on demand keeps drawing");
    for _ in 0..3 {
        frame(&gpu, &mut renderer);
    }
    assert!(renderer.pipeline_building().is_some());

    gate.open();
    frames_until_installed(&gpu, &mut renderer);
    assert!(*started.lock().unwrap());
    assert_eq!(renderer.pipeline_status(), &PipelineStatus::Ready);
    assert_eq!(renderer.pipeline_id(), Some("test.first"));
    assert_eq!(renderer.pipeline_building(), None);
}

#[test]
fn a_pipeline_replacing_one_that_renders_builds_while_that_one_draws() {
    let Some(gpu) = gpu() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: pipeline swap off the frame thread");
        return;
    };
    let mut renderer = renderer(&gpu);
    let (first, gate, _) = gated("test.one");
    gate.open();
    renderer.set_pipeline(Some(first));
    frames_until_installed(&gpu, &mut renderer);
    assert_eq!(renderer.pipeline_id(), Some("test.one"));
    assert!(renderer.pipeline_status().is_ready());

    let (second, gate, _) = gated("test.two");
    renderer.set_pipeline(Some(second));
    frame(&gpu, &mut renderer);
    // Still the first pipeline's graph; the second builds behind it.
    assert_eq!(renderer.pipeline_status(), &PipelineStatus::Ready);
    assert_eq!(renderer.pipeline_id(), Some("test.one"));
    assert_eq!(renderer.pipeline_building(), Some("test.two"));
    for _ in 0..3 {
        frame(&gpu, &mut renderer);
        assert_eq!(renderer.pipeline_id(), Some("test.one"));
    }

    gate.open();
    frames_until_installed(&gpu, &mut renderer);
    assert_eq!(renderer.pipeline_status(), &PipelineStatus::Ready);
    assert_eq!(renderer.pipeline_id(), Some("test.two"));
}

#[test]
fn a_pipeline_assigned_again_while_building_replaces_the_build() {
    let Some(gpu) = gpu() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: pipeline reassigned while building");
        return;
    };
    let mut renderer = renderer(&gpu);
    let (slow, slow_gate, _) = gated("test.slow");
    renderer.set_pipeline(Some(slow));
    frame(&gpu, &mut renderer);
    assert_eq!(renderer.pipeline_building(), Some("test.slow"));

    let (next, next_gate, _) = gated("test.next");
    next_gate.open();
    renderer.set_pipeline(Some(next));
    frames_until_installed(&gpu, &mut renderer);
    assert_eq!(renderer.pipeline_id(), Some("test.next"));
    // The superseded build ends later and is dropped unread.
    slow_gate.open();
    frame(&gpu, &mut renderer);
    assert_eq!(renderer.pipeline_id(), Some("test.next"));
    assert_eq!(renderer.pipeline_status(), &PipelineStatus::Ready);
}

#[test]
fn apply_pending_pipeline_still_waits_for_the_build() {
    let Some(gpu) = gpu() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: blocking pipeline apply");
        return;
    };
    let mut renderer = renderer(&gpu);
    let (pipeline, gate, _) = gated("test.sync");
    gate.open();
    renderer.set_pipeline(Some(pipeline));
    renderer.apply_pending_pipeline();
    assert_eq!(renderer.pipeline_status(), &PipelineStatus::Ready);
    assert_eq!(renderer.pipeline_id(), Some("test.sync"));
}
