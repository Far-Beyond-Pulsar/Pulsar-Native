use super::field_renderers::render_setting_row;
use super::tabs::render_tab_switcher;
use super::{SettingsContainer, SettingsTab};
use engine_state::{global_config, ConfigValue, NS_EDITOR, NS_PROJECT};
use gpui::*;
use gpui::prelude::FluentBuilder as _;
use std::path::PathBuf;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Colorize as _, Disableable as _, Icon, IconName, scroll::ScrollbarAxis, StyledExt as _, IndexPath,
    input::{InputEvent, NumberInputEvent},
    dropdown::{DropdownEvent, DropdownState},
    slider::{SliderEvent, SliderState, SliderValue},
    color_picker::{ColorPickerEvent, ColorPickerState},
};

/// Props for the new settings screen
pub struct SettingsScreenV2Props {
    pub project_path: Option<PathBuf>,
}

/// The new modern settings screen
pub struct SettingsScreenV2 {
    /// Settings container with both global and project settings
    container: SettingsContainer,
    /// Currently active tab
    active_tab: SettingsTab,
    /// Currently selected page within the active tab
    active_page: String,
    /// Pending changes (not yet saved)
    pending_changes: std::collections::HashMap<String, ConfigValue>,
    /// Whether there are unsaved changes
    has_unsaved_changes: bool,
    /// Input states for text/number fields
    input_states: std::collections::HashMap<String, Entity<ui::input::InputState>>,
    /// Dropdown states for dropdown fields
    dropdown_states: std::collections::HashMap<String, Entity<DropdownState<Vec<String>>>>,
    /// Slider states for slider fields
    slider_states: std::collections::HashMap<String, Entity<SliderState>>,
    /// Color picker states for color fields
    color_picker_states: std::collections::HashMap<String, Entity<ColorPickerState>>,
    /// Subscriptions to component events (prevents them from being dropped)
    subscriptions: std::collections::HashMap<String, Vec<Subscription>>,
}

impl SettingsScreenV2 {
    pub fn new(props: SettingsScreenV2Props) -> Self {
        let container = SettingsContainer::new(props.project_path);

        // Get the first page for the active tab
        let active_page = global_config()
            .list_pages(NS_EDITOR)
            .into_iter()
            .next()
            .unwrap_or_else(|| "General".to_string());

        Self {
            container,
            active_tab: SettingsTab::Global,
            active_page,
            pending_changes: std::collections::HashMap::new(),
            has_unsaved_changes: false,
            input_states: std::collections::HashMap::new(),
            dropdown_states: std::collections::HashMap::new(),
            slider_states: std::collections::HashMap::new(),
            color_picker_states: std::collections::HashMap::new(),
            subscriptions: std::collections::HashMap::new(),
        }
    }

    fn get_or_create_input_state(
        &mut self,
        key: &str,
        is_number_field: bool,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Entity<ui::input::InputState> {
        if let Some(state) = self.input_states.get(key) {
            return state.clone();
        }

        // Create new input state
        let state = cx.new(|cx| ui::input::InputState::new(window, cx));

        // Subscribe to input events
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &InputEvent, window, _cx| {
                match event {
                    // On every keystroke: parse and update if valid
                    InputEvent::Change => {
                        let key_clone_inner = key_clone.clone();
                        if let Some(input) = this.input_states.get(&key_clone) {
                            input.read(_cx).text().to_string();
                            let text = input.read(_cx).text().to_string();

                            if is_number_field {
                                // Try to parse as number
                                if let Ok(value) = text.parse::<f64>() {
                                    this.pending_changes.insert(key_clone_inner, ConfigValue::Float(value));
                                    this.has_unsaved_changes = true;
                                }
                            } else {
                                // It's a text field
                                this.pending_changes.insert(key_clone_inner, ConfigValue::String(text));
                                this.has_unsaved_changes = true;
                            }
                        }
                    }

                    // On blur: reformat and ensure value is valid
                    InputEvent::Blur => {
                        let key_clone_inner = key_clone.clone();
                        if let Some(input) = this.input_states.get(&key_clone) {
                            let text = input.read(_cx).text().to_string();

                            if is_number_field {
                                // Parse, validate, and reformat
                                if let Ok(value) = text.parse::<f64>() {
                                    this.pending_changes.insert(key_clone_inner, ConfigValue::Float(value));
                                    this.has_unsaved_changes = true;

                                    // Reformat to canonical form
                                    let formatted = value.to_string();
                                    input.update(_cx, |state, cx| {
                                        state.set_value(&formatted, window, cx);
                                    });
                                }
                            } else {
                                // Text fields don't need reformatting
                                this.pending_changes.insert(key_clone_inner, ConfigValue::String(text));
                                this.has_unsaved_changes = true;
                            }
                        }
                    }

                    _ => {}
                }
            },
        );

        // Store state and subscription
        self.input_states.insert(key.to_string(), state.clone());
        self.subscriptions.entry(key.to_string()).or_insert_with(Vec::new).push(subscription);

        state
    }

    fn get_or_create_dropdown_state(
        &mut self,
        key: &str,
        options: &[engine_state::DropdownOption],
        current_value: &str,
        is_theme: bool,
        window: &mut Window,
        cx: &mut Context<Self>
    ) -> Entity<DropdownState<Vec<String>>> {
        if let Some(state) = self.dropdown_states.get(key) {
            return state.clone();
        }

        // Special handling for theme dropdown - get themes from ThemeRegistry
        let option_values: Vec<String> = if is_theme {
            ui::theme::ThemeRegistry::global(cx)
                .themes()
                .keys()
                .map(|k| k.to_string())
                .collect()
        } else {
            options.iter().map(|opt| opt.value.clone()).collect()
        };

        let selected_index = option_values.iter().position(|opt| opt == current_value)
            .map(|row| IndexPath::default().row(row));

        // Create dropdown state with Vec<String> as delegate
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

                    // Special handling for theme - apply immediately for preview
                    if is_theme {
                        let theme_name = SharedString::from(value.clone());
                        if let Some(theme_config) = ui::theme::ThemeRegistry::global(cx)
                            .themes()
                            .get(&theme_name)
                            .cloned()
                        {
                            ui::theme::Theme::global_mut(cx).apply_config(&theme_config);
                            cx.refresh_windows();
                        }
                    }
                }
            },
        );

        // Store state and subscription
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

        // Create slider state
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
            move |this, _state, event: &SliderEvent, _window, _cx| {
                if let SliderEvent::Change(value) = event {
                    let num_value = match value {
                        SliderValue::Single(v) => *v as f64,
                        SliderValue::Range(_, end) => *end as f64, // Use end value for range
                    };
                    this.pending_changes.insert(key_clone.clone(), ConfigValue::Float(num_value));
                    this.has_unsaved_changes = true;
                }
            },
        );

        // Store state and subscription
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

        // Create color picker state
        let state = cx.new(|cx| {
            ColorPickerState::new(window, cx)
                .default_value(color)
        });

        // Subscribe to color picker events
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &ColorPickerEvent, _window, _cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    let hex = Self::color_to_hex(*color);
                    this.pending_changes.insert(key_clone.clone(), ConfigValue::String(hex));
                    this.has_unsaved_changes = true;
                }
            },
        );

        // Store state and subscription
        self.color_picker_states.insert(key.to_string(), state.clone());
        self.subscriptions.entry(key.to_string()).or_insert_with(Vec::new).push(subscription);

        state
    }

    fn parse_hex_color(hex: &str) -> Hsla {
        Hsla::parse_hex(hex).unwrap_or(Hsla::parse_hex("#000000").unwrap())
    }

    fn color_to_hex(color: Hsla) -> String {
        let rgb = color.to_rgb();
        let r = (rgb.r * 255.0).round() as u8;
        let g = (rgb.g * 255.0).round() as u8;
        let b = (rgb.b * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }

    fn get_current_value(&self, info: &engine_state::SettingInfo, cx: &App) -> ConfigValue {
        // Compound key for pending_changes lookup
        let compound = format!("{}/{}", info.owner, info.key);

        // Check pending changes first
        if let Some(value) = self.pending_changes.get(&compound) {
            return value.clone();
        }

        // Special handling for theme - get from Theme global
        if info.owner == "appearance" && info.key == "theme" {
            let theme = ui::theme::Theme::global(cx);
            let theme_name = match theme.mode {
                ui::theme::ThemeMode::Light => theme.light_theme.name.to_string(),
                ui::theme::ThemeMode::Dark => theme.dark_theme.name.to_string(),
            };
            return ConfigValue::String(theme_name);
        }

        // Return the current value from SettingInfo (already populated by ConfigManager)
        info.current_value.clone()
    }

    fn handle_setting_change(&mut self, owner: String, key: String, value: ConfigValue, cx: &mut Context<Self>) {
        // Store with compound key so owner:key is unique
        self.pending_changes.insert(format!("{}/{}", owner, key), value);
        self.has_unsaved_changes = true;
        cx.notify();
    }

    fn save_all_changes(&mut self, _cx: &mut Context<Self>) {
        tracing::info!("Saving all changes for {:?} settings", self.active_tab);

        let ns = match self.active_tab {
            SettingsTab::Global => NS_EDITOR,
            SettingsTab::Project => NS_PROJECT,
        };

        // Apply all pending changes via ConfigManager owner handles
        for (compound_key, value) in self.pending_changes.drain() {
            if let Some((owner, key)) = compound_key.rsplit_once('/') {
                tracing::debug!("Applying change: {}/{} = {:?}", owner, key, value);
                if let Some(handle) = global_config().owner_handle(ns, owner) {
                    if let Err(e) = handle.set(key, value) {
                        tracing::warn!("Failed to apply setting {}/{}: {}", owner, key, e);
                    }
                }
            }
        }

        // Save to disk
        match self.active_tab {
            SettingsTab::Global => {
                if let Err(e) = self.container.global.save_all() {
                    tracing::error!("Failed to save global settings: {}", e);
                } else {
                    tracing::info!("Successfully saved global settings");
                }
            }
            SettingsTab::Project => {
                if let Some(ref project) = self.container.project {
                    tracing::info!("Saving project settings to: {:?}", project.path());
                    if let Err(e) = project.save_all() {
                        tracing::error!("Failed to save project settings: {}", e);
                    } else {
                        tracing::info!("Successfully saved project settings");
                    }
                } else {
                    tracing::warn!("No project is currently loaded - cannot save project settings");
                }
            }
        }

        self.has_unsaved_changes = false;
    }

    fn discard_changes(&mut self, cx: &mut Context<Self>) {
        self.pending_changes.clear();
        self.has_unsaved_changes = false;
        cx.notify();
    }

    fn switch_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if self.active_tab != tab {
            self.active_tab = tab;

            // Reset to first page of new tab
            let ns = match tab {
                SettingsTab::Global => NS_EDITOR,
                SettingsTab::Project => NS_PROJECT,
            };

            self.active_page = global_config()
                .list_pages(ns)
                .into_iter()
                .next()
                .unwrap_or_else(|| "General".to_string());

            cx.notify();
        }
    }

    fn switch_page(&mut self, page: String, cx: &mut Context<Self>) {
        if self.active_page != page {
            self.active_page = page;
            cx.notify();
        }
    }
}

impl Render for SettingsScreenV2 {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.render_header(cx))
            .child(self.render_tab_switcher(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_sidebar(window, cx))
                    .child(self.render_content(window, cx))
            )
    }
}

impl SettingsScreenV2 {
    fn render_tab_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active_tab = self.active_tab;

        h_flex()
            .w_full()
            .gap_2()
            .p_2()
            .bg(theme.background)
            .border_b_1()
            .border_color(theme.border)
            .child(self.render_tab_button(SettingsTab::Global, cx))
            .child(self.render_tab_button(SettingsTab::Project, cx))
    }

    fn render_tab_button(&self, tab: SettingsTab, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_active = self.active_tab == tab;

        div()
            .flex_1()
            .px_6()
            .py_3()
            .rounded_lg()
            .cursor_pointer()
            .when(is_active, |this| {
                this.bg(theme.primary)
            })
            .when(!is_active, |this| {
                this.bg(theme.secondary.opacity(0.3))
                    .hover(|style| style.bg(theme.secondary.opacity(0.5)))
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |screen, _event, _window, cx| {
                screen.switch_tab(tab, cx);
            }))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(tab.icon())
                            .size(px(20.0))
                            .text_color(if is_active {
                                theme.primary_foreground
                            } else {
                                theme.foreground
                            })
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(if is_active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .text_color(if is_active {
                                        theme.primary_foreground
                                    } else {
                                        theme.foreground
                                    })
                                    .child(tab.label())
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if is_active {
                                        theme.primary_foreground.opacity(0.8)
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(tab.description())
                            )
                    )
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .px_8()
            .py_5()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        div()
                            .w(px(56.0))
                            .h(px(56.0))
                            .rounded_xl()
                            .bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.15))
                            .border_1()
                            .border_color(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Settings)
                                    .size(px(28.0))
                                    .text_color(theme.primary)
                            )
                    )
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_3xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child("Engine Settings")
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Customize your Pulsar Engine experience")
                            )
                    )
            )
            .child(
                h_flex()
                    .gap_3()
                    .when(self.has_unsaved_changes, |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_1p5()
                                .rounded_md()
                                .bg(hsla(theme.warning.h, theme.warning.s, theme.warning.l, 0.15))
                                .border_1()
                                .border_color(hsla(theme.warning.h, theme.warning.s, theme.warning.l, 0.3))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.warning)
                                        .child("Unsaved changes")
                                )
                        )
                    })
                    .child(
                        Button::new("discard")
                            .ghost()
                            .icon(IconName::X)
                            .label("Discard")
                            .disabled(!self.has_unsaved_changes)
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.discard_changes(cx);
                            }))
                    )
                    .child(
                        Button::new("save-all")
                            .primary()
                            .icon(IconName::Check)
                            .label("Save All")
                            .disabled(!self.has_unsaved_changes)
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.save_all_changes(cx);
                            }))
                    )
            )
    }

    fn render_sidebar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let ns = match self.active_tab {
            SettingsTab::Global => NS_EDITOR,
            SettingsTab::Project => NS_PROJECT,
        };

        let pages = global_config().list_pages(ns);

        v_flex()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.muted_foreground)
                            .child("PAGES")
                    )
            )
            .child(
                v_flex()
                    .id("settings-sidebar-pages")
                    .flex_1()
                    .p_3()
                    .gap_1p5()
                    .scrollable(Axis::Vertical)
                    .children(pages.iter().map(|page| {
                        self.render_page_button(page.clone(), cx)
                    }))
            )
    }

    fn render_page_button(&self, page: String, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_active = self.active_page == page;
        let page_label = page.clone();

        div()
            .w_full()
            .px_4()
            .py_3()
            .rounded_lg()
            .cursor_pointer()
            .when(is_active, |this| {
                this.bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.15))
                    .border_1()
                    .border_color(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.3))
            })
            .when(!is_active, |this| {
                this.hover(|style| style.bg(theme.secondary.opacity(0.5)))
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |screen, _event, _window, cx| {
                screen.switch_page(page.clone(), cx);
            }))
            .child(
                div()
                    .text_sm()
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_active {
                        theme.primary
                    } else {
                        theme.foreground
                    })
                    .child(page_label)
            )
    }

    fn render_setting_row_inline(
        &mut self,
        definition: &engine_state::SettingInfo,
        current_value: &ConfigValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use engine_state::FieldType;
        let theme = cx.theme();
        let key = definition.key.clone();

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .p_4()
            .border_b_1()
            .border_color(theme.border)
            .hover(|style| style.bg(theme.secondary.opacity(0.3)))
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(definition.label.clone().unwrap_or_else(|| definition.key.clone()))
                    )
                    .when(!definition.description.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(definition.description.clone())
                        )
                    })
            )
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.render_field_editor(definition, current_value, window, cx))
            )
    }

    fn render_field_editor(
        &mut self,
        definition: &engine_state::SettingInfo,
        current_value: &ConfigValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use ui::input::{TextInput, NumberInput};
        use ui::switch::Switch;
        use ui::button::{Button, ButtonVariants as _};
        use ui::dropdown::Dropdown;
        use ui::slider::Slider;
        use ui::color_picker::ColorPicker;

        let key = definition.key.clone();
        let owner = definition.owner.clone();
        let setting_id = format!("{}/{}", owner, key);
        let is_theme_setting = owner == "appearance" && key == "theme";

        match definition.field_type.as_ref() {
            Some(engine_state::FieldType::Checkbox) => {
                let checked = current_value.as_bool().unwrap_or(false);

                Switch::new(ElementId::Name(key.clone().into()))
                    .checked(checked)
                    .on_click(cx.listener(move |screen, _, _, cx| {
                        screen.handle_setting_change(owner.clone(), key.clone(), ConfigValue::Bool(!checked), cx);
                    }))
                    .into_any_element()
            }

            Some(engine_state::FieldType::NumberInput { min, max, step: _ }) => {
                let value = current_value.as_float()
                    .or_else(|_| current_value.as_int().map(|i| i as f64))
                    .unwrap_or(0.0);
                let min_opt = *min;
                let max_opt = *max;

                let input_state = self.get_or_create_input_state(&setting_id, true, window, cx);
                input_state.update(cx, |state, cx| {
                    let current_text = value.to_string();
                    if state.text().to_string() != current_text {
                        state.set_value(&current_text, window, cx);
                    }
                });

                let theme = cx.theme();

                h_flex()
                    .gap_2()
                    .items_center()
                    .child(NumberInput::new(&input_state))
                    .when(min_opt.is_some() || max_opt.is_some(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "({} - {})",
                                    min_opt.map(|v| v.to_string()).unwrap_or_else(|| "∞".to_string()),
                                    max_opt.map(|v| v.to_string()).unwrap_or_else(|| "∞".to_string())
                                ))
                        )
                    })
                    .into_any_element()
            }

            Some(engine_state::FieldType::TextInput { placeholder: _, multiline: _ }) => {
                let value = current_value.as_str().unwrap_or("").to_string();

                let input_state = self.get_or_create_input_state(&setting_id, false, window, cx);
                input_state.update(cx, |state, cx| {
                    let current_text = value.clone();
                    if state.text().to_string() != current_text {
                        state.set_value(&current_text, window, cx);
                    }
                });

                TextInput::new(&input_state)
                    .w(px(250.0))
                    .into_any_element()
            }

            Some(engine_state::FieldType::Dropdown { options }) => {
                let current_str = current_value.as_str().unwrap_or("");
                let dropdown_state = self.get_or_create_dropdown_state(&setting_id, options, current_str, is_theme_setting, window, cx);
                Dropdown::new(&dropdown_state)
                    .w(px(200.0))
                    .into_any_element()
            }

            Some(engine_state::FieldType::Slider { min, max, step }) => {
                let value = current_value.as_float()
                    .or_else(|_| current_value.as_int().map(|i| i as f64))
                    .unwrap_or(*min);
                let min_val = *min;
                let max_val = *max;
                let step_val = *step;

                let slider_state = self.get_or_create_slider_state(&setting_id, min_val, max_val, step_val, value, window, cx);

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

            Some(engine_state::FieldType::ColorPicker) => {
                let color_str = current_value.as_str().unwrap_or("#000000");
                let color_picker_state = self.get_or_create_color_picker_state(&setting_id, color_str, window, cx);
                ColorPicker::new(&color_picker_state).into_any_element()
            }

            Some(engine_state::FieldType::PathSelector { directory }) => {
                let theme = cx.theme();
                let path = current_value.as_str().unwrap_or("").to_string();
                let is_dir = *directory;

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
                                        if is_dir { "No directory selected".to_string() }
                                        else { "No file selected".to_string() }
                                    } else { path })
                            )
                    )
                    .child(Button::new("browse").ghost().icon(IconName::Folder).label("Browse"))
                    .into_any_element()
            }

            None => div().into_any_element(),
        }
    }

    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let ns = match self.active_tab {
            SettingsTab::Global => NS_EDITOR,
            SettingsTab::Project => NS_PROJECT,
        };

        let settings = global_config().list_settings_by_page(ns, &self.active_page);

        v_flex()
            .flex_1()
            .min_w_0()
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .child(
                        div()
                            .w_full()
                            .px_8()
                            .py_6()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child(self.active_page.clone())
                            )
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .children(settings.iter().map(|info| {
                                let current_value = self.get_current_value(info, cx);
                                self.render_setting_row_inline(info, &current_value, window, cx)
                            }))
                    )
            )
    }
}
