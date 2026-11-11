//! Systems: per-frame work a plugin's class brings, ordered by labels.
//!
//! A class's systems come with it (`ClassSpec::systems`, registered by the
//! plugin's root through its builder): each has a **label**
//! (`pulsar.physics.step`), the labels it runs **after** and **before**, and
//! the function run once a frame over the world. A system runs only while
//! its class is registered (its plugin enabled), as the class's own
//! lifecycle does.
//!
//! The engine's native component lifecycles (every `#[component_runtime]`
//! class's `begin_play`/`tick`/`end_play`, [`crate::tick_live_components`])
//! are the node [`COMPONENTS`] of the same order, so a plugin places its
//! systems around them by label, never by link or name order.
//!
//! A label no registered system has (an optional plugin's, disabled) orders
//! nothing; with every plugin enabled each label must exist
//! ([`unknown_labels`], which `plugins/installed` tests). A cycle is
//! reported and its systems run in label order.
//!
//! How others do it: Bevy's `add_systems(Update, step.after(Input).before
//! (Animation))` with system sets, where ordering against an empty set is
//! no error; Unreal's tick groups and `AddTickPrerequisiteComponent`;
//! Unity's `[UpdateBefore]`/`[UpdateAfter]` on ECS systems. Ours is Bevy's
//! labelled ordering on one schedule.

use std::collections::{BTreeMap, BTreeSet};

use pulsar_events::EventHub;
use pulsar_scenedb::World;

use crate::ComponentRuntimeState;

/// The label of the engine's component lifecycles in the system order.
pub const COMPONENTS: &str = "pulsar.components";

/// What a system runs with each frame.
pub struct SystemContext<'a> {
    pub world: &'a mut World,
    pub events: &'a EventHub,
    pub delta_seconds: f32,
    /// The session's component lifecycle state (subscriptions, inboxes).
    pub runtime: &'a mut ComponentRuntimeState,
}

/// A system as a class declares it (`ClassSpec::systems`).
#[derive(Clone, Copy, Debug)]
pub struct System {
    /// Stable, namespaced by its plugin (`pulsar.physics.step`).
    pub label: &'static str,
    /// Labels this system runs after.
    pub after: &'static [&'static str],
    /// Labels this system runs before.
    pub before: &'static [&'static str],
    pub run: fn(&mut SystemContext<'_>),
}

impl System {
    pub const fn new(label: &'static str, run: fn(&mut SystemContext<'_>)) -> Self {
        Self {
            label,
            after: &[],
            before: &[],
            run,
        }
    }

    pub const fn after(mut self, labels: &'static [&'static str]) -> Self {
        self.after = labels;
        self
    }

    pub const fn before(mut self, labels: &'static [&'static str]) -> Self {
        self.before = labels;
        self
    }
}

/// A registered system: one of a class's, with the class that gates it.
pub struct SystemRegistration {
    /// The plugin root that registered the class.
    pub origin: &'static str,
    /// The class whose registration gates it.
    pub class_name: &'static str,
    pub system: System,
}

impl pulsar_plugins::Contribution for SystemRegistration {
    const CATEGORY: &'static str = "gameplay.systems";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.system.label.to_owned()
    }

    fn label(&self) -> String {
        let mut text = self.system.label.to_owned();
        if !self.system.after.is_empty() {
            text.push_str(&format!(" (after {})", self.system.after.join(", ")));
        }
        if !self.system.before.is_empty() {
            text.push_str(&format!(" (before {})", self.system.before.join(", ")));
        }
        text
    }
}

/// One step of the frame's order.
#[derive(Clone, Copy)]
pub enum Step {
    /// The engine's component lifecycles ([`COMPONENTS`]).
    Components,
    System(&'static SystemRegistration),
}

/// The order of `systems` with the component lifecycles: each system after
/// every label it names in `after` and before every label in `before`;
/// unconstrained ones by label. A cycle is returned as a problem, with an
/// order that still runs every system.
pub fn order(systems: &[&'static SystemRegistration]) -> (Vec<Step>, Vec<String>) {
    let mut problems = Vec::new();
    let labels: BTreeSet<&str> = systems
        .iter()
        .map(|s| s.system.label)
        .chain([COMPONENTS])
        .collect();
    // Edges: label -> labels that must run after it.
    let mut after_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for registration in systems {
        let system = &registration.system;
        for &earlier in system.after.iter().filter(|l| labels.contains(*l)) {
            after_of.entry(earlier).or_default().insert(system.label);
        }
        for &later in system.before.iter().filter(|l| labels.contains(*l)) {
            after_of.entry(system.label).or_default().insert(later);
        }
    }
    let mut incoming: BTreeMap<&str, usize> = labels.iter().map(|l| (*l, 0)).collect();
    for laters in after_of.values() {
        for later in laters {
            *incoming.get_mut(later).expect("a known label") += 1;
        }
    }
    let mut ready: BTreeSet<&str> = incoming
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(l, _)| *l)
        .collect();
    let mut ordered: Vec<&str> = Vec::new();
    while let Some(label) = ready.pop_first() {
        ordered.push(label);
        for later in after_of.get(label).into_iter().flatten() {
            let n = incoming.get_mut(later).expect("a known label");
            *n -= 1;
            if *n == 0 {
                ready.insert(later);
            }
        }
    }
    if ordered.len() < labels.len() {
        let stuck: Vec<&str> = labels
            .iter()
            .copied()
            .filter(|l| !ordered.contains(l))
            .collect();
        problems.push(format!(
            "systems {} order each other in a cycle; they run in label order",
            stuck.join(", ")
        ));
        ordered.extend(stuck);
    }
    let steps = ordered
        .into_iter()
        .flat_map(|label| {
            if label == COMPONENTS {
                vec![Step::Components]
            } else {
                systems
                    .iter()
                    .filter(|s| s.system.label == label)
                    .map(|s| Step::System(s))
                    .collect()
            }
        })
        .collect();
    (steps, problems)
}

/// The labels `systems` order against that none of them has (nor the
/// component lifecycles): with every plugin enabled, a typo.
pub fn unknown_labels(systems: &[&'static SystemRegistration]) -> Vec<String> {
    let labels: BTreeSet<&str> = systems
        .iter()
        .map(|s| s.system.label)
        .chain([COMPONENTS])
        .collect();
    systems
        .iter()
        .flat_map(|s| {
            s.system
                .after
                .iter()
                .chain(s.system.before)
                .filter(|label| !labels.contains(*label))
                .map(move |label| format!("system {} orders against `{label}`", s.system.label))
        })
        .collect()
}

/// Every system the linked plugins register, whatever their state.
pub fn all_systems() -> Vec<&'static SystemRegistration> {
    pulsar_plugins::built::<SystemRegistration>()
}

fn all_system_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    pulsar_plugins::built::<SystemRegistration>()
        .into_iter()
        .map(|registration| registration.summary())
        .collect()
}

fn active_system_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    crate::runtime::systems()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

// Systems are gated with their class (across plugin libraries, like the
// classes), so their catalog source reads the registry.
inventory::submit! {
    pulsar_plugins::ContributionSource {
        category: "gameplay.systems",
        all: all_system_summaries,
        active: active_system_summaries,
        // Systems come only from a class's spec, through a plugin's build.
        linked: Vec::new,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: &mut SystemContext<'_>) {}

    fn registration(system: System) -> &'static SystemRegistration {
        Box::leak(Box::new(SystemRegistration {
            origin: "test",
            class_name: "Test",
            system,
        }))
    }

    fn labels(steps: &[Step]) -> Vec<&'static str> {
        steps
            .iter()
            .map(|step| match step {
                Step::Components => COMPONENTS,
                Step::System(s) => s.system.label,
            })
            .collect()
    }

    #[test]
    fn systems_run_in_their_labelled_order_around_the_components() {
        let systems = [
            registration(System::new("p.physics.step", noop).after(&[COMPONENTS])),
            registration(
                System::new("p.character.move", noop)
                    .after(&["p.input.devices"])
                    .before(&["p.physics.step"]),
            ),
            registration(System::new("p.input.devices", noop).before(&[COMPONENTS])),
        ];
        let (steps, problems) = order(&systems);
        assert!(problems.is_empty(), "{problems:?}");
        let order = labels(&steps);
        let at = |label| order.iter().position(|l| *l == label).unwrap();
        assert!(at("p.input.devices") < at("p.character.move"));
        assert!(at("p.character.move") < at("p.physics.step"));
        assert!(at("p.input.devices") < at(COMPONENTS));
        assert!(at(COMPONENTS) < at("p.physics.step"));
    }

    #[test]
    fn an_absent_label_orders_nothing_and_a_cycle_is_reported() {
        let systems = [
            registration(
                System::new("p.a", noop)
                    .after(&["p.b"])
                    .before(&["p.nowhere"]),
            ),
            registration(System::new("p.b", noop).after(&["p.a"])),
        ];
        let (steps, problems) = order(&systems);
        assert_eq!(steps.len(), 3, "both systems and the components run");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("cycle"), "{problems:?}");
        assert_eq!(
            unknown_labels(&systems),
            ["system p.a orders against `p.nowhere`"]
        );
    }
}
