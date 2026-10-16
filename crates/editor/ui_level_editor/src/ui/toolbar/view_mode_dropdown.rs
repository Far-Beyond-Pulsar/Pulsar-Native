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

    pub fn render(state: &Entity<ViewModePickerState>, label: SharedString) -> impl IntoElement {
        ViewModeTrigger {
            state: state.clone(),
            label,
        }
    }
}

#[derive(IntoElement)]
struct ViewModeTrigger {
    state: Entity<ViewModePickerState>,
    label: SharedString,
}

impl RenderOnce for ViewModeTrigger {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let style = window.text_style();
        let font_size = window.rem_size() * 0.875; // Dropdown's small text size.
        let text = window.text_system().shape_line(
            self.label.clone(),
            font_size,
            &[TextRun {
                len: self.label.len(),
                font: style.font(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
                letter_spacing: None,
            }],
            None,
        );
        // Two 8px insets, two 1px borders, a 1rem caret plus its gap.
        let width = text.width.ceil() + px(18.) + window.rem_size();
        div().w(width).h(px(28.)).flex_shrink_0().child(
            Dropdown::new(&self.state)
                .placeholder("Lit")
                .menu_width(px(280.))
                .appearance(false)
                .focus_outline(false)
                .centered_trigger(true)
                .small()
                .w(width)
                .h(px(28.)),
        )
    }
}
