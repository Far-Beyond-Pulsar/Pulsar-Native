//! #641 property tests: churn spawn/despawn/reuse while refs are held and
//! prove no cross-object writes can occur through any scripting path.
//!
//! Deterministic (xorshift64*, fixed seeds) -- a failure reproduces exactly;
//! run several seeds to cover different slot-recycling orders.

#![cfg(test)]

use pulsar_scenedb::{Entity, World};

use crate::errors::ScriptRefError;
use crate::refs::{ActorRef, ComponentRef};
use crate::test_support::TestGizmo;

/// xorshift64* -- tiny, deterministic, good enough for scheduling churn.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// One modeled object: its unique marker value and its held handles.
struct Tracked {
    entity: Entity,
    marker: i32,
    actor_ref: ActorRef,
    component_ref: ComponentRef,
}

/// The churn driver: maintains ground truth alongside the World and checks
/// EVERY access against it.
struct Churn {
    world: World,
    tracked: Vec<Tracked>,
    /// Handles whose targets were deliberately despawned -- every one must
    /// report clean staleness forever after.
    retired: Vec<ComponentRef>,
    next_marker: i32,
}

impl Churn {
    fn new() -> Self {
        Self {
            world: World::new(),
            tracked: Vec::new(),
            retired: Vec::new(),
            next_marker: 1,
        }
    }

    fn spawn(&mut self) {
        let marker = self.next_marker;
        self.next_marker += 1;
        let entity = self.world.spawn();
        self.world.insert(entity, TestGizmo { charges: marker });
        let actor_ref = ActorRef::new(entity);
        let component_ref = ComponentRef::live(actor_ref, "TestGizmo");
        self.tracked.push(Tracked {
            entity,
            marker,
            actor_ref,
            component_ref,
        });
    }

    /// Despawn one tracked object through ITS OWN ref (the scripting path),
    /// retiring its handles first so their staleness can be verified later.
    fn despawn_random(&mut self, rng: &mut Rng) {
        if self.tracked.is_empty() {
            return;
        }
        let victim = rng.below(self.tracked.len() as u64) as usize;
        let t = self.tracked.remove(victim);
        assert!(
            t.actor_ref.validate(&self.world).is_ok(),
            "tracked actor was already stale"
        );
        self.retired.push(t.component_ref);
        assert!(
            t.actor_ref.despawn(&mut self.world),
            "despawn of a live tracked actor"
        );
    }

    /// Write through one HELD ref, then prove nothing else moved: the
    /// pre-write World values of every OTHER tracked object are compared
    /// against the World afterward.
    fn write_random(&mut self, rng: &mut Rng) {
        if self.tracked.is_empty() {
            return;
        }
        let index = rng.below(self.tracked.len() as u64) as usize;
        let others_before: Vec<(u64, i32)> = self
            .tracked
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, t)| (t.entity.bits(), t.marker))
            .collect();
        // Distinct from every live marker (markers are positive).
        let new_value = -self.next_marker;
        self.next_marker += 1;
        let r = self.tracked[index].component_ref.clone();

        match r.set_property(&mut self.world, "charges", serde_json::json!(new_value)) {
            Ok(()) => self.tracked[index].marker = new_value,
            Err(ScriptRefError::ReferenceDespawned { .. }) => {
                panic!("live tracked entity refused as despawned: {r:?}")
            }
            Err(other) => panic!("unexpected error writing through a live ref: {other}"),
        }

        // THE #641 invariant: no other object's value moved.
        for (bits, expected) in others_before {
            let entity = Entity::from_bits(bits);
            let actual = self.world.get::<TestGizmo>(entity).map(|g| g.charges);
            assert_eq!(actual, Some(expected), "cross-object write onto {entity:?}");
        }
    }

    /// Read through every held ref; each must see ONLY its own current
    /// value.
    fn verify_all_refs(&self) {
        for t in &self.tracked {
            assert!(
                t.actor_ref.validate(&self.world).is_ok(),
                "live tracked actor failed validation"
            );
            let value = t
                .component_ref
                .get_property(&self.world, "charges")
                .unwrap_or_else(|e| {
                    panic!(
                        "read through a live ref failed: {e} (ref {:?})",
                        t.component_ref
                    )
                });
            assert_eq!(
                value,
                serde_json::json!(t.marker),
                "ref saw another object's value"
            );
        }
    }

    /// Every retired (despawned-under) handle must report clean typed
    /// staleness forever -- never success against a recycled slot.
    fn verify_retired_refs_are_all_rejected(&self) {
        for r in &self.retired {
            match r.get_property(&self.world, "charges") {
                Err(ScriptRefError::ReferenceDespawned { .. }) => {}
                other => panic!("retired ref did not report staleness cleanly: {other:?}"),
            }
        }
    }
}

/// The #641 property test: heavy spawn/despawn/reuse churn while scripts
/// hold refs. No cross-object write ever occurs; no accessor ever panics.
#[test]
fn churn_spawn_despawn_and_slot_reuse_never_crosses_writes() {
    for seed in [0xDEADBEEF, 0x12345678, 0x00C0FFEE] {
        let mut rng = Rng::new(seed);
        let mut sim = Churn::new();

        for _ in 0..2000 {
            match rng.below(100) {
                0..=39 => sim.spawn(),
                40..=69 => sim.despawn_random(&mut rng),
                70..=94 => sim.write_random(&mut rng),
                _ => sim.verify_all_refs(),
            }
        }

        sim.verify_all_refs();
        sim.verify_retired_refs_are_all_rejected();

        // Final consistency: every surviving entity's World value equals the
        // model's expectation.
        for t in &sim.tracked {
            assert_eq!(
                sim.world.get::<TestGizmo>(t.entity).map(|g| g.charges),
                Some(t.marker),
                "final state diverged for {:?}",
                t.entity
            );
        }
    }
}

/// Slot reuse specifically: despawn everything except one survivor, respawn
/// past the freed capacity so slots recycle, and confirm the survivor's ref
/// still addresses only the survivor while every stale ref is refused.
#[test]
fn recycled_slots_never_adopt_stale_handles() {
    let mut world = World::new();
    let survivors_marker = 7i32;

    let survivor = world.spawn();
    world.insert(
        survivor,
        TestGizmo {
            charges: survivors_marker,
        },
    );
    let survivor_ref = ComponentRef::live(ActorRef::new(survivor), "TestGizmo");

    // Fill, kill, refill -- twice over -- so `World` recycles slots with
    // generation bumps (SceneDB's free-list behavior).
    let mut stales = Vec::new();
    for round in 0..4 {
        let mut batch = Vec::new();
        for _ in 0..8 {
            let e = world.spawn();
            world.insert(
                e,
                TestGizmo {
                    charges: 1000 + round,
                },
            );
            batch.push((e, ComponentRef::live(ActorRef::new(e), "TestGizmo")));
        }
        for (e, r) in batch.drain(..) {
            stales.push(r);
            world.despawn(e);
        }
    }

    // The survivor was never touched and its ref still works...
    assert_eq!(
        survivor_ref.get_property(&world, "charges").unwrap(),
        serde_json::json!(survivors_marker)
    );
    // ...while every stale handle reports staleness, never success.
    for r in &stales {
        assert!(matches!(
            r.get_property(&world, "charges"),
            Err(ScriptRefError::ReferenceDespawned { .. })
        ));
    }
}
