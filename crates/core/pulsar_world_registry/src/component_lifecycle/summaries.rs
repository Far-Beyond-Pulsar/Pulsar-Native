//! The catalog source for component lifecycle tables.

fn all_actor_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    crate::runtime::all_actors()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

fn active_actor_summaries() -> Vec<pulsar_plugins::Summary> {
    use pulsar_plugins::Contribution as _;
    crate::runtime::actors()
        .iter()
        .map(|registration| registration.summary())
        .collect()
}

// Tables are gated with their class (across plugin libraries, like the
// classes and systems), so their catalog source reads the registry.
inventory::submit! {
    pulsar_plugins::ContributionSource {
        category: "component.lifecycle",
        all: all_actor_summaries,
        active: active_actor_summaries,
        // Tables come only from a class's spec, through a plugin's build.
        linked: Vec::new,
    }
}
