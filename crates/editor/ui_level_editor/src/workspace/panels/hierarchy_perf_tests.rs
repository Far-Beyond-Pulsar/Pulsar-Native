//! Headless measurement of the hierarchy tree (`HierarchicalTreeView`'s panel
//! layout) while its panel is resized and scrolled: the persistent
//! `gpui::list` the level editor's outliner uses
//! ([`ui::HierarchyListState`]) against the per-render virtual list other
//! panels still use.
//!
//! ```text
//! cargo test -p ui_level_editor --lib hierarchy_resize_report -- --ignored --nocapture --test-threads=1
//! ```

use super::super::properties::perf_tests::RENDER_LOCK;
use gpui::{
    div, px, render_stats, size, AppContext as _, Context, IntoElement, ParentElement as _,
    Render, Styled as _, TestAppContext, VisualTestContext, Window,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{HierarchicalTreeView, HierarchyConfig, HierarchyItem, HierarchyLayout, HierarchyListState};

#[derive(Clone)]
struct Node {
    id: usize,
    children: Vec<usize>,
}

#[derive(Clone)]
struct Payload;

impl Render for Payload {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl HierarchyItem for Node {
    type Id = usize;
    type DragPayload = Payload;

    fn id(&self) -> usize {
        self.id
    }
    fn name(&self) -> String {
        format!("Object {}", self.id)
    }
    fn icon(&self) -> ui::IconName {
        ui::IconName::Box
    }
    fn icon_color<V: Render>(&self, cx: &Context<V>) -> gpui::Hsla {
        use ui::ActiveTheme as _;
        cx.theme().foreground
    }
    fn children_ids(&self) -> Vec<usize> {
        self.children.clone()
    }
    fn is_selected(&self) -> bool {
        false
    }
    fn create_drag_payload(&self) -> Payload {
        Payload
    }
    fn drag_drop_id(&self) -> String {
        self.id.to_string()
    }
}

struct Host {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    list_state: Option<HierarchyListState<usize>>,
    /// The tree never changes here, so its content key never does.
    content_key: u64,
    renders: usize,
}

impl Render for Host {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let clone_scope = render_stats::scope("test host: clone items");
        let items = self.nodes.clone();
        drop(clone_scope);
        let config = HierarchyConfig {
            items,
            root_ids: self.roots.clone(),
            layout: HierarchyLayout::Panel,
            title: Some("Hierarchy".into()),
            header_buttons: Vec::new(),
            root_drop_zone: None,
            widget_title: None,
            widget_icon: None,
            widget_add_button: None,
            empty_message: String::new(),
            disable_nesting: false,
            // Every group open: the flattened tree holds every object.
            is_expanded: Arc::new(|_| true),
            on_toggle_expand: Arc::new(|_, _, _| {}),
            on_select: Arc::new(|_, _, _| {}),
            on_drop: Arc::new(|_, _, _, _, _| {}),
        };
        let view = HierarchicalTreeView::new(config);
        let view = match &self.list_state {
            Some(state) => view.with_list_state(state.clone(), self.content_key),
            None => view,
        };
        div().size_full().child(view.render(cx))
    }
}

/// 250 groups of 20 objects: 5,250 rows.
fn tree() -> (Vec<Node>, Vec<usize>) {
    let mut nodes = Vec::new();
    let mut roots = Vec::new();
    for g in 0..250 {
        let group = nodes.len();
        roots.push(group);
        nodes.push(Node {
            id: group,
            children: (1..=20).map(|c| group + c).collect(),
        });
        for c in 1..=20 {
            nodes.push(Node {
                id: group + c,
                children: Vec::new(),
            });
        }
        let _ = g;
    }
    (nodes, roots)
}

fn run(cx: &mut TestAppContext, persistent: bool) {
    cx.update(|cx| ui::init(cx));
    let (nodes, roots) = tree();
    let window = cx.open_window(size(px(320.), px(700.)), |_window, _cx| Host {
            nodes,
            roots,
            list_state: persistent.then(HierarchyListState::new),
            content_key: 1,
            renders: 0,
    });
    let host = window.root(cx).unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let draw = |vcx: &mut VisualTestContext| {
        vcx.update(|w, c| {
            w.draw(c).clear();
        })
    };
    for _ in 0..4 {
        host.update(&mut vcx, |_, cx| cx.notify());
        draw(&mut vcx);
    }

    render_stats::set_force_enabled(true);
    render_stats::reset();
    let frames = 60;
    let mut wall = Duration::ZERO;
    for step in 0..frames {
        let w = 260. + 4. * ((step % 60) as f32 - 30.).abs();
        let start = Instant::now();
        vcx.simulate_resize(size(px(w), px(700.)));
        vcx.run_until_parked();
        draw(&mut vcx);
        wall += start.elapsed();
    }
    let s = render_stats::snapshot();
    let per = |name: &str| s.counters.get(name).copied().unwrap_or(0) as f64 / frames as f64;
    let ms = |name: &str| {
        s.timers
            .get(name)
            .map(|t| t.total.as_secs_f64() * 1e3 / frames as f64)
            .unwrap_or(0.0)
    };
    eprintln!(
        "{}: resize mean wall {:.2} ms/step | layout {:.2} prepaint {:.2} paint {:.2} ms | element nodes {:.0} | list rows built {:.1} visible + {:.1} measure-only",
        if persistent { "gpui::list (persistent)" } else { "v_virtual_list (per render)" },
        wall.as_secs_f64() * 1e3 / frames as f64,
        ms("frame: layout"),
        ms("frame: prepaint"),
        ms("frame: paint"),
        per("frame: element tree nodes"),
        per("list: item built (visible)"),
        per("list: item built (measure only)"),
    );
    for name in ["test host: clone items", "hierarchy tree: flatten", "hierarchy tree: list sync", "frame: render", "taffy: compute layout"] {
        eprintln!("  {name:<28} {:.2} ms/step", ms(name));
    }
    render_stats::set_force_enabled(false);
    let renders = host.read_with(&vcx, |h, _| h.renders);
    eprintln!("  host renders in total: {renders}");
}

#[gpui::test]
#[ignore = "manual measurement; see module docs"]
fn hierarchy_resize_report(cx: &mut TestAppContext) {
    let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    run(cx, false);
    run(cx, true);
}

/// The outliner's list keeps its scroll position across renders (the
/// per-render virtual list it replaced reset to the top on every render, so
/// the panel could not be scrolled).
#[gpui::test]
fn hierarchy_list_keeps_its_scroll_across_renders(cx: &mut TestAppContext) {
    let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    cx.update(|cx| ui::init(cx));
    let (nodes, roots) = tree();
    let state = HierarchyListState::new();
    let window = cx.open_window(size(px(320.), px(700.)), |_window, _cx| Host {
            nodes,
            roots,
            list_state: Some(state.clone()),
            content_key: 1,
            renders: 0,
    });
    let host = window.root(cx).unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.update(|w, c| {
        w.draw(c).clear();
    });
    assert_eq!(state.row_count(), 250 * 21);
    state.scroll_by(px(2_000.));
    for _ in 0..3 {
        host.update(&mut vcx, |_, cx| cx.notify());
        vcx.update(|w, c| {
            w.draw(c).clear();
        });
    }
    let top = state.first_visible_row();
    assert!(top > 0, "the scroll position was lost on re-render");
}
