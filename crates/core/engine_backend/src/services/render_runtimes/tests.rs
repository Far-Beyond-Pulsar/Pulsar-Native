//! Tests for the runtime lifecycle, update modes, driven runtimes and the
//! scheduler drawing an offscreen output.
use super::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::services::gpu_renderer::GpuRendererBuilder;

fn description(label: &str, mode: UpdateMode) -> RuntimeDescription {
    RuntimeDescription {
        label: label.into(),
        origin: pulsar_plugins::engine_origin!(),
        scene_label: "test".into(),
        priority: 0,
        mode,
    }
}

fn renderer() -> SharedRenderer {
    Arc::new(Mutex::new(GpuRendererBuilder::new(1, 1).build()))
}

fn task_of(label: &str) -> Option<editor_task_queue::TaskSnapshot> {
    let title = format!("Helio runtime: {label}");
    editor_task_queue::global()
        .snapshots()
        .into_iter()
        .find(|task| task.title == title)
}

#[test]
fn a_runtime_is_a_task_listed_with_its_details_until_its_last_handle_drops() {
    let label = "unit: lifecycle";
    let handle = global().create(description(label, UpdateMode::WhenVisible), renderer());
    assert!(global().list().iter().any(|(id, _, _)| *id == handle.id()));
    // No output yet: it waits to start, and its task says so (idle).
    assert_eq!(handle.state(), RuntimeState::Starting);
    let task = task_of(label).expect("the runtime's task");
    assert_eq!(task.category, RUNTIME_CATEGORY);
    assert!(task.idle);
    let details: Vec<&str> = task.details.iter().map(|(k, _)| k.as_str()).collect();
    for key in ["Owner", "Scene", "Size", "Mode", "State"] {
        assert!(details.contains(&key), "{key} in {details:?}");
    }
    assert!(task.details.contains(&("Size".into(), "no output".into())));

    let clone = handle.clone();
    let id = handle.id();
    drop(handle);
    assert!(global().list().iter().any(|(listed, _, _)| *listed == id));
    drop(clone);
    assert!(!global().list().iter().any(|(listed, _, _)| *listed == id));
    assert!(task_of(label).unwrap().status.is_finished());
}

#[test]
fn update_modes_decide_whether_a_runtime_wants_frames() {
    let handle = global().create(
        description("unit: modes", UpdateMode::WhenVisible),
        renderer(),
    );
    let runtime = &handle.runtime;
    assert!(runtime.wants_frames(), "shown by default");
    handle.set_visible(false);
    assert!(!runtime.wants_frames(), "hidden");
    handle.set_mode(UpdateMode::Always);
    assert!(runtime.wants_frames(), "always, hidden or not");
    handle.set_mode(UpdateMode::OnDemand(0));
    assert!(!runtime.wants_frames());
    handle.request_frames(2);
    assert!(runtime.wants_frames());
    handle.set_mode(UpdateMode::Disabled);
    assert!(!runtime.wants_frames());
}

#[test]
fn a_paused_driven_runtime_draws_nothing() {
    let runtime = DrivenRuntime::register(description("unit: driven", UpdateMode::Always));
    {
        let _frame = runtime.begin_frame().expect("running");
    }
    runtime.pause();
    assert!(runtime.begin_frame().is_none());
    runtime.resume();
    assert!(runtime.begin_frame().is_some());
    let task = task_of("unit: driven").expect("its task");
    assert!(task.beats >= 1, "a frame is a heartbeat");
}

/// The scheduler draws an offscreen runtime on the engine's device and its
/// readback arrives without blocking; stopping frees the renderer and
/// resuming builds it again.
#[test]
fn the_scheduler_draws_an_offscreen_runtime_and_stop_frees_its_renderer() {
    let Some(target) = OffscreenTarget::new(32, 32) else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: render runtimes");
        return;
    };
    let target = Arc::new(target);
    let handle = global().create(
        description("unit: offscreen", UpdateMode::Always),
        renderer(),
    );
    handle.set_output(target.clone());
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut image = None;
    while image.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        image = target.take_image();
    }
    let image = image.expect("a frame was drawn and read back");
    assert_eq!(image.len(), 32 * 32 * 4);
    assert_eq!(handle.state(), RuntimeState::Running);

    handle.stop();
    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.state() != RuntimeState::Stopped && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(handle.state(), RuntimeState::Stopped);
    assert!(!handle.renderer().lock().unwrap().is_initialized());

    let drawn = handle.frames();
    handle.resume();
    let deadline = Instant::now() + Duration::from_secs(120);
    while handle.frames() == drawn && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(handle.frames() > drawn, "resumed: built again and drawing");
}
