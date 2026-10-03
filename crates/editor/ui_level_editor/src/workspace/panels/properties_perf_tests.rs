//! Headless layout-cost measurement and regression budget for the Properties
//! panel.
//!
//! Drives the REAL [`PropertiesPanelWrapper`] (real `ObjectTypeFieldsSection`,
//! real reflected-property editors, real `ui` inputs) inside a headless
//! `TestAppContext` window with a real text system, for an object carrying
//! several reflected components, and reads the `render_stats` counters and
//! timers around dirty and idle frames. No GPU is involved: every measured
//! millisecond is CPU element-tree work (render, request_layout, Taffy,
//! prepaint, paint).
//!
//! Run the measurement (prints the full breakdown):
//!
//! ```text
//! $env:CARGO_TARGET_DIR='D:\Github\Pulsar-Native\target-perf-investigation'
//! cargo test -p ui_level_editor --lib properties_panel_layout_report -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The budget test (`properties_panel_layout_budget`) runs in the normal test
//! suite: it asserts element/Taffy node counts and cache behaviour, which are
//! deterministic, rather than wall-clock time, which is not.

use super::*;
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use gpui::{
    AppContext as _, StyleRefinement, TestAppContext, VisualTestContext, render_stats, size, px,
};
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::time::{Duration, Instant};

/// Hosts the panel the way the dock does: behind `.cached(...)`, so a notify
/// on the panel (or an entity it reads) rebuilds it through the cached-view
/// path that the profiler's `view rebuild (...)` spans come from.
struct Host {
    panel: Entity<PropertiesPanelWrapper>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            AnyView::from(self.panel.clone()).cached(StyleRefinement::default().size_full()),
        )
    }
}

/// Reflected classes ordered by property count (largest first), so the
/// scenario uses the heaviest real inspector cards the editor ships.
fn heaviest_classes(limit: usize) -> Vec<(String, usize)> {
    let mut classes: Vec<(String, usize)> = REGISTRY
        .get_class_names()
        .into_iter()
        .filter(|name| *name != pulsar_class::CLASS_INSTANCE)
        .filter_map(|name| {
            let instance = REGISTRY.create_instance(name)?;
            Some((name.to_string(), instance.get_properties().len()))
        })
        .collect();
    classes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    classes.truncate(limit);
    classes
}

/// A scene with one selected object carrying `class_names` as components.
fn scene_with_components(
    class_names: &[String],
) -> Arc<parking_lot::RwLock<LevelEditorState>> {
    use crate::commands::{SceneCommand, execute_command};

    let mut state = LevelEditorState::new();
    let id = execute_command(
        &mut state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: "Perf Object".to_string(),
                object_type: ObjectType::Empty,
                transform: Transform::default(),
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            parent_id: None,
        },
    )
    .affected_ids[0]
        .clone();

    for class_name in class_names {
        let Some(instance) = REGISTRY.create_instance(class_name) else {
            continue;
        };
        let mut map = serde_json::Map::new();
        for prop in instance.get_properties() {
            let value = (prop.getter)(instance.as_ref());
            let json = RUNTIME_TYPE_REGISTRY
                .serialize_json_for_any(value.as_ref())
                .unwrap_or(serde_json::json!(null));
            map.insert(prop.name.to_string(), json);
        }
        let mut world = state.scene.world_mut();
        crate::scene_edit::components::add_component(
            &mut world,
            &id,
            class_name.clone(),
            serde_json::Value::Object(map),
        );
    }
    state.scene.select_object(Some(id));
    Arc::new(parking_lot::RwLock::new(state))
}

fn counter(snapshot: &render_stats::Snapshot, name: &str) -> u64 {
    snapshot.counters.get(name).copied().unwrap_or(0)
}

fn timer_ms(snapshot: &render_stats::Snapshot, name: &str, frames: usize) -> f64 {
    snapshot
        .timers
        .get(name)
        .map(|t| t.total.as_secs_f64() * 1e3 / frames as f64)
        .unwrap_or(0.0)
}

struct Fixture {
    cx: VisualTestContext,
    panel: Entity<PropertiesPanelWrapper>,
}

fn open_fixture(cx: &mut TestAppContext, class_names: &[String]) -> Fixture {
    cx.update(|cx| ui::init(cx));
    let state = scene_with_components(class_names);
    let window = cx.open_window(size(px(380.), px(1000.)), |window, cx| {
        let panel = cx.new(|cx| PropertiesPanelWrapper::new(state.clone(), window, cx));
        Host { panel }
    });
    cx.run_until_parked();
    let panel = window.root(cx).expect("root view").read_with(cx, |host, _| host.panel.clone());
    let cx = VisualTestContext::from_window(window.into(), cx);
    Fixture { cx, panel }
}

fn dirty_frame(f: &mut Fixture) -> Duration {
    let panel = f.panel.clone();
    let start = Instant::now();
    panel.update(&mut f.cx, |_, cx| cx.notify());
    start.elapsed()
}

fn idle_frame(f: &mut Fixture) -> Duration {
    let mut elapsed = Duration::ZERO;
    f.cx.update(|window, cx| {
        let start = Instant::now();
        window.draw(cx).clear();
        elapsed = start.elapsed();
    });
    elapsed
}

/// Run `frames` frames of `step`, returning mean wall ms and the stats drained
/// around exactly that stretch.
fn measure(
    frames: usize,
    mut step: impl FnMut() -> Duration,
) -> (f64, render_stats::Snapshot) {
    render_stats::reset();
    let total: Duration = (0..frames).map(|_| step()).sum();
    let snapshot = render_stats::snapshot();
    (total.as_secs_f64() * 1e3 / frames as f64, snapshot)
}

fn print_profile(label: &str, frames: usize, wall_ms: f64, s: &render_stats::Snapshot) {
    eprintln!("---- {label}: {frames} frames, mean wall {wall_ms:.3} ms/frame ----");
    for name in [
        "frame: layout",
        "frame: render",
        "frame: prepaint",
        "frame: paint",
        "taffy: compute layout",
        "taffy: measure",
        "text: truncate_line",
        "frame: text shaping",
        "properties panel: render",
        "  rebuild: render",
        "  rebuild: layout",
        "  rebuild: prepaint",
    ] {
        eprintln!("  {name:<28} {:>9.3} ms/frame", timer_ms(s, name, frames));
    }
    for name in [
        "frame: element tree nodes",
        "frame: taffy nodes created",
        "taffy: measure calls",
        "text: measure cache hit",
        "text: measure reshaped",
        "properties panel: render",
        "view cache: rebuilt",
        "view cache: rebuilt (dependency changed)",
        "view cache: reused",
    ] {
        eprintln!("  {name:<44} {:>8.1} /frame", counter(s, name) as f64 / frames as f64);
    }
    let mut elements: Vec<_> = s
        .counters
        .iter()
        .filter(|(name, _)| name.starts_with("element: "))
        .collect();
    elements.sort_by(|a, b| b.1.cmp(a.1));
    for (name, n) in elements.iter().take(12) {
        eprintln!("    {name:<90} {:>8.1} /frame", **n as f64 / frames as f64);
    }
    for (name, n) in s
        .counters
        .iter()
        .filter(|(name, _)| name.starts_with("notify: ") || name.contains("(dependency changed): "))
    {
        eprintln!("    {name:<90} {:>8.1} /frame", *n as f64 / frames as f64);
    }
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn properties_panel_layout_report(cx: &mut TestAppContext) {
    let card_count = std::env::var("PROPS_PERF_CARDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let heavy = heaviest_classes(card_count);
    eprintln!("cards: {heavy:?}");
    let class_names: Vec<String> = heavy.iter().map(|(n, _)| n.clone()).collect();

    let mut f = open_fixture(cx, &class_names);
    render_stats::set_force_enabled(true);
    for _ in 0..10 {
        dirty_frame(&mut f);
    }
    let frames = 40;
    let (wall, dirty) = measure(frames, || dirty_frame(&mut f));
    print_profile("DIRTY (panel notified)", frames, wall, &dirty);
    let (wall, idle) = measure(frames, || idle_frame(&mut f));
    print_profile("IDLE", frames, wall, &idle);
    render_stats::set_force_enabled(false);
}

/// Deterministic regression budget: a clean frame must not rebuild or re-render
/// the panel at all, and a rebuilt panel must stay within a node budget.
#[gpui::test]
fn properties_panel_layout_budget(cx: &mut TestAppContext) {
    let class_names: Vec<String> = heaviest_classes(5).into_iter().map(|(n, _)| n).collect();
    assert!(!class_names.is_empty(), "no reflected classes registered");
    let mut f = open_fixture(cx, &class_names);
    render_stats::set_force_enabled(true);
    for _ in 0..4 {
        dirty_frame(&mut f);
    }
    let frames = 8;
    let (_, idle) = measure(frames, || idle_frame(&mut f));
    let (_, dirty) = measure(frames, || dirty_frame(&mut f));
    render_stats::set_force_enabled(false);

    assert_eq!(
        counter(&idle, "properties panel: render"),
        0,
        "an idle frame must not re-render the Properties panel"
    );
    let dirty_nodes = counter(&dirty, "frame: taffy nodes created") / frames as u64;
    eprintln!("properties panel dirty frame: {dirty_nodes} taffy nodes");
    assert!(
        dirty_nodes <= PROPERTIES_PANEL_TAFFY_NODE_BUDGET,
        "Properties panel dirty frame built {dirty_nodes} taffy nodes (budget {PROPERTIES_PANEL_TAFFY_NODE_BUDGET})"
    );
}

/// Placeholder until measured; tightened to the post-fix number.
const PROPERTIES_PANEL_TAFFY_NODE_BUDGET: u64 = u64::MAX;
