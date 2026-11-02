//! The root-motion authority contract: who moves the owner, and what the
//! authority and the owning client log for each other. No transport is
//! involved; the moves are the exchange format.

use super::phase3_tests::*;
use super::*;
use pulsar_scenedb::World;
use pulsar_world_registry::ComponentRuntimeState;
use RootMotionAuthoritySetting::*;

struct Run {
    position: [f64; 3],
    moves: Vec<RootMotionMove>,
}

fn run(authority: RootMotionAuthoritySetting, with_montage: bool, frames: usize) -> Run {
    let rig = rig();
    let graph = walk_graph(&rig, true);
    let montage = compile_montage_asset(
        &rig,
        &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Montage),
    );
    let hub = registered_hub();
    let mut world = World::new();
    let owner = owner_with_transform(&mut world);
    let mut c = component(&rig, &graph);
    c.root_motion_mode = RootMotionModeSetting::RootMotionFromEverything;
    c.root_motion_authority = authority;
    let instance = pulsar_world_registry::attach_value(&mut world, owner, c).unwrap();
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.0, &mut state);
    if with_montage {
        world
            .get_mut::<SkeletalMeshComponent>(instance)
            .unwrap()
            .play_compiled_montage(montage, 1.0, None)
            .unwrap();
    }
    for _ in 0..frames {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    let moves = world
        .get_mut::<SkeletalMeshComponent>(instance)
        .unwrap()
        .drain_root_motion_moves();
    Run {
        position: owner_position(&world, owner),
        moves,
    }
}

#[test]
fn every_role_but_the_simulated_proxy_moves_its_owner_by_the_animation() {
    let standalone = run(Standalone, false, 8);
    assert!(
        (standalone.position[0] - 18.0).abs() < 1e-2,
        "8 m over 2 s of walking, from x=10: {:?}",
        standalone.position
    );
    for role in [Authority, AutonomousProxy] {
        assert_eq!(
            run(role, false, 8).position,
            standalone.position,
            "{role:?} predicts and applies the same move"
        );
    }
    assert_eq!(
        run(SimulatedProxy, false, 8).position,
        [10.0, 0.0, 0.0],
        "a simulated proxy is moved by replication only"
    );
}

#[test]
fn authority_and_the_owning_client_log_identical_moves_and_the_others_log_none() {
    let server = run(Authority, false, 8);
    let client = run(AutonomousProxy, false, 8);
    assert_eq!(
        server.moves, client.moves,
        "the same input produces the same moves on both sides"
    );
    assert_eq!(
        server.moves.iter().map(|m| m.sequence).collect::<Vec<_>>(),
        (1..=server.moves.len() as u64).collect::<Vec<_>>()
    );
    let travelled: f32 = server
        .moves
        .iter()
        .map(|m| m.delta.translation.length())
        .sum();
    assert!(
        (travelled - 8.0).abs() < 1e-2,
        "the log sums to the walk: {travelled}"
    );
    assert!(server
        .moves
        .iter()
        .all(|m| m.montage.is_none() && m.delta_seconds > 0.0));
    assert!(
        run(Standalone, false, 8).moves.is_empty()
            && run(SimulatedProxy, false, 8).moves.is_empty()
    );
}

#[test]
fn montage_owned_root_motion_is_logged_with_its_playback_so_the_server_can_replay_it() {
    let client = run(AutonomousProxy, true, 4);
    let with_montage: Vec<_> = client
        .moves
        .iter()
        .filter(|m| m.montage.is_some())
        .collect();
    assert!(
        !with_montage.is_empty(),
        "the Montage's playback is named in the moves it moved the owner by"
    );
    let (id, position) = with_montage.last().unwrap().montage.clone().unwrap();
    assert!(!id.is_empty() && position > 0.0, "{id} at {position}");
    assert!(
        with_montage
            .iter()
            .any(|m| m.montage_delta.translation.length() > 0.0),
        "the Montage's own part of the move is separated from the graph's"
    );
    assert_eq!(drain_twice(), 0, "draining hands the moves over once");
}

fn drain_twice() -> usize {
    let rig = rig();
    let graph = walk_graph(&rig, false);
    let mut c = component(&rig, &graph);
    c.root_motion_authority = Authority;
    c.ensure_runtime();
    c.advance(0.25);
    assert!(!c.drain_root_motion_moves().is_empty());
    c.drain_root_motion_moves().len()
}
