//! A lost GPU device is noticed, its errors stop panicking once it is lost,
//! and the scene's GPU copy follows a renderer onto a new device (SceneDB#84).
//! Loss is simulated with `Device::destroy`, which reports it through the
//! same callback a driver reset does. Needs a GPU adapter (lavapipe works);
//! skips without one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use engine_backend::scene::{ensure_gpu_mirror, new_scene, SceneWorldExt, SpawnObject};
use engine_backend::services::device_loss::DeviceLoss;

type Gpu = (Arc<wgpu::Device>, Arc<wgpu::Queue>);

/// `count` devices from one adapter of one instance, as a process gets them
/// (wgpu tells devices apart within an instance).
fn devices(count: usize) -> Option<Vec<Gpu>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    (0..count)
        .map(|_| {
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    required_limits: helio::required_wgpu_limits(adapter.limits()),
                    ..Default::default()
                }))
                .ok()?;
            Some((Arc::new(device), Arc::new(queue)))
        })
        .collect()
}

/// A buffer wgpu rejects (mappable for reading and a storage buffer).
fn invalid_buffer(device: &wgpu::Device) {
    let _ = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("invalid"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
}

#[test]
fn a_lost_device_is_noticed_and_its_errors_do_not_panic() {
    let Some((device, _queue)) = devices(1).and_then(|mut gpus| gpus.pop()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let loss = DeviceLoss::watch(&device);
    let again = DeviceLoss::watch(&device);
    assert!(!loss.is_lost());

    // Before a loss, an error no scope caught still panics, as wgpu's own
    // handler does.
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invalid_buffer(&device)));
    assert!(
        caught.is_err(),
        "a GPU error panics while the device is alive"
    );

    device.destroy();
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    assert!(loss.is_lost());
    assert!(again.is_lost(), "every watcher of the device sees the loss");

    // After it, errors are the loss's echo: logged, not a panic.
    invalid_buffer(&device);
}

#[test]
fn an_owner_watching_its_device_keeps_its_callbacks() {
    let Some((device, _queue)) = devices(1).and_then(|mut gpus| gpus.pop()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // The owner (wgpui in the editor) holds the device's callbacks and its
    // own flag.
    let owner_flag = Arc::new(AtomicBool::new(false));
    let flag = owner_flag.clone();
    device.set_device_lost_callback(move |_, _| flag.store(true, Ordering::Release));
    let adopted = DeviceLoss::adopt(&device, owner_flag.clone());
    let watched = DeviceLoss::watch(&device);

    device.destroy();
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    assert!(
        owner_flag.load(Ordering::Acquire),
        "watching the device left the owner's lost callback in place"
    );
    assert!(adopted.is_lost() && watched.is_lost());
}

#[test]
fn the_scene_gpu_copy_follows_the_renderer_onto_a_new_device() {
    let Some(mut gpus) = devices(2) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (second, second_queue) = gpus.pop().unwrap();
    let (first, first_queue) = gpus.pop().unwrap();
    assert!(first.as_ref() != second.as_ref());
    let mut scene = new_scene();
    scene
        .world
        .spawn_object(SpawnObject::new("cube"))
        .expect("spawned");

    let on_first = ensure_gpu_mirror(&mut scene, first.clone(), first_queue.clone());
    assert!(on_first.store().device_arc().as_ref() == first.as_ref());
    let again = ensure_gpu_mirror(&mut scene, first.clone(), first_queue);
    assert!(
        again.shares_state_with(&on_first),
        "another renderer on the same device shares the mirror"
    );

    // The first device is lost; the renderer is rebuilt on the second.
    first.destroy();
    let on_second = ensure_gpu_mirror(&mut scene, second.clone(), second_queue.clone());
    assert!(on_second.store().device_arc().as_ref() == second.as_ref());
    assert!(!on_second.shares_state_with(&on_first));
    assert!(
        scene
            .world
            .gpu_mirror()
            .is_some_and(|mirror| mirror.shares_state_with(&on_second)),
        "the world mirrors to the new device"
    );
    assert!(ensure_gpu_mirror(&mut scene, second, second_queue).shares_state_with(&on_second));
}
