//! The voxel plugin's inspectors in the shared inspector (`ui_common`):
//! the terrain's features list (Helio's list editor over the real
//! `features` property) and the layers stack editor this crate registers
//! (headings and cards, not one giant row). Moved here from the level
//! editor's inspector tests: the stack editor is this crate's.
//!
//! ```text
//! cargo test -p voxel_plugin --test inspector -- --ignored --nocapture --test-threads=1
//! ```

use gpui::{
    AnyElement, AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use ui_common::PropertyStateManager;
use ui_common::inspector_model::{CardDesc, PropDesc};
use ui_common::{Inspector, InspectorRow};
// This crate's registrations (the stack editor) and the component's.
use voxel_plugin as _;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

struct ArrayHost {
    inspector: Inspector<ArrayHost>,
    state: PropertyStateManager,
    value: Box<dyn std::any::Any>,
    ty: &'static pulsar_reflection::RuntimeTypeInfo,
    writes: Arc<Mutex<usize>>,
}

impl ArrayHost {
    fn row(
        &mut self,
        row: InspectorRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match row {
            InspectorRow::Property { card, prop } => {
                let writes = self.writes.clone();
                let write_back = Arc::new(
                    move |_v: Box<dyn std::any::Any + Send>,
                          _w: &mut Window,
                          _c: &mut gpui::App| {
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
        div()
            .size_full()
            .child(self.inspector.render(ArrayHost::row, cx))
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
    let host = window
        .root(cx)
        .unwrap()
        .read_with(cx, |r, _| r.host.clone());
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

// ── Helio's list editor (the voxel Features list) ───────────────────────────

use voxel_component::{VoxelFeatureGrowth, VoxelFeatureSpawn};

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

fn list_editor_of(host: &gpui::Entity<ArrayHost>, vcx: &VisualTestContext) -> gpui::AnyView {
    host.read_with(vcx, |h, _| h.state.editor_view("Comp#0", "items"))
        .expect("list editor built")
}

#[gpui::test]
fn the_features_list_puts_its_cards_into_the_panel_list(cx: &mut TestAppContext) {
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
    let growths: usize = std::env::var("GROWTHS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
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

use voxel_component::VoxelTerrainStack;

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
    let stack = VoxelTerrainStack::earth();
    let (layers, rules) = (stack.layers.len(), stack.rules.len());
    let (mut vcx, host) = stack_host(cx, stack);
    draw(&mut vcx);
    let items = host.read_with(&vcx, |h, _| h.inspector.item_count());
    // card header + property row + 5 headings + layer and rule cards
    // (+ the validation line when there is one)
    assert!(items >= 2 + 5 + layers + rules, "{items} items");
    assert!(items <= 2 + 5 + layers + rules + 1, "{items} items");
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
