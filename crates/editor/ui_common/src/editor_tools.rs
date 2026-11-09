//! What an optional editor tool adds to the editor shell, so the shell names
//! no tool (Pulsar-Native#1605): a status-bar button and a Tools menu entry
//! that open its window, an item in the title bar, an entry in the account
//! menu, and work when a project opens. The tool's crate
//! submits the registrations; the editor binary links the tools it ships
//! (git, the flamegraph, Mission Control, Fab search), and `ui_core` and the
//! loading screen read them here.
//!
//! How others do it: Unreal's editor modules extend the level editor's
//! toolbar and menus through `UToolMenus` from their own `StartupModule`;
//! VS Code contributes status-bar items and commands from an extension's
//! manifest. Ours is the same idea through the registrations every plugin
//! uses (`pulsar_plugins::registration!`), so the catalog lists the tools'
//! contributions too.

use std::path::Path;

use gpui::{AnyView, App, Window};

/// An optional tool's button in the status bar, which opens its window.
pub struct EditorToolRegistration {
    /// The submitting package (`pulsar_plugins::engine_origin!()` for the
    /// engine's tools, `origin!()` for a plugin's).
    pub origin: &'static str,
    /// Stable id (`tool.flamegraph`).
    pub id: &'static str,
    /// The button's tooltip (the tool's own translation).
    pub label: fn() -> String,
    pub icon: ui::IconName,
    /// Buttons are drawn in this order, then by id.
    pub order: i32,
    /// Open (or focus) the tool's window.
    pub open: fn(&mut Window, &mut App),
    /// Whether the tool is open now (the button is highlighted).
    pub is_open: Option<fn(&App) -> bool>,
    /// Run once when the editor starts (the tool's parameterised windows,
    /// its background tasks).
    pub init: Option<fn(&mut App)>,
}

impl pulsar_plugins::Contribution for EditorToolRegistration {
    const CATEGORY: &'static str = "editor.tool";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }

    fn label(&self) -> String {
        (self.label)()
    }
}

pulsar_plugins::registration!(EditorToolRegistration);

/// Every enabled tool, in button order.
pub fn editor_tools() -> Vec<&'static EditorToolRegistration> {
    let mut tools: Vec<_> = pulsar_plugins::registered::<EditorToolRegistration>().collect();
    tools.sort_by_key(|tool| (tool.order, tool.id));
    tools
}

/// Initialize every enabled tool (once, when the editor starts).
pub fn init_editor_tools(cx: &mut App) {
    for tool in editor_tools() {
        if let Some(init) = tool.init {
            init(cx);
        }
    }
}

/// An item a tool shows in the window's title bar for the open project (the
/// git tool's sync state).
pub struct TitleBarItemRegistration {
    /// The submitting package.
    pub origin: &'static str,
    /// Stable id (`git.sync_state`).
    pub id: &'static str,
    /// Items are drawn in this order, then by id.
    pub order: i32,
    /// The item for the project at the path (`None`: no project open).
    pub build: fn(project: Option<&Path>, window: &mut Window, cx: &mut App) -> AnyView,
}

impl pulsar_plugins::Contribution for TitleBarItemRegistration {
    const CATEGORY: &'static str = "editor.title_bar_item";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }
}

pulsar_plugins::registration!(TitleBarItemRegistration);

/// Every enabled title-bar item, in order.
pub fn title_bar_items() -> Vec<&'static TitleBarItemRegistration> {
    let mut items: Vec<_> = pulsar_plugins::registered::<TitleBarItemRegistration>().collect();
    items.sort_by_key(|item| (item.order, item.id));
    items
}

/// Work a tool does while the editor opens a project, shown as a step of the
/// loading screen (the git tool syncs the project's configured hooks).
pub struct ProjectOpenedRegistration {
    /// The submitting package.
    pub origin: &'static str,
    /// Stable id (`git.hooks`).
    pub id: &'static str,
    /// The step's label on the loading screen.
    pub label: fn() -> String,
    /// Run on the loading worker for the project at the path; returns what
    /// the step did, for the loading screen's detail line.
    pub run: fn(project: &Path) -> String,
}

impl pulsar_plugins::Contribution for ProjectOpenedRegistration {
    const CATEGORY: &'static str = "editor.project_opened";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }

    fn label(&self) -> String {
        (self.label)()
    }
}

pulsar_plugins::registration!(ProjectOpenedRegistration);

/// Every enabled project-opened step, by id.
pub fn project_opened_steps() -> Vec<&'static ProjectOpenedRegistration> {
    let mut steps: Vec<_> = pulsar_plugins::registered::<ProjectOpenedRegistration>().collect();
    steps.sort_by_key(|step| step.id);
    steps
}

/// An entry a tool adds to the account menu (the profile dropdown), under
/// its own section (the git tool's settings).
pub struct AccountMenuItemRegistration {
    /// The submitting package.
    pub origin: &'static str,
    /// Stable id (`git.settings`).
    pub id: &'static str,
    /// The section heading it is listed under.
    pub section: fn() -> String,
    pub label: fn() -> String,
    pub icon: ui::IconName,
    /// What the entry opens.
    pub open: fn(&mut Window, &mut App),
}

impl pulsar_plugins::Contribution for AccountMenuItemRegistration {
    const CATEGORY: &'static str = "editor.account_menu_item";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }

    fn label(&self) -> String {
        (self.label)()
    }
}

pulsar_plugins::registration!(AccountMenuItemRegistration);

/// Every enabled account-menu entry, by id.
pub fn account_menu_items() -> Vec<&'static AccountMenuItemRegistration> {
    let mut items: Vec<_> = pulsar_plugins::registered::<AccountMenuItemRegistration>().collect();
    items.sort_by_key(|item| item.id);
    items
}
