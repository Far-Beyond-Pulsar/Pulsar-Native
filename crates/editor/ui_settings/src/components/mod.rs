//! The settings screen: a searchable navigation tree on the left (a
//! virtualized `uniform_list`, up to three levels deep) and the selected
//! page on the right (a measured `list`, so rows of any height never overlap
//! and a section jump lands exactly on its heading).

mod content;
mod fields;
mod game;
mod save_bar;
mod sidebar;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use gpui::{
    div, prelude::FluentBuilder as _, px, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ListAlignment, ListState, ParentElement as _, Render,
    Styled, Subscription, UniformListScrollHandle, Window,
};
use ui::{
    h_flex,
    input::{InputEvent, InputState},
    scroll::ScrollbarState,
    v_flex, ActiveTheme as _,
};

use crate::utils::model::{self, ContentRow, Page, SettingId, TreeRow, WindowKind, PLUGINS_FOLDER};

pub use fields::FieldState;
pub use save_bar::render_save_bar;

/// The width of the navigation tree.
const SIDEBAR_WIDTH: f32 = 280.0;

pub struct SettingsScreen {
    pub(crate) kind: WindowKind,
    pub(crate) project_path: Option<PathBuf>,
    pub(crate) focus_handle: FocusHandle,

    pub(crate) search: Entity<InputState>,
    /// The search text, lowercased.
    pub(crate) query: String,

    /// Every page, as registered.
    pub(crate) all_pages: Vec<Page>,
    /// The pages the search keeps (all of them when not searching).
    pub(crate) pages: Vec<Page>,
    /// The plugin set the pages were built for (`pulsar_plugins::generation`).
    pub(crate) plugin_generation: u64,

    /// Open tree rows, by id.
    pub(crate) expanded: BTreeSet<String>,
    pub(crate) tree: Vec<TreeRow>,
    pub(crate) tree_scroll: UniformListScrollHandle,
    pub(crate) tree_scrollbar: ScrollbarState,

    /// The page shown, by id (it survives a rebuild or a search).
    pub(crate) selected_page: Option<String>,
    /// The section the content is scrolled to (highlighted in the tree).
    pub(crate) selected_section: Option<usize>,
    pub(crate) rows: Vec<ContentRow>,
    /// The content row of each section heading.
    pub(crate) headings: Vec<usize>,
    pub(crate) content: ListState,
    pub(crate) content_scrollbar: ScrollbarState,
    /// A section heading to scroll to once the new page has been laid out.
    pub(crate) pending_jump: Option<usize>,

    /// The editor state of text, number and color fields, by setting.
    pub(crate) fields: HashMap<SettingId, FieldState>,
    /// The Game page's keys of the project file (the project window with a
    /// project open).
    pub(crate) game: Option<crate::utils::game::GameSettings>,
    /// The Game page's asset pickers, by key.
    pub(crate) game_pickers: HashMap<String, game::GamePicker>,

    pub(crate) has_pending_changes: bool,
    /// A changed setting applies only after the editor restarts.
    pub(crate) restart_required: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsScreen {
    pub fn new(
        kind: WindowKind,
        project_path: Option<PathBuf>,
        initial_page: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let search_subscription = cx.subscribe(&search, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().trim().to_lowercase();
                crate::handlers::set_query(this, query);
                cx.notify();
            }
        });

        let content = ListState::new(0, ListAlignment::Top, px(600.0)).measure_all();
        let weak = cx.entity().downgrade();
        content.set_scroll_handler(move |event, _, cx| {
            let first = event.visible_range.start;
            let _ = weak.update(cx, |this, cx| {
                if crate::handlers::follow_scroll(this, first) {
                    cx.notify();
                }
            });
        });

        let mut expanded = BTreeSet::new();
        expanded.insert(PLUGINS_FOLDER.to_string());

        let game = project_path
            .as_deref()
            .filter(|_| kind == WindowKind::Project)
            .map(crate::utils::game::GameSettings::load);
        let mut this = Self {
            kind,
            project_path,
            game,
            game_pickers: HashMap::new(),
            focus_handle: cx.focus_handle(),
            search,
            query: String::new(),
            all_pages: Vec::new(),
            pages: Vec::new(),
            plugin_generation: pulsar_plugins::generation(),
            expanded,
            tree: Vec::new(),
            tree_scroll: UniformListScrollHandle::new(),
            tree_scrollbar: ScrollbarState::default(),
            selected_page: None,
            selected_section: None,
            rows: Vec::new(),
            headings: Vec::new(),
            content,
            content_scrollbar: ScrollbarState::default(),
            pending_jump: None,
            fields: HashMap::new(),
            has_pending_changes: false,
            restart_required: false,
            _subscriptions: vec![search_subscription],
        };
        crate::handlers::rebuild(&mut this);
        if let Some(page) = initial_page {
            crate::handlers::select_page_titled(&mut this, &page);
        }
        this
    }

    /// The project directory this window edits, if any.
    pub fn project_path(&self) -> Option<&Path> {
        self.project_path.as_deref()
    }

    pub fn kind(&self) -> WindowKind {
        self.kind
    }

    /// Show the page titled `title` (a plugin's name), opening its folder.
    pub fn show_page(&mut self, title: &str, cx: &mut Context<Self>) {
        crate::handlers::select_page_titled(self, title);
        cx.notify();
    }

    /// The selected page among the shown pages.
    pub(crate) fn current_page(&self) -> Option<(usize, &Page)> {
        let id = self.selected_page.as_deref()?;
        self.pages
            .iter()
            .enumerate()
            .find(|(_, page)| page.id == id)
    }
}

impl Focusable for SettingsScreen {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A plugin enabled or disabled since: its pages come and go.
        if pulsar_plugins::generation() != self.plugin_generation {
            crate::handlers::rebuild(self);
        }
        // A section jump onto a page that was just laid out (its row heights
        // are known now, so the jump lands on the heading).
        if let Some(row) = self.pending_jump.take() {
            let content = self.content.clone();
            window.on_next_frame(move |_, _| {
                content.scroll_to(gpui::ListOffset {
                    item_ix: row,
                    offset_in_item: px(0.0),
                });
            });
        }

        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .when(self.has_pending_changes, |this| {
                this.child(render_save_bar(self.restart_required, cx))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .w(px(SIDEBAR_WIDTH))
                            .flex_shrink_0()
                            .h_full()
                            .child(self.render_sidebar(cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_content(window, cx)),
                    ),
            )
    }
}

/// The empty state shown in place of a list.
pub(crate) fn empty_note(text: &'static str, cx: &Context<SettingsScreen>) -> impl IntoElement {
    div()
        .p_6()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

impl SettingsScreen {
    /// The pages the window shows, rebuilt from the settings database.
    pub(crate) fn build_pages(&self) -> Vec<Page> {
        model::build_pages(self.kind)
    }
}
