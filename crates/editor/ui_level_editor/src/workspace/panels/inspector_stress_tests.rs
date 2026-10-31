//! Stress measurement of the shared [`Inspector`] with non-uniform content.
//!
//! A synthetic host view stands in for a properties panel: `cards` cards of
//! `props` rows each, row heights cycling through 18..=140 px (the way real
//! editors differ: a toggle versus a colour picker versus a vec3).  Reports
//! the cost of the operations a user does: first draw, idle draw, small and
//! far scrolls, collapse, search keystroke.
//!
//! ```text
//! cargo test -p ui_level_editor --lib inspector_stress -- --ignored --nocapture --test-threads=1
//! ```

use gpui::{
    div, px, size, AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
};
use std::time::Instant;
use super::perf_tests::RENDER_LOCK;
use ui_common::inspector_model::{CardDesc, PropDesc};
use ui_common::{Inspector, InspectorRow};

struct Host {
    inspector: Inspector<Host>,
    rows_built: usize,
}

impl Host {
    fn row(&mut self, row: InspectorRow, _w: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        match row {
            InspectorRow::Property { card, prop } => {
                self.rows_built += 1;
                let h = 18.0 + ((prop * 37 + card * 11) % 123) as f32;
                let el = div().id(("row", card * 100_000 + prop)).w_full().h(px(h)).into_any_element();
                self.inspector.wrap_row(card, prop, el, _cx)
            }
            _ => div().into_any_element(),
        }
    }
}

impl Render for Host {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.inspector.render(Host::row, cx))
    }
}

fn cards(cards: usize, props: usize) -> Vec<CardDesc> {
    (0..cards)
        .map(|c| CardDesc {
            key: format!("Card{c}#0"),
            title: format!("Card{c}"),
            props: (0..props)
                .map(|p| PropDesc {
                    name: format!("p{p}"),
                    display_name: format!("Prop {p}"),
                    category: (p % 7 != 0).then(|| format!("Cat{}", p % 5)),
                    category_color: None,
                    category_default_collapsed: false,
                    category_order: Some(p % 5),
                    extra_rows: 0,
                })
                .collect(),
            custom: false,
            note: None,
        })
        .collect()
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

fn run(cx: &mut TestAppContext, n_cards: usize, n_props: usize) {
    cx.update(|cx| ui::init(cx));
    let window = cx.open_window(size(px(380.), px(1000.)), |window, cx| {
        let host = cx.new(|cx| Host {
            inspector: Inspector::new(
                "stress",
                "Search",
                0,
                |h: &mut Host| &mut h.inspector,
                window,
                cx,
            ),
            rows_built: 0,
        });
        Root { host }
    });
    let host = window.root(cx).unwrap().read_with(cx, |r, _| r.host.clone());
    let mut vcx = VisualTestContext::from_window(window.into(), cx);

    let t = Instant::now();
    host.update(&mut vcx, |h, cx| {
        h.inspector.set_cards(cards(n_cards, n_props));
        cx.notify();
    });
    let set_cards = ms(t);

    let draw = |vcx: &mut VisualTestContext| {
        let t = Instant::now();
        vcx.update(|window, cx| {
            window.draw(cx).clear();
        });
        ms(t)
    };
    let first = draw(&mut vcx);
    let idle = draw(&mut vcx);
    host.update(&mut vcx, |h, cx| {
        h.rows_built = 0;
        h.inspector.scroll_by(px(300.));
        cx.notify();
    });
    let small = draw(&mut vcx);
    let built_small = host.read_with(&vcx, |h, _| h.rows_built);
    host.update(&mut vcx, |h, cx| {
        h.inspector.scroll_by(px(5_000_000.));
        cx.notify();
    });
    let far = draw(&mut vcx);
    host.update(&mut vcx, |h, cx| {
        h.inspector.scroll_by(px(-4_000_000.));
        cx.notify();
    });
    let back = draw(&mut vcx);
    eprintln!(
        "cards={n_cards} props={n_props}: set_cards {set_cards:.1}ms | draw first {first:.1} idle {idle:.1} \
         scroll300 {small:.1} (rows built {built_small}) far {far:.1} back {back:.1}"
    );
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn inspector_stress_one_large_card(cx: &mut TestAppContext) {
    run(cx, 1, 20_000);
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn inspector_stress_many_cards(cx: &mut TestAppContext) {
    run(cx, 300, 100);
}

struct Root {
    host: gpui::Entity<Host>,
}

impl Render for Root {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.host.clone())
    }
}

// ── Composite (array / struct) editor ───────────────────────────────────────
//
// The array's rows join the inspector's ONE list below the property's own row
// (`VirtualRows`): no sub-scroll area, no paging.

use pulsar_reflection::Reflectable;
use std::sync::{Arc, Mutex};
use ui_common::composite_editor::CompositeEditor;
use ui_common::PropertyStateManager;

struct ArrayHost {
    inspector: Inspector<ArrayHost>,
    state: PropertyStateManager,
    value: Box<dyn std::any::Any>,
    ty: &'static pulsar_reflection::RuntimeTypeInfo,
    writes: Arc<Mutex<usize>>,
}

impl ArrayHost {
    fn row(&mut self, row: InspectorRow, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match row {
            InspectorRow::Property { card, prop } => {
                let writes = self.writes.clone();
                let write_back = Arc::new(
                    move |_v: Box<dyn std::any::Any + Send>, _w: &mut Window, _c: &mut gpui::App| {
                        *writes.lock().unwrap() += 1;
                    },
                );
                let el = ui_common::render_property_row_runtime(
                    &mut self.state,
                    "t",
                    "Comp#0",
                    "Comp",
                    "Items",
                    "items",
                    self.ty,
                    self.value.as_ref(),
                    write_back,
                    window,
                    cx,
                );
                self.inspector.wrap_row(card, prop, el, cx)
            }
            InspectorRow::Child { card, prop, ix } => {
                let Some(rows) = self.state.virtual_rows("Comp#0", "items") else {
                    return div().into_any_element();
                };
                let el = rows.render_row(ix, window, cx);
                self.inspector.wrap_row(card, prop, el, cx)
            }
            _ => div().into_any_element(),
        }
    }
}

impl Render for ArrayHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for (key, prop, count) in self.state.virtual_row_counts(cx) {
            self.inspector.set_extra_rows(&key, &prop, count);
        }
        div().size_full().child(self.inspector.render(ArrayHost::row, cx))
    }
}

struct ArrayRoot {
    host: gpui::Entity<ArrayHost>,
}

impl Render for ArrayRoot {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.host.clone())
    }
}

fn array_host(
    cx: &mut TestAppContext,
    value: Vec<Vec<i32>>,
) -> (VisualTestContext, gpui::Entity<ArrayHost>) {
    typed_host(cx, <Vec<Vec<i32>> as Reflectable>::type_info(), Box::new(value))
}

fn typed_host(
    cx: &mut TestAppContext,
    ty: &'static pulsar_reflection::RuntimeTypeInfo,
    value: Box<dyn std::any::Any>,
) -> (VisualTestContext, gpui::Entity<ArrayHost>) {
    cx.update(|cx| ui::init(cx));
    let window = cx.open_window(size(px(380.), px(1000.)), |window, cx| {
        let host = cx.new(|cx| {
            let mut state = PropertyStateManager::new();
            let this = cx.weak_entity();
            state.set_wake(Arc::new(move |cx: &mut gpui::App| {
                let _ = this.update(cx, |_, cx| cx.notify());
            }));
            let mut inspector = Inspector::new(
                "arr",
                "Search",
                0,
                |h: &mut ArrayHost| &mut h.inspector,
                window,
                cx,
            );
            inspector.set_cards(vec![CardDesc {
                key: "Comp#0".into(),
                title: "Comp".into(),
                props: vec![PropDesc {
                    name: "items".into(),
                    display_name: "Items".into(),
                    category: None,
                    category_color: None,
                    category_default_collapsed: false,
                    category_order: None,
                    extra_rows: 0,
                }],
                custom: false,
                note: None,
            }]);
            ArrayHost {
                inspector,
                state,
                value,
                ty,
                writes: Arc::new(Mutex::new(0)),
            }
        });
        ArrayRoot { host }
    });
    let host = window.root(cx).unwrap().read_with(cx, |r, _| r.host.clone());
    (VisualTestContext::from_window(window.into(), cx), host)
}

fn draw(vcx: &mut VisualTestContext) -> f64 {
    vcx.run_until_parked();
    let t = Instant::now();
    vcx.update(|w, c| {
        w.draw(c).clear();
    });
    ms(t)
}

fn composite_of(host: &gpui::Entity<ArrayHost>, vcx: &VisualTestContext) -> gpui::Entity<CompositeEditor> {
    host.read_with(vcx, |h, _| h.state.editor_view("Comp#0", "items"))
        .expect("composite editor built")
        .downcast::<CompositeEditor>()
        .expect("composite editor")
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn composite_editor_huge_array(cx: &mut TestAppContext) {
    // 2,000 arrays of 500 ints = 1,000,000 leaves.
    let value = (0..2_000).map(|i| (0..500).map(|j| i * 500 + j).collect()).collect();
    let (mut vcx, host) = array_host(cx, value);

    let first = draw(&mut vcx);
    let editor = composite_of(&host, &vcx);
    editor.update(&mut vcx, |e, cx| e.set_open(true, cx));
    let open = draw(&mut vcx);
    let open_items = host.read_with(&vcx, |h, _| h.inspector.item_count());
    editor.update(&mut vcx, |e, cx| e.expand_top_level(cx));
    let t = Instant::now();
    let expand = draw(&mut vcx);
    let expand_total = ms(t);
    let items = host.read_with(&vcx, |h, _| h.inspector.item_count());

    let mut worst: f64 = 0.0;
    let mut total = 0.0;
    for _ in 0..200 {
        host.update(&mut vcx, |h, cx| {
            h.inspector.scroll_by(px(97.));
            cx.notify();
        });
        let d = draw(&mut vcx);
        worst = worst.max(d);
        total += d;
    }
    eprintln!(
        "composite 1M leaves in the panel list: first {first:.1}ms | open {open:.1}ms ({open_items} items) \
         | expand all {expand:.1}ms (+relayout, {expand_total:.1}ms; {items} items) | scroll mean {:.2}ms worst {worst:.2}ms",
        total / 200.0
    );
    assert!(items > 1_000_000, "every leaf must be a row of the panel list");
}

#[gpui::test]
fn composite_editor_edit_writes_the_rebuilt_value_back(cx: &mut TestAppContext) {
    let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut vcx, host) = array_host(cx, vec![vec![1, 2, 3], vec![4, 5]]);
    draw(&mut vcx);
    let editor = composite_of(&host, &vcx);

    editor.update_in(&mut vcx, |e, window, cx| {
        e.edit_element(&[1, 0], serde_json::json!(40), window, cx);
        // An unchanged value writes nothing.
        e.edit_element(&[1, 0], serde_json::json!(40), window, cx);
    });
    assert_eq!(*host.read_with(&vcx, |h, _| h.writes.clone()).lock().unwrap(), 1);
}

#[gpui::test]
fn opening_a_composite_adds_its_rows_to_the_panel_list(cx: &mut TestAppContext) {
    let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut vcx, host) = array_host(cx, vec![vec![1, 2, 3], vec![4, 5]]);
    draw(&mut vcx);
    assert_eq!(host.read_with(&vcx, |h, _| h.inspector.item_count()), 2); // card + header

    composite_of(&host, &vcx).update(&mut vcx, |e, cx| e.set_open(true, cx));
    draw(&mut vcx);
    // card header, the property row, and its two top-level rows
    assert_eq!(host.read_with(&vcx, |h, _| h.inspector.item_count()), 4);

    composite_of(&host, &vcx).update(&mut vcx, |e, cx| e.expand_top_level(cx));
    draw(&mut vcx);
    assert_eq!(host.read_with(&vcx, |h, _| h.inspector.item_count()), 4 + 5);

    composite_of(&host, &vcx).update(&mut vcx, |e, cx| e.set_open(false, cx));
    draw(&mut vcx);
    assert_eq!(host.read_with(&vcx, |h, _| h.inspector.item_count()), 2);
}

// Registry entry for the test's value type, the way `pulsar_world_registry`
// shims `Vec<i32>` (wrapper types are not registered by upstream).
fn ser_nested(value: &dyn std::any::Any) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    let typed = value.downcast_ref::<Vec<Vec<i32>>>().expect("Vec<Vec<i32>>");
    let mut serializer = pulsar_reflection::JsonSerializer::new();
    typed.serialize(&mut serializer)?;
    Ok(serializer.into_json())
}

fn de_nested(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<Box<dyn std::any::Any>> {
    let mut deserializer = pulsar_reflection::JsonDeserializer::new(value);
    Ok(Box::new(<Vec<Vec<i32>> as Reflectable>::deserialize(&mut deserializer)?))
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::RuntimeTypeRegistration {
        type_info: <Vec<Vec<i32>> as Reflectable>::type_info,
        serialize_json: ser_nested,
        deserialize_json: de_nested,
    }
}

// ── Helio's list editor (the voxel Features list) ───────────────────────────

use helio_component::components::{VoxelFeatureGrowth, VoxelFeatureSpawn};

/// The real `features` property of the voxel terrain: its type and a value.
fn features_host(
    cx: &mut TestAppContext,
    spawns: usize,
    growths: usize,
) -> (VisualTestContext, gpui::Entity<ArrayHost>) {
    let instance = pulsar_reflection::REGISTRY
        .create_instance("VoxelTerrainComponent")
        .expect("voxel terrain registered");
    let ty = instance
        .get_properties()
        .into_iter()
        .find(|p| p.name == "features")
        .expect("features property")
        .type_info;
    let value: Vec<VoxelFeatureSpawn> = (0..spawns)
        .map(|_| VoxelFeatureSpawn {
            growths: vec![VoxelFeatureGrowth::default(); growths],
            ..Default::default()
        })
        .collect();
    typed_host(cx, ty, Box::new(value))
}

fn list_editor_of(
    host: &gpui::Entity<ArrayHost>,
    vcx: &VisualTestContext,
) -> gpui::AnyView {
    host.read_with(vcx, |h, _| h.state.editor_view("Comp#0", "items"))
        .expect("list editor built")
}

#[gpui::test]
fn the_features_list_puts_its_cards_into_the_panel_list(cx: &mut TestAppContext) {
    let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut vcx, host) = features_host(cx, 3, 2);
    draw(&mut vcx);
    // card header + the property row + one header per feature
    assert_eq!(host.read_with(&vcx, |h, _| h.inspector.item_count()), 2 + 3);
    let _ = list_editor_of(&host, &vcx);
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn the_features_list_with_thousands_of_nested_items(cx: &mut TestAppContext) {
    let t = Instant::now();
    let growths: usize = std::env::var("GROWTHS").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let (mut vcx, host) = features_host(cx, 200, growths);
    let first = draw(&mut vcx);
    let setup = ms(t);
    let items = host.read_with(&vcx, |h, _| h.inspector.item_count());
    let mut worst: f64 = 0.0;
    let mut total = 0.0;
    for _ in 0..200 {
        host.update(&mut vcx, |h, cx| {
            h.inspector.scroll_by(px(97.));
            cx.notify();
        });
        let d = draw(&mut vcx);
        worst = worst.max(d);
        total += d;
    }
    eprintln!(
        "features 200 spawns x 50 growths: setup {setup:.1}ms first draw {first:.1}ms, {items} items, \
         scroll mean {:.2}ms worst {worst:.2}ms",
        total / 200.0
    );
}

// ── Terrain layers stack ────────────────────────────────────────────────────

use helio_component::components::VoxelTerrainStack;

fn stack_host(
    cx: &mut TestAppContext,
    stack: VoxelTerrainStack,
) -> (VisualTestContext, gpui::Entity<ArrayHost>) {
    let instance = pulsar_reflection::REGISTRY
        .create_instance("VoxelTerrainLayersComponent")
        .expect("layers component registered");
    let ty = instance
        .get_properties()
        .into_iter()
        .find(|p| p.type_info.type_id == std::any::TypeId::of::<VoxelTerrainStack>())
        .expect("a stack property")
        .type_info;
    typed_host(cx, ty, Box::new(stack))
}

#[gpui::test]
fn the_layers_stack_offers_headings_and_cards_not_one_giant_row(cx: &mut TestAppContext) {
    let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stack = VoxelTerrainStack::earth();
    let (layers, rules) = (stack.layers.len(), stack.rules.len());
    let (mut vcx, host) = stack_host(cx, stack);
    draw(&mut vcx);
    let items = host.read_with(&vcx, |h, _| h.inspector.item_count());
    // card header + property row + 5 headings + layer and rule cards + 2 adds
    // (+ the validation line when there is one)
    assert!(items >= 2 + 5 + layers + rules + 2, "{items} items");
    assert!(items <= 2 + 5 + layers + rules + 2 + 1, "{items} items");
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn the_layers_stack_with_hundreds_of_layers(cx: &mut TestAppContext) {
    let mut stack = VoxelTerrainStack::earth();
    let template = stack.layers.clone();
    while stack.layers.len() < 300 {
        stack.layers.extend(template.iter().cloned());
    }
    let (mut vcx, host) = stack_host(cx, stack);
    let first = draw(&mut vcx);
    let items = host.read_with(&vcx, |h, _| h.inspector.item_count());
    let mut worst: f64 = 0.0;
    let mut total = 0.0;
    for _ in 0..200 {
        host.update(&mut vcx, |h, cx| {
            h.inspector.scroll_by(px(97.));
            cx.notify();
        });
        let d = draw(&mut vcx);
        worst = worst.max(d);
        total += d;
    }
    eprintln!(
        "layers stack, 300+ layers: first draw {first:.1}ms, {items} items, scroll mean {:.2}ms worst {worst:.2}ms",
        total / 200.0
    );
}
