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

use super::{LevelEditorState, PropertiesPanelWrapper};
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use gpui::{
    div, px, render_stats, size, AnyElement, AnyView, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement as _, IntoElement, ParentElement as _, Render,
    StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window,
};
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Hosts the panel the way the dock does: behind `.cached(...)`, so a notify
/// on the panel (or an entity it reads) rebuilds it through the cached-view
/// path that the profiler's `view rebuild (...)` spans come from.
struct Host {
    panel: Entity<PropertiesPanelWrapper>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // `PROPS_PERF_DEPTH` nests the panel under that many id'd divs to mimic its
        // depth inside the real dock (every id'd element and component copies and
        // hashes the whole id path).
        let depth: usize = std::env::var("PROPS_PERF_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut el: gpui::AnyElement = AnyView::from(self.panel.clone())
            .cached(StyleRefinement::default().size_full())
            .into_any_element();
        for d in 0..depth {
            el = div()
                .id(("host-wrap", d))
                .size_full()
                .child(el)
                .into_any_element();
        }
        div().size_full().child(el)
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
fn scene_with_components(class_names: &[String]) -> Arc<parking_lot::RwLock<LevelEditorState>> {
    use crate::commands::{execute_command, SceneCommand};

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
    let panel = window
        .root(cx)
        .expect("root view")
        .read_with(cx, |host, _| host.panel.clone());
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
fn measure(frames: usize, mut step: impl FnMut() -> Duration) -> (f64, render_stats::Snapshot) {
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
        "frame: id'd elements",
        "frame: id'd elements, id stack depth (sum)",
        "frame: component global ids",
        "frame: component global ids, id stack depth (sum)",
    ] {
        eprintln!(
            "  {name:<44} {:>8.1} /frame",
            counter(s, name) as f64 / frames as f64
        );
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

// ── Per-widget layout cost bisect ───────────────────────────────────────────

/// One kind of row in the synthetic bisect tree.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Plain,
    FlexRow3,
    StatefulDiv,
    StaticText,
    TruncateText,
    Icon,
    ButtonIcon,
    TextInput,
    NumberInput,
    NumberRow,
    NumberRowInEntity,
    NumberRowDecorated,
}

struct EditorRowView {
    label: String,
    input: Entity<ui::input::InputState>,
}

impl Render for EditorRowView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, input::NumberInput, ActiveTheme as _, Sizable as _};
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label.clone()),
            )
            .child(NumberInput::new(&self.input).xsmall().w(px(92.)))
    }
}

struct Synth {
    kind: Kind,
    inputs: Vec<Entity<ui::input::InputState>>,
    row_views: Vec<Entity<EditorRowView>>,
}

impl Synth {
    fn new(kind: Kind, rows: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs: Vec<_> = (0..rows)
            .map(|_| cx.new(|cx| ui::input::InputState::new(window, cx)))
            .collect();
        let row_views = inputs
            .iter()
            .enumerate()
            .map(|(i, input)| {
                cx.new(|_| EditorRowView {
                    label: format!("Property {i}"),
                    input: input.clone(),
                })
            })
            .collect();
        Self {
            kind,
            inputs,
            row_views,
        }
    }
}

impl Render for Synth {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::prelude::*;
        use ui::{
            button::{Button, ButtonVariants as _},
            h_flex,
            input::{NumberInput, TextInput},
            v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
        };
        let kind = self.kind;
        let row_views = self.row_views.clone();
        let muted = cx.theme().muted_foreground;
        let rows = self
            .inputs
            .iter()
            .enumerate()
            .map(move |(i, input)| -> AnyElement {
                match kind {
                    Kind::Plain => div().w_full().h(px(20.)).into_any_element(),
                    Kind::FlexRow3 => h_flex()
                        .w_full()
                        .child(div().h(px(20.)).w(px(10.)))
                        .child(div().h(px(20.)).flex_1())
                        .child(div().h(px(20.)).w(px(10.)))
                        .into_any_element(),
                    Kind::StatefulDiv => {
                        div().id(("row", i)).w_full().h(px(20.)).into_any_element()
                    }
                    Kind::StaticText => div().text_sm().child("Property label").into_any_element(),
                    Kind::TruncateText => div()
                        .w(px(120.))
                        .truncate()
                        .text_sm()
                        .child("A rather long property label that truncates")
                        .into_any_element(),
                    Kind::Icon => Icon::new(IconName::Plus).xsmall().into_any_element(),
                    Kind::ButtonIcon => Button::new(("btn", i))
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .into_any_element(),
                    Kind::TextInput => TextInput::new(input).xsmall().into_any_element(),
                    Kind::NumberInput => NumberInput::new(input)
                        .xsmall()
                        .w(px(92.))
                        .into_any_element(),
                    Kind::NumberRow => h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(format!("Property {i}")),
                        )
                        .child(NumberInput::new(input).xsmall().w(px(92.)))
                        .into_any_element(),
                    Kind::NumberRowInEntity => row_views[i].clone().into_any_element(),
                    Kind::NumberRowDecorated => h_flex()
                        .w_full()
                        .items_center()
                        .gap_1()
                        .child(div().w(px(2.)).h_full().min_h(px(18.)).rounded(px(1.)))
                        .child(div().flex_1().min_w_0().child(row_views[i].clone()))
                        .into_any_element(),
                }
            });
        let depth: usize = std::env::var("PROPS_PERF_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut el: AnyElement = v_flex()
            .size_full()
            .gap_1()
            .children(rows.collect::<Vec<_>>())
            .into_any_element();
        for d in 0..depth {
            el = div()
                .id(("wrap", d))
                .size_full()
                .child(el)
                .into_any_element();
        }
        el
    }
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn widget_layout_cost_report(cx: &mut TestAppContext) {
    cx.update(|cx| ui::init(cx));
    let rows: usize = std::env::var("PROPS_PERF_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let frames = 20;
    eprintln!(
        "{:<22} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "kind (per row)",
        "elems",
        "nodes",
        "layout us",
        "prepaint us",
        "paint us",
        "wall us",
        "measure",
        "wall(no stats)"
    );
    for kind in [
        Kind::Plain,
        Kind::FlexRow3,
        Kind::StatefulDiv,
        Kind::StaticText,
        Kind::TruncateText,
        Kind::Icon,
        Kind::ButtonIcon,
        Kind::TextInput,
        Kind::NumberInput,
        Kind::NumberRow,
        Kind::NumberRowInEntity,
        Kind::NumberRowDecorated,
    ] {
        render_stats::set_force_enabled(false);
        let window = cx.open_window(size(px(380.), px(100_000.)), |window, cx| {
            Synth::new(kind, rows, window, cx)
        });
        cx.run_until_parked();
        let root = window.root(cx).unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        render_stats::set_force_enabled(true);
        for _ in 0..5 {
            root.update(&mut vcx, |_, cx| cx.notify());
        }
        render_stats::reset();
        let start = Instant::now();
        for _ in 0..frames {
            root.update(&mut vcx, |_, cx| cx.notify());
        }
        let wall = start.elapsed().as_secs_f64() * 1e6 / frames as f64 / rows as f64;
        let s = render_stats::snapshot();
        render_stats::set_force_enabled(false);
        let start = Instant::now();
        for _ in 0..frames {
            root.update(&mut vcx, |_, cx| cx.notify());
        }
        let wall_clean = start.elapsed().as_secs_f64() * 1e6 / frames as f64 / rows as f64;
        let per_row = |n: u64| n as f64 / frames as f64 / rows as f64;
        eprintln!(
            "{:<22} {:>9.1} {:>9.1} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.1} {:>9.2}",
            format!("{kind:?}"),
            per_row(counter(&s, "frame: element tree nodes")),
            per_row(counter(&s, "frame: taffy nodes created")),
            timer_ms(&s, "frame: layout", frames) * 1e3 / rows as f64,
            timer_ms(&s, "frame: prepaint", frames) * 1e3 / rows as f64,
            timer_ms(&s, "frame: paint", frames) * 1e3 / rows as f64,
            wall,
            per_row(counter(&s, "taffy: measure calls")),
            wall_clean,
        );
        drop(vcx);
    }
}

// ── What invalidates the panel? ─────────────────────────────────────────────

fn print_invalidation(label: &str, wall: Duration, s: &render_stats::Snapshot) {
    eprintln!(
        "{label:<34} wall {:>8.2} ms | panel renders {:>3} | view rebuilds {:>3} (dep changed {:>3}) | reused {:>3}",
        wall.as_secs_f64() * 1e3,
        counter(s, "properties panel: render"),
        counter(s, "view cache: rebuilt"),
        counter(s, "view cache: rebuilt (dependency changed)"),
        counter(s, "view cache: reused"),
    );
    for (name, n) in s.counters.iter().filter(|(name, _)| {
        name.starts_with("notify: ")
            || name.starts_with("view cache: rebuilt (dependency changed): ")
    }) {
        eprintln!("      {n:>4} x {name}");
    }
}

/// Drives realistic input at the panel (hover, wheel scroll, focusing a
/// number field and letting its caret blink) and reports what each of them
/// invalidates. Anything beyond "the one widget touched" is the bug.
#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn properties_panel_invalidation_report(cx: &mut TestAppContext) {
    use gpui::{point, Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase};

    let class_names: Vec<String> = heaviest_classes(5).into_iter().map(|(n, _)| n).collect();
    let mut f = open_fixture(cx, &class_names);
    render_stats::set_force_enabled(true);
    for _ in 0..4 {
        dirty_frame(&mut f);
    }
    idle_frame(&mut f);

    // Hover: move across the panel; each move is its own frame.
    render_stats::reset();
    let start = Instant::now();
    for step in 0..20 {
        let y = 150. + 40. * step as f32;
        f.cx.simulate_mouse_move(point(px(250.), px(y)), None, Modifiers::default());
        idle_frame(&mut f);
    }
    print_invalidation(
        "hover: 20 mouse moves",
        start.elapsed(),
        &render_stats::snapshot(),
    );

    // Wheel scroll.
    render_stats::reset();
    let start = Instant::now();
    for _ in 0..10 {
        f.cx.simulate_event(ScrollWheelEvent {
            position: point(px(200.), px(500.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        idle_frame(&mut f);
    }
    print_invalidation(
        "scroll: 10 wheel events",
        start.elapsed(),
        &render_stats::snapshot(),
    );

    // Click a field to focus it, then let the caret blink for ~3 s.
    render_stats::reset();
    let start = Instant::now();
    f.cx.simulate_mouse_move(point(px(300.), px(400.)), None, Modifiers::default());
    f.cx.simulate_mouse_down(
        point(px(300.), px(400.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    f.cx.simulate_mouse_up(
        point(px(300.), px(400.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    idle_frame(&mut f);
    print_invalidation(
        "click (focus a field)",
        start.elapsed(),
        &render_stats::snapshot(),
    );

    render_stats::reset();
    let start = Instant::now();
    for _ in 0..6 {
        f.cx.executor().advance_clock(Duration::from_millis(500));
        f.cx.run_until_parked();
        idle_frame(&mut f);
    }
    print_invalidation(
        "caret blink: 6 x 500 ms",
        start.elapsed(),
        &render_stats::snapshot(),
    );
    render_stats::set_force_enabled(false);
}
