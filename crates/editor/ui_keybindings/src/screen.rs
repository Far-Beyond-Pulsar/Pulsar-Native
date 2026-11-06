use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, TitleBar,
    input::{InputEvent, InputState},
    scroll::ScrollbarState,
    v_flex,
};

use crate::utils::registry;
use crate::utils::store::KeybindingStore;
use crate::utils::view_model::{self, ActionEntry, Filter};

/// A shortcut being captured for an action.
pub(crate) struct Recorder {
    pub action_name: String,
    pub action_title: String,
    /// The binding being changed, or `None` when adding another.
    pub replacing: Option<crate::utils::Binding>,
    /// Keystrokes captured so far, each in `Keystroke::parse` form.
    pub keys: Vec<String>,
    pub error: Option<String>,
    pub _intercept: Subscription,
}

pub(crate) enum Notice {
    Info(String),
    Error(String),
}

pub struct KeybindingsWindow {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) context_input: Entity<InputState>,
    pub(crate) query: String,
    pub(crate) filter: Filter,
    /// Every action with its live bindings, from the last refresh.
    pub(crate) entries: Vec<ActionEntry>,
    /// Indexes into `entries` that pass the search and filter.
    pub(crate) visible: Vec<usize>,
    pub(crate) list_scroll: UniformListScrollHandle,
    pub(crate) list_scrollbar: ScrollbarState,
    pub(crate) recorder: Option<Recorder>,
    /// The action whose full list of shortcuts is open.
    pub(crate) details: Option<String>,
    pub(crate) notice: Option<Notice>,
    /// "Reset all" was clicked once; the next click does it.
    pub(crate) confirm_reset_all: bool,
}

impl KeybindingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Search actions, areas or shortcuts", window, cx);
            state
        });
        let context_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Everywhere", window, cx);
            state
        });
        cx.subscribe(
            &search_input,
            |this: &mut Self, state, _: &InputEvent, cx| {
                this.query = state.read(cx).value().to_string();
                this.confirm_reset_all = false;
                this.update_visible();
                cx.notify();
            },
        )
        .detach();

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            search_input,
            context_input,
            query: String::new(),
            filter: Filter::All,
            entries: Vec::new(),
            visible: Vec::new(),
            list_scroll: UniformListScrollHandle::new(),
            list_scrollbar: ScrollbarState::default(),
            recorder: None,
            details: None,
            notice: None,
            confirm_reset_all: false,
        };
        this.refresh(cx);
        this
    }

    pub(crate) fn project() -> Option<std::path::PathBuf> {
        engine_state::get_project_path().map(std::path::PathBuf::from)
    }

    /// Rebuild the entries from the live keymap and the store.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let project = Self::project();
        let user = KeybindingStore::for_project(project.as_deref(), cx)
            .overrides
            .clone();
        let actions = registry::actions(cx);
        let defaults = registry::defaults(cx);
        self.entries = view_model::build_entries(&actions, &defaults, &user);
        self.update_visible();
    }

    pub(crate) fn update_visible(&mut self) {
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| view_model::matches(entry, &self.query, self.filter))
            .map(|(ix, _)| ix)
            .collect();
    }

    pub(crate) fn count(&self, filter: Filter) -> usize {
        self.entries
            .iter()
            .filter(|entry| view_model::matches(entry, "", filter))
            .count()
    }
}

impl Focusable for KeybindingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KeybindingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(cx);
        let banner = self.render_banner(cx);
        let list = self.render_list(cx);
        let status = self.render_status_bar(cx);
        let recorder = self.recorder.is_some().then(|| self.render_recorder(window, cx));
        let details = (self.recorder.is_none() && self.details.is_some())
            .then(|| self.render_details(cx));

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .bg(theme.background)
            .child(TitleBar::new().child("Keyboard Shortcuts"))
            .child(toolbar)
            .children(banner)
            .child(list)
            .child(status)
            .children(details)
            .children(recorder)
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for KeybindingsWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "KeybindingsWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1100.0, 760.0)
    }

    fn build(_: (), window: &mut Window, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|cx| KeybindingsWindow::new(window, cx))
    }
}

