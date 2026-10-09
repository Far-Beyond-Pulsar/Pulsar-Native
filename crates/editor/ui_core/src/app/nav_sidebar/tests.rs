//! The sidebar in a real editor window (no level editor, so no GPU needed for
//! the layout checks): it replaces the tab strip, opens over the editor on
//! hover without moving it, makes room when kept open, switches tabs, and lists
//! a folder's assets in the bottom drawer.
//!
//! `sidebar_screenshots` walks the same steps with real fonts and icons and
//! saves a PNG of each state, when `PULSAR_SIDEBAR_SHOTS` names a folder and a
//! GPU adapter is available.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use engine_state::settings::{global_config, ConfigValue, NS_EDITOR};
use gpui::{
    div, point, px, size, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Pixels, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, WindowBounds, WindowOptions,
};
use ui::dock::{Panel, PanelEvent, PanelView};

use super::render::{DRAWER_WIDTH, RAIL_WIDTH};
use crate::app::PulsarApp;

/// A stand-in editor tab.
struct Page {
    kind: &'static str,
    title: &'static str,
    file: Option<PathBuf>,
    focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for Page {}

impl Focusable for Page {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for Page {
    fn panel_name(&self) -> &'static str {
        self.kind
    }
    fn tab_name(&self, _: &gpui::App) -> Option<gpui::SharedString> {
        Some(self.title.into())
    }
    fn panel_file_path(&self, _: &gpui::App) -> Option<PathBuf> {
        self.file.clone()
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::ActiveTheme as _;
        let title = self.title;
        div()
            .size_full()
            .bg(cx.theme().background)
            .debug_selector(move || format!("page-{title}"))
            .child(title)
    }
}

fn set(key: &str, on: bool) {
    // Registering again is harmless; the test may set a value before opening.
    pulsar_settings::editor::navigation::register(global_config());
    let handle = global_config()
        .owner_handle(NS_EDITOR, "navigation")
        .expect("navigation settings registered");
    handle.set(key, ConfigValue::Bool(on)).unwrap();
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for folder in [
        "Content/Maps",
        "Content/Characters/Hero",
        "Content/Materials",
        "src",
    ] {
        std::fs::create_dir_all(dir.path().join(folder)).unwrap();
    }
    for file in [
        "Content/Maps/Arena.level",
        "Content/Maps/Canyon.level",
        "Content/Characters/Hero/Hero.png",
    ] {
        std::fs::write(dir.path().join(file), b"").unwrap();
    }
    dir
}

struct Editor {
    app: Entity<PulsarApp>,
    pages: Vec<Entity<Page>>,
    cx: VisualTestContext,
    window: gpui::AnyWindowHandle,
}

fn open(cx: &mut TestAppContext, root: &Path) -> Editor {
    // As the engine binary does at startup, for the window's HTTP clients.
    let _ = rustls::crypto::ring::default_provider().install_default();
    engine_state::EngineContext::new().set_global();
    cx.update(|cx| {
        // As the engine's startup does, before any init.
        cx.set_global(window_manager::WindowManager::new());
        cx.set_global(window_manager::WindowRegistry::new());
        ui::init(cx);
        ui::themes::init(cx);
    });
    let root = root.to_path_buf();
    let mut app = None;
    let mut pages = Vec::new();
    let window = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(1280.), px(800.)),
            ))),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            let analyzer =
                cx.new(|cx| engine_backend::services::RustAnalyzerManager::new(window, cx));
            let view = cx.new(|cx| {
                let mut app =
                    PulsarApp::new_internal(Some(root), Some(analyzer), None, false, window, cx);
                // No "project loaded" toast: it would outlive the test.
                app.state.shown_welcome_notification = true;
                app
            });
            for (kind, title, file) in [
                ("Level Editor", "Arena", None),
                (
                    "Blueprint Editor",
                    "Hero.class",
                    Some("Content/Characters/Hero.class"),
                ),
                ("Blueprint Editor", "Door.class", Some("Content/Door.class")),
                (
                    "Material Editor",
                    "Rock.mat",
                    Some("Content/Materials/Rock.mat"),
                ),
            ] {
                let page = cx.new(|cx| Page {
                    kind,
                    title,
                    file: file.map(PathBuf::from),
                    focus: cx.focus_handle(),
                });
                let center = view.read(cx).state.center_tabs.clone();
                center.update(cx, |tabs, cx| {
                    tabs.add_panel(Arc::new(page.clone()) as Arc<dyn PanelView>, window, cx)
                });
                pages.push(page);
            }
            app = Some(view.clone());
            cx.new(|cx| ui::Root::new(view.into(), window, cx))
        })
        .expect("window")
    });
    cx.run_until_parked();
    let handle: gpui::AnyWindowHandle = window.into();
    Editor {
        app: app.unwrap(),
        pages,
        cx: VisualTestContext::from_window(handle, cx),
        window: handle,
    }
}

impl Editor {
    /// Draw until the layout settles. The sidebar applies its mode after a
    /// frame, and cached views record no fresh bounds unless marked changed.
    fn draw(&mut self) {
        for _ in 0..3 {
            for page in &self.pages {
                page.update(&mut self.cx, |_, cx| cx.notify());
            }
            self.app.update(&mut self.cx, |_, cx| cx.notify());
            self.cx.update(|window, cx| window.draw(cx).clear());
            self.cx.run_until_parked();
        }
    }

    fn bounds(&mut self, selector: &'static str) -> Option<Bounds<Pixels>> {
        self.cx.debug_bounds(selector)
    }

    /// Whether the hover drawer is open. (Debug bounds outlive the frame that
    /// drew them, so they can't tell that something is gone.)
    fn hover_open(&mut self) -> bool {
        self.app
            .read_with(&self.cx, |app, _| app.state.nav_sidebar.hover.is_open())
    }
}

#[gpui::test]
fn the_sidebar_replaces_the_tab_strip_and_never_moves_the_viewport(cx: &mut TestAppContext) {
    walk(cx, None);
}

#[test]
fn sidebar_screenshots() {
    let Some(dir) = std::env::var_os("PULSAR_SIDEBAR_SHOTS").map(PathBuf::from) else {
        return;
    };
    let mut cx = TestAppContext::with_real_text_system();
    cx.set_asset_source(ui::Assets);
    walk(&mut cx, Some(dir));
}

/// The navigation settings are process-wide; one walk at a time.
static SETTINGS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every state of the sidebar, checked; with `shots`, also saved as PNGs.
fn walk(cx: &mut TestAppContext, shots: Option<PathBuf>) {
    let _settings = SETTINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = project();
    set("unified_sidebar", false);
    set("sidebar_pinned", false);
    let mut ed = open(cx, dir.path());
    let shots = Screenshots::new(cx, ed.window, shots);

    // Off: the usual tab strip above the page, no sidebar.
    ed.draw();
    let page = ed.bounds("page-Rock.mat").expect("active page");
    assert!(page.origin.y > px(0.), "the tab strip sits above the page");
    assert!(ed.bounds("nav-sidebar-rail").is_none(), "no sidebar yet");
    shots.save(cx, "0-tab-bar");

    // On: a rail on the left, the page fills the editor area.
    set("unified_sidebar", true);
    ed.draw();
    let rail = ed.bounds("nav-sidebar-rail").expect("rail");
    let area = ed.bounds("nav-sidebar-editor-area").expect("editor area");
    assert_eq!(rail.size.width, px(RAIL_WIDTH));
    assert_eq!(area.origin.x, rail.origin.x + px(RAIL_WIDTH));
    let page = ed.bounds("page-Rock.mat").expect("active page");
    assert_eq!(
        page.origin, area.origin,
        "no tab strip: the page starts at the area's corner"
    );
    shots.save(cx, "1-collapsed-rail");

    // Hover: the drawer opens over the editor; nothing moves.
    let tabs = ed.app.read_with(&ed.cx, |app, cx| app.sidebar_tabs(cx));
    assert_eq!(tabs.len(), 4);
    ed.cx
        .simulate_mouse_move(rail.center(), None, Modifiers::none());
    ed.draw();
    assert!(ed.hover_open(), "hovering the rail opens the drawer");
    let drawer = ed.bounds("nav-sidebar-drawer").expect("drawer drawn");
    assert_eq!(
        drawer.origin.x, area.origin.x,
        "the drawer lies over the editor, beside the rail"
    );
    assert_eq!(drawer.size.width, px(DRAWER_WIDTH));
    assert_eq!(
        ed.bounds("nav-sidebar-editor-area").unwrap(),
        area,
        "the editor area did not move"
    );
    assert_eq!(
        ed.bounds("page-Rock.mat").unwrap(),
        page,
        "the viewport did not move"
    );
    shots.save(cx, "2-hover-drawer");

    // Choosing a folder lists its assets in the bottom drawer.
    let maps = dir.path().join("Content/Maps");
    ed.app.update(&mut ed.cx, |app, cx| {
        app.show_sidebar_folder(maps.clone(), cx)
    });
    ed.draw();
    ed.app.read_with(&ed.cx, |app, cx| {
        assert!(app.state.drawer_open);
        let drawer = app.state.file_manager_drawer.read(cx);
        assert_eq!(drawer.selected_folder(), Some(maps.as_path()));
        assert!(drawer.folder_tree_hidden(), "the sidebar supplies the tree");
    });
    // The pointer moves on to the assets, so the hover drawer closes.
    ed.app.update(&mut ed.cx, |app, cx| {
        app.state.nav_sidebar.hover.close();
        cx.notify();
    });
    ed.draw();
    shots.save(cx, "3-folder-assets");
    ed.app.update(&mut ed.cx, |app, cx| {
        app.state.drawer_open = false;
        cx.notify();
    });

    // Clicking a row shows that tab.
    let hero = tabs
        .iter()
        .find(|t| t.title == "Hero.class")
        .unwrap()
        .clone();
    ed.app.update_in(&mut ed.cx, |app, window, cx| {
        app.activate_sidebar_tab(&hero, window, cx)
    });
    ed.draw();
    assert!(ed.app.read_with(&ed.cx, |app, cx| {
        app.sidebar_tabs(cx)
            .iter()
            .any(|t| t.title == "Hero.class" && t.active)
    }));
    assert!(ed.bounds("page-Hero.class").is_some(), "its page is drawn");
    assert!(!ed.hover_open(), "choosing a tab closes the hover drawer");

    // Kept open: the drawer takes its own column and the editor moves over.
    set("sidebar_pinned", true);
    ed.draw();
    let drawer = ed.bounds("nav-sidebar-drawer").expect("drawer kept open");
    let area = ed.bounds("nav-sidebar-editor-area").unwrap();
    assert_eq!(area.origin.x, drawer.origin.x + px(DRAWER_WIDTH));
    assert_eq!(drawer.origin.x, px(0.), "in the rail's place");
    shots.save(cx, "4-kept-open");

    set("unified_sidebar", false);
    set("sidebar_pinned", false);
    ed.draw();
    let page = ed.bounds("page-Hero.class").unwrap();
    assert!(
        page.origin.y > px(0.),
        "turning it off brings the tab strip back"
    );

    ed.cx.update(|window, _| window.remove_window());
    ed.cx.run_until_parked();
}

/// PNGs of the window into `dir`, when there is one and a GPU adapter is
/// available; otherwise does nothing.
struct Screenshots {
    window: Option<gpui::headless::HeadlessWindow>,
    dir: Option<PathBuf>,
}

impl Screenshots {
    fn new(cx: &mut TestAppContext, handle: gpui::AnyWindowHandle, dir: Option<PathBuf>) -> Self {
        let window = dir
            .as_ref()
            .and_then(|_| gpui::headless::HeadlessWindow::attach(handle, cx));
        if dir.is_some() && window.is_none() {
            eprintln!("no GPU adapter: no screenshots");
        }
        Self { window, dir }
    }

    fn save(&self, cx: &mut TestAppContext, name: &str) {
        let (Some(window), Some(dir)) = (&self.window, &self.dir) else {
            return;
        };
        // Virtual lists (the asset grid) size themselves from the frame before.
        for _ in 0..3 {
            window.draw(cx, true);
        }
        let _ = window.settle(cx);
        let (width, height) = window.size();
        let rgba = window.presented();
        std::fs::create_dir_all(dir).unwrap();
        image::RgbaImage::from_raw(width, height, rgba)
            .expect("frame size")
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
}
