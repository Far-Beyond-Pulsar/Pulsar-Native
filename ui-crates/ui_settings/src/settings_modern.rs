//! Modern Professional Settings UI - Redesigned to match engine style

use engine_state::{global_config, SettingInfo, ConfigValue, FieldType, DropdownOption};
use gpui::*;
use gpui::prelude::FluentBuilder as _;
use std::path::PathBuf;
use std::collections::HashMap;
use ui::{
    h_flex, v_flex, ActiveTheme, Icon, IconName, IndexPath,
    button::{Button, ButtonVariants as _},
    input::{TextInput, InputState},
    switch::Switch,
    dropdown::{Dropdown, DropdownEvent, DropdownState},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
};

// ── Settings Categories ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsCategory {
    General,
    Appearance,
    Editor,
    Project,
    Performance,
    Network,
    Advanced,
}

impl SettingsCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Editor => "Editor",
            Self::Project => "Project",
            Self::Performance => "Performance",
            Self::Network => "Network",
            Self::Advanced => "Advanced",
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Editor => IconName::Code,
            Self::Project => IconName::Folder,
            Self::Performance => IconName::Activity,
            Self::Network => IconName::Wifi,
            Self::Advanced => IconName::Settings2,
        }
    }

    fn all() -> Vec<Self> {
        vec![
            Self::General,
            Self::Appearance,
            Self::Editor,
            Self::Project,
            Self::Performance,
            Self::Network,
            Self::Advanced,
        ]
    }
}

// ── Modern Settings Screen ──────────────────────────────────────────────────

pub struct ModernSettingsScreen {
    active_category: SettingsCategory,
    search_query: String,
    search_input: Entity<InputState>,
    _project_path: Option<PathBuf>,
    pending_changes: HashMap<String, ConfigValue>,
    has_unsaved_changes: bool,
    /// Cache input states to avoid recreating on every render
    input_states: HashMap<String, Entity<InputState>>,
    /// Cache filtered settings for current category
    cached_settings: Option<(SettingsCategory, Vec<SettingInfo>)>,
    /// Cache dropdown states
    dropdown_states: HashMap<String, Entity<DropdownState<Vec<String>>>>,
    /// Cache slider states
    slider_states: HashMap<String, Entity<SliderState>>,
    /// Cache color picker states
    color_picker_states: HashMap<String, Entity<ColorPickerState>>,
    /// Event subscriptions
    subscriptions: HashMap<String, Vec<Subscription>>,
}

impl ModernSettingsScreen {
    pub fn new(_project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        engine_state::register_default_settings();
        let search_input = cx.new(|cx| InputState::new(window, cx));

        Self {
            active_category: SettingsCategory::General,
            search_query: String::new(),
            search_input,
            _project_path,
            pending_changes: HashMap::new(),
            has_unsaved_changes: false,
            input_states: HashMap::new(),
            cached_settings: None,
            dropdown_states: HashMap::new(),
            slider_states: HashMap::new(),
            color_picker_states: HashMap::new(),
            subscriptions: HashMap::new(),
        }
    }

    fn get_settings_for_category(&mut self, category: SettingsCategory) -> Vec<SettingInfo> {
        // Check cache first
        if let Some((cached_cat, ref settings)) = self.cached_settings {
            if cached_cat == category {
                return settings.clone();
            }
        }

        // Cache miss - fetch and filter by page (which maps to category label)
        let page_label = category.label();
        let all_settings = global_config().list_all_settings();

        let filtered: Vec<SettingInfo> = all_settings
            .into_iter()
            .filter(|info| {
                info.page.as_deref() == Some(page_label)
            })
            .collect();

        // Update cache
        self.cached_settings = Some((category, filtered.clone()));
        filtered
    }

    fn get_or_create_dropdown_state(
        &mut self,
        key: &str,
        options: &[DropdownOption],
        current_value: &str,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Entity<DropdownState<Vec<String>>> {
        if let Some(state) = self.dropdown_states.get(key) {
            return state.clone();
        }

        let option_values: Vec<String> = options.iter().map(|opt| opt.value.clone()).collect();
        let selected_index = option_values.iter()
            .position(|v| v == current_value)
            .map(|row| IndexPath::default().row(row));

        let state = cx.new(|cx| DropdownState::new(option_values, selected_index, window, cx));

        // Subscribe to dropdown events
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &DropdownEvent<Vec<String>>, _window, cx| {
                if let DropdownEvent::Confirm(Some(value)) = event {
                    this.pending_changes.insert(key_clone.clone(), ConfigValue::String(value.clone()));
                    this.has_unsaved_changes = true;
                    cx.notify();
                }
            },
        );

        self.dropdown_states.insert(key.to_string(), state.clone());
        self.subscriptions.entry(key.to_string()).or_insert_with(Vec::new).push(subscription);

        state
    }

    fn get_or_create_slider_state(
        &mut self,
        key: &str,
        min: f64,
        max: f64,
        step: f64,
        current_value: f64,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Entity<SliderState> {
        if let Some(state) = self.slider_states.get(key) {
            return state.clone();
        }

        let state = cx.new(|_cx| {
            SliderState::new()
                .min(min as f32)
                .max(max as f32)
                .step(step as f32)
                .default_value(current_value as f32)
        });

        // Subscribe to slider events
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &SliderEvent, _window, cx| {
                let SliderEvent::Change(value) = event;
                let num_value = match value {
                    SliderValue::Single(v) => *v as f64,
                    SliderValue::Range(_, end) => *end as f64,
                };
                this.pending_changes.insert(key_clone.clone(), ConfigValue::Float(num_value));
                this.has_unsaved_changes = true;
                cx.notify();
            },
        );

        self.slider_states.insert(key.to_string(), state.clone());
        self.subscriptions.entry(key.to_string()).or_insert_with(Vec::new).push(subscription);

        state
    }

    fn get_or_create_color_picker_state(
        &mut self,
        key: &str,
        current_value: &str,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Entity<ColorPickerState> {
        if let Some(state) = self.color_picker_states.get(key) {
            return state.clone();
        }

        // Parse color from hex string
        let color = Self::parse_hex_color(current_value);

        let state = cx.new(|cx| {
            ColorPickerState::new(window, cx)
                .default_value(color)
        });

        // Subscribe to color picker events
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &ColorPickerEvent, _window, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    let hex = Self::color_to_hex(*color);
                    this.pending_changes.insert(key_clone.clone(), ConfigValue::String(hex));
                    this.has_unsaved_changes = true;
                    cx.notify();
                }
            },
        );

        self.color_picker_states.insert(key.to_string(), state.clone());
        self.subscriptions.entry(key.to_string()).or_insert_with(Vec::new).push(subscription);

        state
    }

    fn parse_hex_color(hex: &str) -> Hsla {
        let hex = hex.trim_start_matches('#');
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Hsla::from(Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        })
    }

    fn color_to_hex(color: Hsla) -> String {
        let rgba: Rgba = color.into();
        format!(
            "#{:02X}{:02X}{:02X}",
            (rgba.r * 255.0) as u8,
            (rgba.g * 255.0) as u8,
            (rgba.b * 255.0) as u8
        )
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .w(px(220.0))
            .h_full()
            .bg(theme.background.opacity(0.5))
            .border_r_1()
            .border_color(theme.border)
            .p_3()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .mb_3()
                    .child(
                        TextInput::new(&self.search_input)
                            .prefix(Icon::new(IconName::Search))
                    )
            )
            .child(
                div()
                    .id("settings-sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap_1()
                            .children(SettingsCategory::all().into_iter().map(|category| {
                        let is_active = self.active_category == category;
                        
                        div()
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .when(is_active, |el| el.bg(theme.accent.opacity(0.15)))
                            .hover(|el| el.bg(theme.muted.opacity(0.1)))
                            .cursor_pointer()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Icon::new(category.icon()).size_4())
                                    .child(
                                        div()
                                            .text_sm()
                                            .when(is_active, |el| el.font_weight(FontWeight::SEMIBOLD))
                                            .child(category.label())
                                    )
                            )
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _event, _window, cx| {
                                this.active_category = category;
                                cx.notify();
                            }))
                    }))                    )            )
            .child(
                v_flex()
                    .mt_auto()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .gap_2()
                    .child(
                        div()
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .hover(|el| el.bg(theme.muted.opacity(0.1)))
                            .cursor_pointer()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Icon::new(IconName::Refresh).size_4())
                                    .child(div().text_sm().child("Reset Defaults"))
                            )
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                this.has_unsaved_changes = false;
                                cx.notify();
                            }))
                    )
            )
    }

    fn render_setting_row(&mut self, definition: SettingInfo, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .w_full()
            .p_3()
            .gap_4()
            .items_start()
            .border_b_1()
            .border_color(theme.border.opacity(0.3))
            .hover(|el| el.bg(theme.muted.opacity(0.1)))
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(definition.label.clone().unwrap_or_else(|| definition.key.clone()))
                    )
                    .when(!definition.description.is_empty(), |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(definition.description.clone())
                        )
                    })
            )
            .child(
                div()
                    .min_w(px(200.0))
                    .child(self.render_setting_control(definition, window, cx))
            )
    }

    fn render_setting_control(&mut self, definition: SettingInfo, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match definition.field_type.as_ref() {
            Some(FieldType::Checkbox) => {
                let checked = definition.current_value.as_bool().unwrap_or(false);
                div().child(Switch::new(SharedString::from(format!("switch_{}", definition.key))).checked(checked)).into_any_element()
            }

            Some(FieldType::TextInput { .. }) => {
                let val = definition.current_value.as_str().unwrap_or("").to_string();
                let input_state = self.input_states.entry(definition.key.clone()).or_insert_with(|| {
                    cx.new(|cx| {
                        let mut state = InputState::new(window, cx);
                        state.set_value(&val, window, cx);
                        state
                    })
                }).clone();
                div().child(TextInput::new(&input_state)).into_any_element()
            }

            Some(FieldType::NumberInput { .. }) => {
                let val = definition.current_value.as_float()
                    .or_else(|_| definition.current_value.as_int().map(|i| i as f64))
                    .unwrap_or(0.0)
                    .to_string();
                let input_state = self.input_states.entry(definition.key.clone()).or_insert_with(|| {
                    cx.new(|cx| {
                        let mut state = InputState::new(window, cx);
                        state.set_value(&val, window, cx);
                        state
                    })
                }).clone();
                div().child(TextInput::new(&input_state)).into_any_element()
            }

            Some(FieldType::Dropdown { options }) => {
                let current_str = definition.current_value.as_str().unwrap_or("");
                let dropdown_state = self.get_or_create_dropdown_state(&definition.key, options, current_str, window, cx);
                div().child(Dropdown::new(&dropdown_state).w(px(200.0))).into_any_element()
            }

            Some(FieldType::Slider { min, max, step }) => {
                let value = definition.current_value.as_float()
                    .or_else(|_| definition.current_value.as_int().map(|i| i as f64))
                    .unwrap_or(*min);
                let (min_val, max_val, step_val) = (*min, *max, *step);
                let slider_state = self.get_or_create_slider_state(&definition.key, min_val, max_val, step_val, value, window, cx);
                let theme = cx.theme();
                v_flex()
                    .gap_2()
                    .min_w(px(200.0))
                    .child(Slider::new(&slider_state).horizontal())
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{}", min_val)))
                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme.foreground).child(format!("{:.1}", value)))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{}", max_val)))
                    )
                    .into_any_element()
            }

            Some(FieldType::ColorPicker) => {
                let color_str = definition.current_value.as_str().unwrap_or("#000000");
                let color_picker_state = self.get_or_create_color_picker_state(&definition.key, color_str, window, cx);
                div().child(ColorPicker::new(&color_picker_state)).into_any_element()
            }

            Some(FieldType::PathSelector { directory }) => {
                let path = definition.current_value.as_str().unwrap_or("").to_string();
                let is_dir = *directory;
                let theme = cx.theme();
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(if path.is_empty() { theme.muted_foreground } else { theme.foreground })
                                    .child(if path.is_empty() {
                                        if is_dir { "No directory selected".to_string() } else { "No file selected".to_string() }
                                    } else { path.clone() })
                            )
                    )
                    .child(Button::new(SharedString::from(format!("browse_{}", definition.key))).label("Browse...").ghost())
                    .into_any_element()
            }

            None => div().into_any_element(),
        }
    }

    fn render_content_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let settings = self.get_settings_for_category(self.active_category);

        v_flex()
            .flex_1()
            .h_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .w_full()
                    .p_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(Icon::new(self.active_category.icon()).size_5().text_color(theme.foreground))
                            .child(div().text_xl().font_weight(FontWeight::BOLD).text_color(theme.foreground).child(self.active_category.label()))
                    )
            )
            .child(
                div()
                    .id("settings-content-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .w_full()
                            .children(
                                if settings.is_empty() {
                                    vec![
                                        div()
                                            .p_8()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .justify_center()
                                            .gap_2()
                                            .child(Icon::new(IconName::Search).size_8().text_color(theme.muted_foreground))
                                            .child(div().text_sm().text_color(theme.muted_foreground).child("No settings found"))
                                            .into_any_element()
                                    ]
                                } else {
                                    settings.into_iter().map(|definition| {
                                        self.render_setting_row(definition, window, cx).into_any_element()
                                    }).collect()
                                }
                            )
                    )
            )
    }
}

// ── Events ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SettingChanged {
    pub key: String,
    pub value: ConfigValue,
}

impl EventEmitter<SettingChanged> for ModernSettingsScreen {}

// ── Render ──────────────────────────────────────────────────────────────────

impl Render for ModernSettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .child(self.render_sidebar(cx))
            .child(self.render_content_area(window, cx))
    }
}
