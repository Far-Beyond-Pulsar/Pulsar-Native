//! Modern database-driven settings UI.

use engine_state::{
    global_config, ConfigValue, DropdownOption, FieldType, GlobalSettings, NS_EDITOR, NS_PROJECT,
    ProjectSettings, SettingInfo,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::collections::HashMap;
use std::path::PathBuf;
use ui::{
    button::{Button, ButtonVariants as _},
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    dropdown::{Dropdown, DropdownEvent, DropdownState},
    h_flex,
    input::{InputEvent, InputState, TextInput},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    switch::Switch,
    v_flex, ActiveTheme, Colorize as _, Disableable as _, Icon, IconName, IndexPath,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsTab {
    Global,
    Project,
}

impl SettingsTab {
    fn label(&self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Project => "Project",
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::Global => IconName::Settings,
            Self::Project => IconName::Folder,
        }
    }

    fn namespace(&self) -> &'static str {
        match self {
            Self::Global => NS_EDITOR,
            Self::Project => NS_PROJECT,
        }
    }
}

pub struct ModernSettingsScreen {
    active_tab: SettingsTab,
    active_page: String,
    search_query: String,
    search_input: Entity<InputState>,
    pending_changes: HashMap<String, ConfigValue>,
    has_unsaved_changes: bool,
    global_backend: GlobalSettings,
    project_backend: Option<ProjectSettings>,
    input_states: HashMap<String, Entity<InputState>>,
    dropdown_states: HashMap<String, Entity<DropdownState<Vec<String>>>>,
    slider_states: HashMap<String, Entity<SliderState>>,
    color_picker_states: HashMap<String, Entity<ColorPickerState>>,
    subscriptions: HashMap<String, Vec<Subscription>>,
}

impl ModernSettingsScreen {
    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        engine_state::register_default_settings();

        let search_input = cx.new(|cx| InputState::new(window, cx));
        let active_page = global_config()
            .list_pages(NS_EDITOR)
            .into_iter()
            .next()
            .unwrap_or_else(|| "General".to_string());

        let mut subscriptions = HashMap::new();
        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |this, _state, event: &InputEvent, _window, cx| {
                match event {
                    InputEvent::Change | InputEvent::Blur => {
                        this.search_query = this.search_input.read(cx).text().to_string();
                        cx.notify();
                    }
                    _ => {}
                }
            },
        );
        subscriptions.insert("search".to_string(), vec![search_subscription]);

        Self {
            active_tab: SettingsTab::Global,
            active_page,
            search_query: String::new(),
            search_input,
            pending_changes: HashMap::new(),
            has_unsaved_changes: false,
            global_backend: GlobalSettings::new(),
            project_backend: project_path.as_deref().map(ProjectSettings::new),
            input_states: HashMap::new(),
            dropdown_states: HashMap::new(),
            slider_states: HashMap::new(),
            color_picker_states: HashMap::new(),
            subscriptions,
        }
    }

    fn make_setting_id(namespace: &str, owner: &str, key: &str) -> String {
        format!("{}/{}/{}", namespace, owner, key)
    }

    fn active_pages(&self) -> Vec<String> {
        global_config().list_pages(self.active_tab.namespace())
    }

    fn get_current_value(&self, info: &SettingInfo, cx: &App) -> ConfigValue {
        let setting_id = Self::make_setting_id(&info.namespace, &info.owner, &info.key);
        if let Some(value) = self.pending_changes.get(&setting_id) {
            return value.clone();
        }

        if info.namespace == NS_EDITOR && info.owner == "appearance" && info.key == "theme" {
            let theme = ui::theme::Theme::global(cx);
            let theme_name = match theme.mode {
                ui::theme::ThemeMode::Light => theme.light_theme.name.to_string(),
                ui::theme::ThemeMode::Dark => theme.dark_theme.name.to_string(),
            };
            return ConfigValue::String(theme_name);
        }

        info.current_value.clone()
    }

    fn switch_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if self.active_tab == tab {
            return;
        }

        self.active_tab = tab;
        self.active_page = self
            .active_pages()
            .into_iter()
            .next()
            .unwrap_or_else(|| "General".to_string());
        cx.notify();
    }

    fn switch_page(&mut self, page: String, cx: &mut Context<Self>) {
        if self.active_page != page {
            self.active_page = page;
            cx.notify();
        }
    }

    fn save_all_changes(&mut self, cx: &mut Context<Self>) {
        for (setting_id, value) in self.pending_changes.drain() {
            if let Some((namespace, rest)) = setting_id.split_once('/') {
                if let Some((owner, key)) = rest.rsplit_once('/') {
                    if let Some(handle) = global_config().owner_handle(namespace, owner) {
                        let _ = handle.set(key, value);
                    }
                }
            }
        }

        let _ = self.global_backend.save_all();
        if let Some(project) = &self.project_backend {
            let _ = project.save_all();
        }

        self.has_unsaved_changes = false;
        cx.notify();
    }

    fn discard_changes(&mut self, cx: &mut Context<Self>) {
        self.pending_changes.clear();
        self.has_unsaved_changes = false;
        cx.notify();
    }

    fn get_or_create_input_state(
        &mut self,
        key: &str,
        is_number_field: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(state) = self.input_states.get(key) {
            return state.clone();
        }

        let state = cx.new(|cx| InputState::new(window, cx));
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &InputEvent, _window, cx| {
                match event {
                    InputEvent::Change | InputEvent::Blur => {
                        if let Some(input) = this.input_states.get(&key_clone) {
                            let text = input.read(cx).text().to_string();
                            if is_number_field {
                                if let Ok(value) = text.parse::<f64>() {
                                    this.pending_changes
                                        .insert(key_clone.clone(), ConfigValue::Float(value));
                                    this.has_unsaved_changes = true;
                                }
                            } else {
                                this.pending_changes
                                    .insert(key_clone.clone(), ConfigValue::String(text));
                                this.has_unsaved_changes = true;
                            }
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            },
        );

        self.input_states.insert(key.to_string(), state.clone());
        self.subscriptions
            .entry(key.to_string())
            .or_default()
            .push(subscription);

        state
    }

    fn get_or_create_dropdown_state(
        &mut self,
        key: &str,
        options: &[DropdownOption],
        current_value: &str,
        is_theme: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<DropdownState<Vec<String>>> {
        if let Some(state) = self.dropdown_states.get(key) {
            return state.clone();
        }

        let option_values: Vec<String> = if is_theme {
            ui::theme::ThemeRegistry::global(cx)
                .themes()
                .keys()
                .map(|k| k.to_string())
                .collect()
        } else {
            options.iter().map(|opt| opt.value.clone()).collect()
        };

        let selected_index = option_values
            .iter()
            .position(|v| v == current_value)
            .map(|row| IndexPath::default().row(row));

        let state = cx.new(|cx| DropdownState::new(option_values, selected_index, window, cx));
        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &DropdownEvent<Vec<String>>, _window, cx| {
                if let DropdownEvent::Confirm(Some(value)) = event {
                    this.pending_changes
                        .insert(key_clone.clone(), ConfigValue::String(value.clone()));
                    this.has_unsaved_changes = true;

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

                    cx.notify();
                }
            },
        );

        self.dropdown_states.insert(key.to_string(), state.clone());
        self.subscriptions
            .entry(key.to_string())
            .or_default()
            .push(subscription);

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
        cx: &mut Context<Self>,
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

        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &SliderEvent, _window, cx| {
                if let SliderEvent::Change(value) = event {
                    let numeric = match value {
                        SliderValue::Single(v) => *v as f64,
                        SliderValue::Range(_, end) => *end as f64,
                    };
                    this.pending_changes
                        .insert(key_clone.clone(), ConfigValue::Float(numeric));
                    this.has_unsaved_changes = true;
                    cx.notify();
                }
            },
        );

        self.slider_states.insert(key.to_string(), state.clone());
        self.subscriptions
            .entry(key.to_string())
            .or_default()
            .push(subscription);

        state
    }

    fn get_or_create_color_picker_state(
        &mut self,
        key: &str,
        current_value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPickerState> {
        if let Some(state) = self.color_picker_states.get(key) {
            return state.clone();
        }

        let color = Self::parse_hex_color(current_value);
        let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(color));

        let key_clone = key.to_string();
        let subscription = cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &ColorPickerEvent, _window, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    let hex = Self::color_to_hex(*color);
                    this.pending_changes
                        .insert(key_clone.clone(), ConfigValue::String(hex));
                    this.has_unsaved_changes = true;
                    cx.notify();
                }
            },
        );

        self.color_picker_states.insert(key.to_string(), state.clone());
        self.subscriptions
            .entry(key.to_string())
            .or_default()
            .push(subscription);

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

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let global_tab = self.render_tab_button(SettingsTab::Global, cx);
        let project_tab = self.render_tab_button(SettingsTab::Project, cx);
        let theme = cx.theme();
        let pages = self.active_pages();

        v_flex()
            .w(px(260.0))
            .h_full()
            .bg(theme.background.opacity(0.5))
            .border_r_1()
            .border_color(theme.border)
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(global_tab)
                    .child(project_tab),
            )
            .child(
                div().w_full().child(
                    TextInput::new(&self.search_input)
                        .prefix(Icon::new(IconName::Search)),
                ),
            )
            .child(
                div()
                    .id("settings-modern-sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(v_flex().gap_1().children(pages.into_iter().map(|page| {
                        let is_active = self.active_page == page;
                        let page_name = page.clone();
                        div()
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .when(is_active, |el| el.bg(theme.accent.opacity(0.15)))
                            .hover(|el| el.bg(theme.muted.opacity(0.1)))
                            .cursor_pointer()
                            .child(div().text_sm().when(is_active, |el| el.font_weight(FontWeight::SEMIBOLD)).child(page.clone()))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                this.switch_page(page_name.clone(), cx);
                            }))
                    }))),
            )
            .child(
                div()
                    .mt_auto()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Database-backed settings view"),
                    ),
            )
    }

    fn render_tab_button(&self, tab: SettingsTab, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active_tab == tab;

        h_flex()
            .gap_1()
            .items_center()
            .px_3()
            .py_1p5()
            .rounded_md()
            .when(active, |el| el.bg(theme.accent.opacity(0.16)))
            .hover(|el| el.bg(theme.muted.opacity(0.1)))
            .cursor_pointer()
            .child(Icon::new(tab.icon()).size_3())
            .child(div().text_sm().when(active, |el| el.font_weight(FontWeight::SEMIBOLD)).child(tab.label()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.switch_tab(tab, cx);
            }))
    }

    fn render_setting_row(
        &mut self,
        definition: &SettingInfo,
        current_value: &ConfigValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .w_full()
            .p_3()
            .gap_4()
            .items_start()
            .border_b_1()
            .border_color(theme.border.opacity(0.3))
            .hover(|el| el.bg(theme.muted.opacity(0.08)))
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(definition.label.clone().unwrap_or_else(|| definition.key.clone())),
                    )
                    .when(!definition.description.is_empty(), |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(definition.description.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .min_w(px(220.0))
                    .child(self.render_setting_control(definition, current_value, window, cx)),
            )
    }

    fn render_setting_control(
        &mut self,
        definition: &SettingInfo,
        current_value: &ConfigValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let setting_id = Self::make_setting_id(&definition.namespace, &definition.owner, &definition.key);

        match definition.field_type.as_ref() {
            Some(FieldType::Checkbox) => {
                let checked = current_value.as_bool().unwrap_or(false);
                let setting_id_clone = setting_id.clone();
                Switch::new(SharedString::from(format!("switch_{}", setting_id)))
                    .checked(checked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pending_changes
                            .insert(setting_id_clone.clone(), ConfigValue::Bool(!checked));
                        this.has_unsaved_changes = true;
                        cx.notify();
                    }))
                    .into_any_element()
            }

            Some(FieldType::TextInput { .. }) => {
                let val = current_value.as_str().unwrap_or("").to_string();
                let input_state = self.get_or_create_input_state(&setting_id, false, window, cx);
                input_state.update(cx, |state, cx| {
                    if state.text().to_string() != val {
                        state.set_value(&val, window, cx);
                    }
                });
                TextInput::new(&input_state).into_any_element()
            }

            Some(FieldType::NumberInput { .. }) => {
                let val = current_value
                    .as_float()
                    .or_else(|_| current_value.as_int().map(|i| i as f64))
                    .unwrap_or(0.0)
                    .to_string();
                let input_state = self.get_or_create_input_state(&setting_id, true, window, cx);
                input_state.update(cx, |state, cx| {
                    if state.text().to_string() != val {
                        state.set_value(&val, window, cx);
                    }
                });
                TextInput::new(&input_state).into_any_element()
            }

            Some(FieldType::Dropdown { options }) => {
                let current_str = current_value.as_str().unwrap_or("");
                let is_theme = definition.namespace == NS_EDITOR
                    && definition.owner == "appearance"
                    && definition.key == "theme";
                let dropdown_state = self.get_or_create_dropdown_state(
                    &setting_id,
                    options,
                    current_str,
                    is_theme,
                    window,
                    cx,
                );
                Dropdown::new(&dropdown_state).w(px(220.0)).into_any_element()
            }

            Some(FieldType::Slider { min, max, step }) => {
                let value = current_value
                    .as_float()
                    .or_else(|_| current_value.as_int().map(|i| i as f64))
                    .unwrap_or(*min);
                let (min_val, max_val, step_val) = (*min, *max, *step);
                let slider_state =
                    self.get_or_create_slider_state(&setting_id, min_val, max_val, step_val, value, window, cx);
                let theme = cx.theme();

                v_flex()
                    .gap_2()
                    .min_w(px(220.0))
                    .child(Slider::new(&slider_state).horizontal())
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{}", min_val)))
                            .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme.foreground).child(format!("{:.1}", value)))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{}", max_val))),
                    )
                    .into_any_element()
            }

            Some(FieldType::ColorPicker) => {
                let color_str = current_value.as_str().unwrap_or("#000000");
                let color_picker_state =
                    self.get_or_create_color_picker_state(&setting_id, color_str, window, cx);
                ColorPicker::new(&color_picker_state).into_any_element()
            }

            Some(FieldType::PathSelector { directory }) => {
                let path = current_value.as_str().unwrap_or("").to_string();
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
                                    .text_color(if path.is_empty() {
                                        theme.muted_foreground
                                    } else {
                                        theme.foreground
                                    })
                                    .child(if path.is_empty() {
                                        if is_dir {
                                            "No directory selected".to_string()
                                        } else {
                                            "No file selected".to_string()
                                        }
                                    } else {
                                        path
                                    }),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("browse_{}", setting_id)))
                            .label("Browse...")
                            .ghost(),
                    )
                    .into_any_element()
            }

            None => div().into_any_element(),
        }
    }

    fn render_content_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let namespace = self.active_tab.namespace();

        let mut settings = global_config().list_settings_by_page(namespace, &self.active_page);
        if !self.search_query.trim().is_empty() {
            let q = self.search_query.to_lowercase();
            settings.retain(|s| {
                s.key.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
                    || s.label
                        .as_ref()
                        .map(|l| l.to_lowercase().contains(&q))
                        .unwrap_or(false)
            });
        }

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
                            .child(Icon::new(self.active_tab.icon()).size_5().text_color(theme.foreground))
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child(self.active_page.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("discard-settings")
                                    .label("Discard")
                                    .disabled(!self.has_unsaved_changes)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discard_changes(cx);
                                    })),
                            )
                            .child(
                                Button::new("save-settings")
                                    .label("Save")
                                    .disabled(!self.has_unsaved_changes)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_all_changes(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("settings-modern-content-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(v_flex().w_full().children(if settings.is_empty() {
                        vec![
                            div()
                                .p_8()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .child(Icon::new(IconName::Search).size_8().text_color(theme.muted_foreground))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child("No settings found"),
                                )
                                .into_any_element(),
                        ]
                    } else {
                        settings
                            .iter()
                            .map(|definition| {
                                let current_value = self.get_current_value(definition, cx);
                                self.render_setting_row(definition, &current_value, window, cx)
                                    .into_any_element()
                            })
                            .collect()
                    })),
            )
    }
}

#[derive(Clone)]
pub struct SettingChanged {
    pub key: String,
    pub value: ConfigValue,
}

impl EventEmitter<SettingChanged> for ModernSettingsScreen {}

impl Render for ModernSettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .child(self.render_sidebar(cx))
            .child(self.render_content_area(window, cx))
    }
}
