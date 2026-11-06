//! The engine's animation phase: it runs before the ECS schedule, so a system
//! (physics reads poses in one) sees the pose evaluated for this very frame.
use meshes_component::components::SkeletalMeshComponent;
use pulsar_game::prelude::{TickLoop, TickMode};
use pulsar_scenedb::World;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn systems_in_the_schedule_read_this_frames_animation_pose() {
    let sequence: skeletal_assets::assets::AnimSeqAsset =
        serde_json::from_value(serde_json::json!({
            "version": 2, "name": "Walk", "duration": 10.0, "fps": 30.0, "tracks": []
        }))
        .unwrap();
    let mut tick_loop = TickLoop::new(
        TickMode::Fixed {
            dt: Duration::from_millis(250),
        },
        0,
    );
    let seen = Arc::new(Mutex::new(Vec::<f32>::new()));
    let log = seen.clone();
    tick_loop
        .schedule
        .add_system("probe", move |world: &mut World, _t| {
            for (_, component) in world.query::<&SkeletalMeshComponent>() {
                log.lock().unwrap().push(component.elapsed_seconds);
            }
        });
    {
        let mut store = tick_loop.scene_store.write();
        let owner = store.world.spawn();
        let mut player = SkeletalMeshComponent::default();
        player.loaded_animation = Some(Arc::new(sequence));
        player.playing = true;
        player.playback_rate = 1.0;
        pulsar_world_registry::attach_value(&mut store.world, owner, player).unwrap();
    }
    for _ in 0..3 {
        tick_loop.tick_once();
    }
    // frame n's schedule already sees n quarter-seconds of playback
    assert_eq!(*seen.lock().unwrap(), [0.25, 0.5, 0.75]);
}
