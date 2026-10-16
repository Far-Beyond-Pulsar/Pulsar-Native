use gpui::*;
use ui::{
    IndexPath, Sizable,
    dropdown::{Dropdown, DropdownItem, DropdownState, SearchableVec},
};

/// A searchable option for the viewport view-mode picker.
#[derive(Clone)]
pub struct ViewModeOption {
    pub debug_mode: u32,
    pub name: SharedString,
}

impl DropdownItem for ViewModeOption {
    type Value = u32;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.debug_mode
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
            debug_mode: 0,
            name: "Lit".into(),
        }];
        options.extend(debug_views.iter().map(|view| ViewModeOption {
            debug_mode: view.debug_mode,
            name: view.name.into(),
        }));
        options
    }

    pub fn new_state(
        window: &mut Window,
        cx: &mut App,
        debug_views: &[engine_backend::subsystems::render::ViewportDebugView],
        selected_mode: u32,
    ) -> Entity<ViewModePickerState> {
        let options = Self::options(debug_views);
        let selected_index = options
            .iter()
            .position(|option| option.debug_mode == selected_mode)
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
