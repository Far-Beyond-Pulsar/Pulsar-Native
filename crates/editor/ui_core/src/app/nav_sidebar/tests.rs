//! The sidebar in a real editor window (no level editor, so no GPU needed for
//! the layout checks): it replaces the tab strip, opens over the editor on
//! hover without moving it, makes room when kept open, switches tabs, lists
//! files in its content tree, and opens the bottom drawer only when asked to.
//!
//! `sidebar_screenshots` walks the same steps with real fonts and saves a PNG
//! of each state, when `PULSAR_SIDEBAR_SHOTS` names a folder and a
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

use super::model::SectionId;
use super::render::{DRAWER_WIDTH, RAIL_WIDTH};
use super::NavSidebar;
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
    nav: Entity<NavSidebar>,
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
    let app = app.unwrap();
    let nav = cx.read(|cx| app.read(cx).state.nav_sidebar.clone());
    Editor {
        app,
        nav,
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
            self.nav.update(&mut self.cx, |_, cx| cx.notify());
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
        self.nav
            .read_with(&self.cx, |nav, _| nav.hover.is_open())
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
    // A test context has no asset source, so icons come out blank.
    let mut cx = TestAppContext::with_real_text_system();
    walk(&mut cx, Some(dir));
}

/// The footer's background updates (the task count, rust-analyzer's
/// progress, the multiuser session) repaint the footer. The sidebar reads the
/// app's state, so these used to rebuild it on every update, by notifying the
/// app; now it replays.
#[gpui::test]
fn footer_updates_do_not_rebuild_the_sidebar(cx: &mut TestAppContext) {
    let _settings = SETTINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = project();
    set("unified_sidebar", true);
    set("sidebar_pinned", false);
    let mut ed = open(cx, dir.path());
    ed.draw();
    let renders = |ed: &mut Editor| ed.nav.read_with(&ed.cx, |nav, _| nav.renders);

    let before = renders(&mut ed);
    for _ in 0..3 {
        ed.app
            .update(&mut ed.cx, |app, cx| app.notify_status_bar(cx));
        ed.cx.run_until_parked();
    }
    assert_eq!(
        renders(&mut ed),
        before,
        "a footer update rebuilt the sidebar"
    );

    // A change to the app itself still reaches it.
    ed.app.update(&mut ed.cx, |_, cx| cx.notify());
    ed.cx.run_until_parked();
    assert!(
        renders(&mut ed) > before,
        "an app change did not reach the sidebar"
    );
    set("unified_sidebar", false);
}

/// The drawer stacks its panes like VS Code's side bar: each scrolls on its
/// own with a scrollbar, the border between them drags, and a collapsed pane
/// gives its height to the others.
#[gpui::test]
fn the_panes_scroll_on_their_own_and_share_the_height(cx: &mut TestAppContext) {
    use super::panes::PaneKind;
    let _settings = SETTINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = project();
    // Enough files that the content pane overflows.
    let maps = dir.path().join("Content/Maps");
    for n in 0..60 {
        std::fs::write(maps.join(format!("Level{n:02}.level")), b"").unwrap();
    }
    set("unified_sidebar", true);
    set("sidebar_pinned", true);
    let mut ed = open(cx, dir.path());
    ed.draw();
    ed.nav
        .update(&mut ed.cx, |nav, cx| nav.toggle_folder(&maps, cx));
    ed.draw();

    let editors = ed.bounds("nav-pane-body-Editors").expect("editors pane");
    let content = ed.bounds("nav-pane-body-Content").expect("content pane");
    assert!(editors.bottom() <= content.top(), "the panes stack");
    let share =
        f32::from(editors.size.height) / f32::from(editors.size.height + content.size.height);
    assert!(
        (share - 0.4).abs() < 0.02,
        "a new project splits 2:3 ({share})"
    );

    // A scroll over the content pane moves it alone.
    let offset = |ed: &mut Editor, kind| {
        ed.nav
            .read_with(&ed.cx, |nav, _| nav.pane_view(kind).scroll.offset().y)
    };
    ed.cx.simulate_event(gpui::ScrollWheelEvent {
        position: content.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-120.))),
        ..Default::default()
    });
    ed.draw();
    assert!(
        offset(&mut ed, PaneKind::Content) < px(0.),
        "the content pane scrolled"
    );
    assert_eq!(
        offset(&mut ed, PaneKind::Editors),
        px(0.),
        "the editors pane did not"
    );

    // Dragging the border moves height from one pane to the other.
    let border = ed.bounds("nav-pane-border-0").expect("border");
    ed.cx
        .simulate_mouse_down(border.center(), gpui::MouseButton::Left, Modifiers::none());
    let to = border.center() + point(px(0.), px(60.));
    ed.cx
        .simulate_mouse_move(to, Some(gpui::MouseButton::Left), Modifiers::none());
    ed.cx
        .simulate_mouse_up(to, gpui::MouseButton::Left, Modifiers::none());
    ed.draw();
    let editors_after = ed.bounds("nav-pane-body-Editors").unwrap();
    let content_after = ed.bounds("nav-pane-body-Content").unwrap();
    let grew = f32::from(editors_after.size.height - editors.size.height);
    let shrank = f32::from(content.size.height - content_after.size.height);
    assert!(
        (grew - 60.).abs() < 2. && (shrank - 60.).abs() < 2.,
        "{grew} / {shrank}"
    );
    assert!(ed.nav.read_with(&ed.cx, |nav, _| nav.border_drag.is_none()));
    let saved = ed
        .nav
        .read_with(&ed.cx, |nav, _| nav.model.save(dir.path()).panes);
    assert_eq!(
        saved,
        ed.nav.read_with(&ed.cx, |nav, _| nav.model.panes.clone())
    );

    // Collapsing the editors pane gives its height to the content pane.
    let header = ed.bounds("nav-pane-Editors").expect("editors header");
    ed.cx.simulate_click(header.center(), Modifiers::none());
    ed.draw();
    let collapsed = ed.nav.read_with(&ed.cx, |nav, _| {
        nav.model.panes.is_collapsed(PaneKind::Editors)
    });
    assert!(collapsed);
    let content_alone = ed.bounds("nav-pane-body-Content").unwrap();
    assert!(
        content_alone.size.height > content_after.size.height + editors_after.size.height - px(1.),
        "the content pane took the height"
    );
    set("unified_sidebar", false);
    set("sidebar_pinned", false);
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

    // Choosing "Open in file drawer" lists a folder's assets there.
    let maps = dir.path().join("Content/Maps");
    ed.nav
        .update(&mut ed.cx, |nav, cx| nav.open_in_drawer(maps.clone(), cx));
    // Keep the hover drawer open to measure it; the click would close it.
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.hover.set_rail(true);
        cx.notify();
    });
    ed.draw();
    ed.app.read_with(&ed.cx, |app, cx| {
        assert!(app.state.drawer_open);
        let drawer = app.state.file_manager_drawer.read(cx);
        assert_eq!(drawer.selected_folder(), Some(maps.as_path()));
        assert!(drawer.folder_tree_hidden(), "the sidebar supplies the tree");
    });
    // The hover sidebar stops above the floating file drawer.
    let drawer_height = ed.app.read_with(&ed.cx, |app, _| app.state.drawer_height);
    let sidebar = ed.bounds("nav-sidebar-drawer").expect("hover sidebar");
    assert_eq!(
        sidebar.bottom(),
        area.bottom() - px(drawer_height),
        "does not cover the assets"
    );
    shots.save(cx, "3-hover-above-drawer");
    // The pointer moves on to the assets, so the hover drawer closes.
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.hover.close();
        cx.notify();
    });
    ed.draw();
    shots.save(cx, "4-folder-assets");
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
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.hover.set_rail(true);
        cx.notify();
    });
    ed.nav.update_in(&mut ed.cx, |nav, window, cx| {
        nav.activate_tab(&hero, window, cx)
    });
    ed.draw();
    assert!(ed.app.read_with(&ed.cx, |app, cx| {
        app.sidebar_tabs(cx)
            .iter()
            .any(|t| t.title == "Hero.class" && t.active)
    }));
    assert!(ed.bounds("page-Hero.class").is_some(), "its page is drawn");
    assert!(!ed.hover_open(), "choosing a tab closes the hover drawer");

    // The user's groups: one made from a row, another tab dropped on it.
    let hero_key = hero.key.clone();
    ed.nav.update_in(&mut ed.cx, |nav, window, cx| {
        nav.new_group(Some(hero_key.clone()), window, cx)
    });
    let (group, field) = ed.nav.read_with(&ed.cx, |nav, _| {
        let group = nav.model.groups()[0].id;
        let field = nav.rename_field(group).cloned();
        (
            group,
            field.expect("a new group starts with its name being edited"),
        )
    });
    field.update_in(&mut ed.cx, |field, window, cx| {
        field.set_value("Combat", window, cx)
    });
    ed.nav.update(&mut ed.cx, |nav, cx| nav.finish_group_rename(cx));

    let door = tabs.iter().position(|t| t.title == "Door.class").unwrap();
    let drag = ed
        .app
        .read_with(&ed.cx, |app, cx| app.sidebar_tab_drag(door, cx))
        .expect("rows drag like tabs");
    ed.app.read_with(&ed.cx, |_, cx| {
        assert_eq!(drag.panel().tab_name(cx).as_deref(), Some("Door.class"));
    });
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.drop_on_section(&SectionId::Custom(group), &drag, cx)
    });
    ed.draw();
    let tabs_now = ed.app.read_with(&ed.cx, |app, cx| app.sidebar_tabs(cx));
    let sections = ed
        .nav
        .read_with(&ed.cx, |nav, _| nav.model.sections(&tabs_now));
    let combat = sections
        .iter()
        .find(|s| s.id == SectionId::Custom(group))
        .unwrap();
    assert_eq!(combat.label, "Combat");
    let titles: Vec<&str> = combat.tabs.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Hero.class", "Door.class"]);
    assert!(
        sections.iter().all(|s| s.label != "Blueprint Editor"),
        "both blueprints left their editor-kind group"
    );

    content_tree(&mut ed, dir.path(), &shots, cx);

    // Kept open: the drawer takes its own column and the editor moves over.
    set("sidebar_pinned", true);
    ed.draw();
    let drawer = ed.bounds("nav-sidebar-drawer").expect("drawer kept open");
    let area = ed.bounds("nav-sidebar-editor-area").unwrap();
    assert_eq!(area.origin.x, drawer.origin.x + px(DRAWER_WIDTH));
    assert_eq!(drawer.origin.x, px(0.), "in the rail's place");
    shots.save(cx, "5-groups-kept-open");

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

/// The content tree (#1139): files under expanded folders, a click that
/// expands without opening the file drawer, the right-click menu, and the
/// file operations it offers.
fn content_tree(ed: &mut Editor, root: &Path, shots: &Screenshots, cx: &mut TestAppContext) {
    let maps = root.join("Content/Maps");
    let arena = maps.join("Arena.level");
    ed.app.update(&mut ed.cx, |app, cx| {
        app.state.drawer_open = false;
        cx.notify();
    });
    let drawer_open = |ed: &mut Editor| ed.app.read_with(&ed.cx, |app, _| app.state.drawer_open);

    // Open the hover drawer and expand Maps with a click on its row.
    let rail = ed.bounds("nav-sidebar-rail").expect("rail");
    let away = point(px(1000.), px(400.));
    ed.cx.simulate_mouse_move(away, None, Modifiers::none());
    ed.cx
        .simulate_mouse_move(rail.center(), None, Modifiers::none());
    ed.draw();
    assert!(ed.hover_open());
    assert!(
        ed.bounds("nav-entry-Arena.level").is_none(),
        "a collapsed folder lists no files"
    );
    let maps_row = ed.bounds("nav-entry-Maps").expect("Maps row");
    // The pointer reaches the row before it clicks, as a hand's does.
    ed.cx
        .simulate_mouse_move(maps_row.center(), None, Modifiers::none());
    ed.cx.simulate_click(maps_row.center(), Modifiers::none());
    ed.draw();
    let arena_row = ed
        .bounds("nav-entry-Arena.level")
        .expect("the expanded folder lists its files");
    assert!(ed.bounds("nav-entry-Canyon.level").is_some());
    assert!(arena_row.origin.y > maps_row.origin.y, "below their folder");
    assert!(!drawer_open(ed), "expanding a folder leaves the file drawer closed");
    shots.save(cx, "6-content-files");

    // One click selects a file, still without the file drawer.
    ed.cx.simulate_click(arena_row.center(), Modifiers::none());
    ed.draw();
    assert_eq!(
        ed.nav.read_with(&ed.cx, |nav, _| nav.selected_path().map(Path::to_path_buf)),
        Some(arena.clone())
    );
    assert!(!drawer_open(ed));

    // A right-click menu keeps the hover drawer open while the pointer is on
    // the menu, and lets it close once the menu goes.
    ed.cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Right,
        position: arena_row.center(),
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    ed.cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Right,
        position: arena_row.center(),
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    ed.draw();
    shots.save(cx, "7-content-menu");
    ed.cx.simulate_mouse_move(away, None, Modifiers::none());
    ed.cx
        .executor()
        .advance_clock(super::model::HOVER_CLOSE_DELAY * 2);
    ed.cx.run_until_parked();
    assert!(ed.hover_open(), "held open while its menu is up");
    ed.cx.simulate_click(away, Modifiers::none());
    ed.cx
        .executor()
        .advance_clock(super::model::HOVER_CLOSE_DELAY * 2);
    ed.cx.run_until_parked();
    assert!(!ed.hover_open(), "closes once the menu is dismissed");

    // The icon at a hovered row's end reveals the file in the file drawer:
    // its folder listed, the file selected.
    ed.cx
        .simulate_mouse_move(rail.center(), None, Modifiers::none());
    ed.draw();
    let arena_row = ed.bounds("nav-entry-Arena.level").expect("Arena row");
    ed.cx
        .simulate_mouse_move(arena_row.center(), None, Modifiers::none());
    ed.draw();
    let reveal = ed.bounds("nav-reveal-Arena.level").expect("reveal icon");
    assert!(arena_row.contains(&reveal.center()), "at the row's end");
    ed.cx
        .simulate_mouse_move(reveal.center(), None, Modifiers::none());
    ed.cx.simulate_click(reveal.center(), Modifiers::none());
    ed.draw();
    assert!(drawer_open(ed));
    ed.app.read_with(&ed.cx, |app, cx| {
        let drawer = app.state.file_manager_drawer.read(cx);
        assert_eq!(drawer.selected_folder(), Some(maps.as_path()));
        assert!(drawer.is_item_selected(&arena));
    });

    // Double-clicking a folder lists it in the file drawer; its first click
    // expanded it. (Showing the file drawer closed the hover sidebar.)
    ed.app.update(&mut ed.cx, |app, cx| {
        app.state.drawer_open = false;
        cx.notify();
    });
    ed.cx
        .simulate_mouse_move(rail.center(), None, Modifiers::none());
    ed.draw();
    assert!(ed.hover_open());
    let materials = root.join("Content/Materials");
    let materials_row = ed.bounds("nav-entry-Materials").expect("Materials row");
    ed.cx
        .simulate_mouse_move(materials_row.center(), None, Modifiers::none());
    for click_count in [1, 2] {
        ed.cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: materials_row.center(),
            modifiers: Modifiers::none(),
            click_count,
            first_mouse: false,
        });
        ed.cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position: materials_row.center(),
            modifiers: Modifiers::none(),
            click_count,
        });
    }
    ed.draw();
    ed.app.read_with(&ed.cx, |app, cx| {
        let drawer = app.state.file_manager_drawer.read(cx);
        assert_eq!(drawer.selected_folder(), Some(materials.as_path()));
    });
    assert!(ed
        .nav
        .read_with(&ed.cx, |nav, _| nav.model.folder_expanded(&materials)));
    ed.nav
        .update(&mut ed.cx, |nav, cx| nav.toggle_folder(&materials, cx));
    ed.app.update(&mut ed.cx, |app, cx| {
        app.state.drawer_open = false;
        cx.notify();
    });

    // Rename in place.
    ed.nav.update_in(&mut ed.cx, |nav, window, cx| {
        nav.start_path_rename(arena.clone(), window, cx)
    });
    let field = ed
        .nav
        .read_with(&ed.cx, |nav, _| nav.path_rename_field(&arena).cloned())
        .expect("renaming shows a text field");
    field.update_in(&mut ed.cx, |field, window, cx| {
        field.set_value("Duel.level", window, cx)
    });
    ed.nav.update(&mut ed.cx, |nav, cx| nav.finish_path_rename(cx));
    let duel = maps.join("Duel.level");
    assert!(duel.exists() && !arena.exists());
    assert_eq!(
        ed.nav.read_with(&ed.cx, |nav, _| nav.selected_path().map(Path::to_path_buf)),
        Some(duel.clone()),
        "the selection follows the rename"
    );

    // Copy, then paste into another folder and beside itself.
    let characters = root.join("Content/Characters");
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.put_on_clipboard(duel.clone(), false, cx);
        nav.paste_into(characters.clone(), cx);
        nav.paste_into(maps.clone(), cx);
    });
    assert!(characters.join("Duel.level").exists());
    assert!(
        maps.join("Duel copy.level").exists(),
        "pasting where the name is taken adds a copy suffix"
    );
    assert!(duel.exists(), "copying leaves the original");
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.hover.set_rail(true);
        cx.notify();
    });
    ed.draw();
    assert!(
        ed.bounds("nav-entry-Duel copy.level").is_some(),
        "the tree lists the pasted file"
    );

    // Cut moves; delete removes.
    let materials = root.join("Content/Materials");
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.put_on_clipboard(characters.join("Duel.level"), true, cx);
        nav.paste_into(materials.clone(), cx);
    });
    assert!(materials.join("Duel.level").exists());
    assert!(!characters.join("Duel.level").exists());
    assert!(
        !ed.nav.read_with(&ed.cx, |nav, cx| nav.can_paste(cx)),
        "a cut is pasted once"
    );
    ed.nav.update(&mut ed.cx, |nav, cx| {
        nav.delete_path(materials.join("Duel.level"), cx)
    });
    assert!(!materials.join("Duel.level").exists());
    ed.draw();

    // A pane whose rows overflow it scrolls on its own, with a scrollbar.
    ed.nav.update(&mut ed.cx, |nav, cx| {
        for _ in 0..30 {
            nav.put_on_clipboard(maps.join("Canyon.level"), false, cx);
            nav.paste_into(maps.clone(), cx);
        }
        nav.hover.set_rail(true);
        cx.notify();
    });
    ed.draw();
    let content = ed.bounds("nav-pane-body-Content").expect("content pane");
    ed.cx
        .simulate_mouse_move(content.center(), None, Modifiers::none());
    ed.cx.simulate_event(gpui::ScrollWheelEvent {
        position: content.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-200.))),
        ..Default::default()
    });
    ed.draw();
    let scrolled = ed.nav.read_with(&ed.cx, |nav, _| {
        nav.pane_view(super::panes::PaneKind::Content)
            .scroll
            .offset()
            .y
    });
    assert!(scrolled < px(0.), "the content pane scrolls");
    // By default the scrollbar shows while scrolling and fades after; a
    // still image needs it always shown.
    let set_scrollbar = |ed: &mut Editor, show| {
        ed.cx
            .update(|_, cx| ui::Theme::global_mut(cx).scrollbar_show = show);
        ed.draw();
    };
    set_scrollbar(ed, ui::scroll::ScrollbarShow::Always);
    shots.save(cx, "8-content-pane-scrolled");
    set_scrollbar(ed, ui::scroll::ScrollbarShow::default());
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
