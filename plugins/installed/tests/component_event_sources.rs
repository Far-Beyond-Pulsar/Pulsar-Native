//! The events a component offers because of the assets its properties name,
//! with the real animation and input plugins linked: the asset slots come from
//! reflection (an `AssetRef<K>` property) or from a plugin's path-slot
//! registration, and the kind's source answers (`engine_backend::asset_event_sources`).

use std::path::Path;

use engine_backend::{asset_slots, component_events, AssetSlot, ComponentData, EventCache};
use installed_plugins::input_plugin::model::{
    ActionAsset, ActionRef, Asset, Entry, MappingAsset, ValueType,
};
use serde_json::json;

fn link_plugins() {
    // linking the plugins registers their classes, sources and slots
    let _ = installed_plugins::skeletal_animation_plugin::animation_component::AnimationComponent::default();
    let _ = installed_plugins::input_plugin::PlayerInputComponent::default();
}

fn slot(class: &str, property: &str) -> Option<AssetSlot> {
    asset_slots(class)
        .iter()
        .copied()
        .find(|slot| slot.property == property)
}

#[test]
fn an_asset_ref_property_is_a_slot_by_its_type_alone() {
    link_plugins();
    let graph = slot("AnimationComponent", "animation_graph")
        .expect("the animator's `Option<AssetRef<AnimationGraphAssetKind>>` is a slot");
    assert_eq!(graph.kind, "anim_graph");
    assert!(
        slot("AnimationComponent", "playing").is_none(),
        "a plain property is not"
    );
}

#[test]
fn a_path_property_is_a_slot_when_its_plugin_says_so() {
    link_plugins();
    let mapping = slot("PlayerInputComponent", "mapping")
        .expect("the player input's `mapping` path is registered as a slot");
    assert_eq!(mapping.kind, "inputmap");
    assert!(slot("PlayerInputComponent", "priority").is_none());
}

fn project(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "component_event_sources_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("input")).unwrap();
    root
}

fn write_mapping(root: &Path) -> String {
    let mut jump = ActionAsset::new();
    jump.identifier = "Jump".into();
    jump.value_type = ValueType::Bool;
    std::fs::write(root.join("input/Jump.inputaction"), Asset::to_text(&jump)).unwrap();
    let mut look = ActionAsset::new();
    look.identifier = "Look".into();
    look.value_type = ValueType::Axis2D;
    std::fs::write(root.join("input/Look.inputaction"), Asset::to_text(&look)).unwrap();

    let mut mapping = MappingAsset::new();
    for (index, (action, path)) in [
        (&jump, "input/Jump.inputaction"),
        (&look, "input/Look.inputaction"),
    ]
    .into_iter()
    .enumerate()
    {
        mapping.categories[0].entries.push(Entry {
            id: index as u32 + 1,
            key: "Keyboard.Space".into(),
            action: Some(ActionRef::new(action.id.clone(), path)),
            enabled: true,
            ..Entry::default()
        });
    }
    std::fs::write(root.join("input/Game.inputmap"), Asset::to_text(&mapping)).unwrap();
    "input/Game.inputmap".to_owned()
}

#[test]
fn a_player_input_component_offers_its_mappings_actions_as_one_family() {
    link_plugins();
    let root = project("family");
    let mapping = write_mapping(&root);
    let data = json!({ "enabled": true, "mapping": mapping, "priority": 0 });
    let components = [ComponentData {
        slot_id: "slot-1",
        class_name: "PlayerInputComponent",
        enabled: true,
        data: &data,
    }];

    let sets = component_events(&root, &components);
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].used.slot_id, "slot-1");
    assert_eq!(sets[0].used.property, "mapping");
    let families = &sets[0].catalog.families;
    assert_eq!(families.len(), 1, "one node however many actions");
    assert_eq!(families[0].id, "input.action");
    let actions: Vec<_> = families[0]
        .param("action")
        .unwrap()
        .options
        .iter()
        .map(|o| o.label.as_str())
        .collect();
    assert_eq!(actions, ["Jump", "Look"]);

    // The second call is the cached catalog: the same `Arc`.
    let again = component_events(&root, &components);
    assert!(std::sync::Arc::ptr_eq(&sets[0].catalog, &again[0].catalog));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_disabled_component_and_an_unset_property_offer_nothing() {
    link_plugins();
    let root = project("none");
    let mapping = write_mapping(&root);
    let on = json!({ "mapping": mapping });
    let unset = json!({ "mapping": "" });
    let sets = component_events(
        &root,
        &[
            ComponentData {
                slot_id: "off",
                class_name: "PlayerInputComponent",
                enabled: false,
                data: &on,
            },
            ComponentData {
                slot_id: "unset",
                class_name: "PlayerInputComponent",
                enabled: true,
                data: &unset,
            },
            ComponentData {
                slot_id: "other",
                class_name: "NoSuchComponent",
                enabled: true,
                data: &on,
            },
        ],
    );
    assert!(sets.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

/// A cache hit does no file or index work. The bound is a loose guard against a
/// regression to reading on every call (a read of this mapping takes far
/// longer), not a benchmark: it holds in a debug build on a busy machine.
#[test]
fn a_cache_hit_is_cheap() {
    link_plugins();
    let root = project("hit");
    let mapping = write_mapping(&root);
    let data = json!({ "mapping": mapping });
    let components = [ComponentData {
        slot_id: "s",
        class_name: "PlayerInputComponent",
        enabled: true,
        data: &data,
    }];
    let _warm = component_events(&root, &components);
    let before = EventCache::global().stats();
    let started = std::time::Instant::now();
    const CALLS: u32 = 2_000;
    for _ in 0..CALLS {
        std::hint::black_box(component_events(&root, &components));
    }
    let per_call = started.elapsed() / CALLS;
    let after = EventCache::global().stats();
    assert_eq!(
        after.misses, before.misses,
        "every call after the first is a hit"
    );
    assert!(
        per_call < std::time::Duration::from_micros(500),
        "a cached lookup took {per_call:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

mod opt_out {
    use super::*;
    use engine_class_derive::engine_class;
    use pulsar_reflection::{AssetRef, InputMapAssetKind};

    /// Two asset references; one opts out of raising events.
    #[engine_class(category = "Test", clone, debug, serialize, deserialize)]
    #[serde(default)]
    pub struct EventsOptOutProbe {
        #[property]
        pub raises: Option<AssetRef<InputMapAssetKind>>,
        #[property(events = false)]
        pub quiet: Option<AssetRef<InputMapAssetKind>>,
    }

    impl Default for EventsOptOutProbe {
        fn default() -> Self {
            Self {
                raises: None,
                quiet: None,
            }
        }
    }

    #[test]
    fn a_property_can_opt_out_of_raising_events() {
        let _ = EventsOptOutProbe::default();
        let raises = slot("EventsOptOutProbe", "raises").expect("an asset reference is a slot");
        assert_eq!(raises.kind, "inputmap");
        assert!(
            slot("EventsOptOutProbe", "quiet").is_none(),
            "`#[property(events = false)]` takes it out"
        );
        assert!(pulsar_reflection::property_declares_events(
            "EventsOptOutProbe",
            "raises"
        ));
        assert!(!pulsar_reflection::property_declares_events(
            "EventsOptOutProbe",
            "quiet"
        ));
    }
}

mod value_dependent {
    use super::*;
    use engine_class_derive::{engine_class, world_component};
    use pulsar_events::gamma::{EventDescriptor, FieldType};
    use pulsar_world_registry::DynamicEvent;

    /// What it raises depends on its own values, not on an asset it names.
    #[engine_class(category = "Test", clone, debug, serialize, deserialize)]
    #[serde(default)]
    pub struct ModeProbe {
        #[property]
        pub armed: bool,
        #[property]
        pub channels: i32,
    }

    impl Default for ModeProbe {
        fn default() -> Self {
            Self {
                armed: false,
                channels: 1,
            }
        }
    }

    #[world_component]
    impl ModeProbe {
        #[bp_event_source]
        fn events(&self) -> Vec<DynamicEvent> {
            let mut out = Vec::new();
            if self.armed {
                for channel in 0..self.channels.max(0) {
                    out.push(DynamicEvent {
                        label: format!("Channel {channel} fired"),
                        group: "Channels".into(),
                        descriptor: EventDescriptor::dynamic_in(
                            "probe",
                            format!("channel_{channel}"),
                            [("entity".to_owned(), FieldType::U64)],
                        )
                        .unwrap(),
                    });
                }
            }
            out
        }
    }

    fn events_of(data: &serde_json::Value) -> Vec<String> {
        let root = std::env::temp_dir().join("value_dependent_events");
        component_events(
            &root,
            &[ComponentData {
                slot_id: "s",
                class_name: "ModeProbe",
                enabled: true,
                data,
            }],
        )
        .iter()
        .flat_map(|set| set.catalog.events.iter().map(|e| e.label.clone()))
        .collect()
    }

    #[test]
    fn a_components_values_decide_which_events_it_offers() {
        let _ = ModeProbe::default();
        assert!(events_of(&json!({ "armed": false, "channels": 3 })).is_empty());
        assert_eq!(
            events_of(&json!({ "armed": true, "channels": 2 })),
            ["Channel 0 fired", "Channel 1 fired"]
        );
        assert_eq!(
            events_of(&json!({ "armed": true, "channels": 3 })).len(),
            3,
            "the events follow the value"
        );
    }

    #[test]
    fn the_same_values_are_answered_from_the_cache() {
        let _ = ModeProbe::default();
        let cache = EventCache::new();
        let data = json!({ "armed": true, "channels": 4 });
        let root = std::env::temp_dir().join("value_dependent_cache");
        let ask = || {
            engine_backend::component_events_with(
                &cache,
                &root,
                &[ComponentData {
                    slot_id: "s",
                    class_name: "ModeProbe",
                    enabled: true,
                    data: &data,
                }],
            )
        };
        let first = ask();
        let second = ask();
        assert!(std::sync::Arc::ptr_eq(
            &first[0].catalog,
            &second[0].catalog
        ));
        assert_eq!(cache.stats().misses, 1, "built once");
        assert_eq!(cache.stats().hits, 1);
    }
}
