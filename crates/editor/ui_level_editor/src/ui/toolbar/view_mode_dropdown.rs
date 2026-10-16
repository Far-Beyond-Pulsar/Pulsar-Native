use gpui::*;
use ui::{
    dropdown::{Dropdown, DropdownItem, DropdownState, SearchableVec},
    IndexPath, Sizable,
};

/// The selected renderer view mode, either lit output or a registered debug view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewModeSelection {
    Lit,
    Debug(u32),
}

/// A searchable option for the viewport view-mode picker.
#[derive(Clone)]
pub struct ViewModeOption {
    pub selection: ViewModeSelection,
    pub name: SharedString,
}

impl DropdownItem for ViewModeOption {
    type Value = ViewModeSelection;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.selection
    }
}

pub type ViewModePickerState = DropdownState<SearchableVec<ViewModeOption>>;

/// Selects the lit viewport image or a debug view registered by a Helio pass.
pub struct ViewModeDropdown;

impl ViewModeDropdown {
    pub fn options(
        debug_views: &[engine_backend::subsystems::render::ViewportDebugView],
    ) -> Vec<ViewModeOption> {
        let mut options = vec![ViewModeOption {
            selection: ViewModeSelection::Lit,
            name: "Lit".into(),
        }];
        options.extend(debug_views.iter().map(|view| ViewModeOption {
            selection: ViewModeSelection::Debug(view.debug_mode),
            name: view.name.into(),
        }));
        options
    }

    pub fn selected(debug_mode: u32) -> ViewModeSelection {
        if debug_mode == 0 {
            ViewModeSelection::Lit
        } else {
            ViewModeSelection::Debug(debug_mode)
        }
    }

    pub fn new_state(
        window: &mut Window,
        cx: &mut App,
        debug_views: &[engine_backend::subsystems::render::ViewportDebugView],
        selected_mode: ViewModeSelection,
    ) -> Entity<ViewModePickerState> {
        let options = Self::options(debug_views);
        let selected_index = options
            .iter()
            .position(|option| option.selection == selected_mode)
            .unwrap_or(0);

        cx.new(|cx| {
            DropdownState::new(
                SearchableVec::new(options),
                Some(IndexPath::new(selected_index)),
                window,
                cx,
            )
        })
    }

    pub fn render(state: &Entity<ViewModePickerState>) -> impl IntoElement {
        Dropdown::new(state)
            .placeholder("Lit")
            .menu_width(px(280.))
            .appearance(false)
            .focus_outline(false)
            .centered_trigger(true)
            .small()
            .w(px(150.))
            .h(px(28.))
    }
}
