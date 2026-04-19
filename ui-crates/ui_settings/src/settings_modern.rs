use engine_state::{
    global_config, ConfigValue, DropdownOption, FieldType, GlobalSettings, NS_EDITOR, NS_PROJECT,
    ProjectSettings, SettingInfo,
};
use gpui::*;
use std::path::PathBuf;
use ui::{
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    ActiveTheme,
};

pub struct ModernSettingsScreen {
    project_path: Option<PathBuf>,
}

impl ModernSettingsScreen {
    pub fn new(project_path: Option<PathBuf>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        engine_state::register_default_settings();

        let global = GlobalSettings::new();
        global.load_all();

        if let Some(path) = project_path.as_deref() {
            ProjectSettings::new(path).load_all();
        }

        Self { project_path }
    }

    fn read_value(info: &SettingInfo) -> ConfigValue {
        global_config()
            .get(&info.namespace, &info.owner, &info.key)
            .ok()
            .unwrap_or_else(|| info.current_value.clone())
    }

    fn write_value(info: &SettingInfo, value: ConfigValue, project_path: &Option<PathBuf>) {
        if let Some(handle) = global_config().owner_handle(&info.namespace, &info.owner) {
            let _ = handle.set(&info.key, value);

            if info.namespace == NS_PROJECT {
                if let Some(path) = project_path.as_deref() {
                    let _ = ProjectSettings::new(path).save_all();
                }
            } else {
                let _ = GlobalSettings::new().save_all();
            }
        }
    }

    fn pretty_owner_path(owner: &str) -> String {
        owner
            .split('/')
            .map(|segment| {
                segment
                    .split('_')
                    .map(|word| {
                        let mut chars = word.chars();
                        match chars.next() {
                            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    fn title_of(info: &SettingInfo) -> String {
        info.label.clone().unwrap_or_else(|| info.key.clone())
    }

    fn text_default(info: &SettingInfo) -> Option<SharedString> {
        info.default_value
            .as_str()
            .ok()
            .map(|v| SharedString::from(v.to_string()))
    }

    fn bool_default(info: &SettingInfo) -> Option<bool> {
        info.default_value.as_bool().ok()
    }

    fn number_default(info: &SettingInfo) -> Option<f64> {
        info.default_value
            .as_float()
            .ok()
            .or_else(|| info.default_value.as_int().ok().map(|v| v as f64))
    }

    fn map_bool_item(&self, info: &SettingInfo) -> SettingItem {
        let info_for_get = info.clone();
        let info_for_set = info.clone();

        let field = SettingField::<bool>::checkbox(
            move |_| Self::read_value(&info_for_get).as_bool().unwrap_or(false),
            {
                let project_path = self.project_path.clone();
                move |value, _| {
                    Self::write_value(&info_for_set, ConfigValue::Bool(value), &project_path);
                }
            },
        );

        let field = if let Some(default_value) = Self::bool_default(info) {
            field.default_value(default_value)
        } else {
            field
        };

        SettingItem::new(Self::title_of(info), field)
    }

    fn map_text_item(&self, info: &SettingInfo) -> SettingItem {
        let info_for_get = info.clone();
        let info_for_set = info.clone();

        let field = SettingField::<SharedString>::input(
            move |_| {
                SharedString::from(
                    Self::read_value(&info_for_get)
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                )
            },
            {
                let project_path = self.project_path.clone();
                move |value, _| {
                    Self::write_value(
                        &info_for_set,
                        ConfigValue::String(value.to_string()),
                        &project_path,
                    );
                }
            },
        );

        let field = if let Some(default_value) = Self::text_default(info) {
            field.default_value(default_value)
        } else {
            field
        };

        SettingItem::new(Self::title_of(info), field)
    }

    fn map_number_item(
        &self,
        info: &SettingInfo,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
    ) -> SettingItem {
        let info_for_get = info.clone();
        let info_for_set = info.clone();
        let min_value = min.unwrap_or(f64::MIN);
        let max_value = max.unwrap_or(f64::MAX);
        let step_value = step.unwrap_or(1.0);

        let field = SettingField::<f64>::number_input(
            NumberFieldOptions {
                min: min_value,
                max: max_value,
                step: step_value,
            },
            move |_| {
                let value = Self::read_value(&info_for_get);
                value
                    .as_float()
                    .ok()
                    .or_else(|| value.as_int().ok().map(|v| v as f64))
                    .unwrap_or(min_value)
            },
            {
                let project_path = self.project_path.clone();
                move |value, _| {
                    Self::write_value(&info_for_set, ConfigValue::Float(value), &project_path);
                }
            },
        );

        let field = if let Some(default_value) = Self::number_default(info) {
            field.default_value(default_value)
        } else {
            field
        };

        SettingItem::new(Self::title_of(info), field)
    }

    fn map_dropdown_item(&self, info: &SettingInfo, options: &Vec<DropdownOption>) -> SettingItem {
        let info_for_get = info.clone();
        let info_for_set = info.clone();

        let field = SettingField::<SharedString>::dropdown(
            options
                .iter()
                .map(|opt| {
                    (
                        SharedString::from(opt.value.clone()),
                        SharedString::from(opt.label.clone()),
                    )
                })
                .collect(),
            move |_| {
                SharedString::from(
                    Self::read_value(&info_for_get)
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                )
            },
            {
                let project_path = self.project_path.clone();
                move |value, _| {
                    Self::write_value(
                        &info_for_set,
                        ConfigValue::String(value.to_string()),
                        &project_path,
                    );
                }
            },
        );

        let field = if let Some(default_value) = Self::text_default(info) {
            field.default_value(default_value)
        } else {
            field
        };

        SettingItem::new(Self::title_of(info), field)
    }

    fn map_item(&self, info: &SettingInfo) -> SettingItem {
        let mut item = match info.field_type.as_ref() {
            Some(FieldType::Checkbox) => self.map_bool_item(info),
            Some(FieldType::TextInput { .. }) => self.map_text_item(info),
            Some(FieldType::PathSelector { .. }) => self.map_text_item(info),
            Some(FieldType::ColorPicker) => self.map_text_item(info),
            Some(FieldType::NumberInput { min, max, step }) => {
                self.map_number_item(info, *min, *max, *step)
            }
            Some(FieldType::Slider { min, max, step }) => {
                self.map_number_item(info, Some(*min), Some(*max), Some(*step))
            }
            Some(FieldType::Dropdown { options }) => self.map_dropdown_item(info, options),
            None => self.map_text_item(info),
        };

        if !info.description.is_empty() {
            item = item.description(info.description.clone());
        }

        item
    }

    fn pages_for_namespace(&self, namespace: &str) -> Vec<SettingPage> {
        global_config()
            .list_pages(namespace)
            .into_iter()
            .map(|page_name| {
                let settings = global_config().list_settings_by_page(namespace, &page_name);
                let mut owners: Vec<String> = Vec::new();

                for info in &settings {
                    if !owners.iter().any(|o| o == &info.owner) {
                        owners.push(info.owner.clone());
                    }
                }

                let mut page = SettingPage::new(page_name.clone());

                for owner in owners {
                    let items = settings
                        .iter()
                        .filter(|info| info.owner == owner)
                        .map(|info| self.map_item(info))
                        .collect::<Vec<_>>();

                    page = page.group(
                        SettingGroup::new()
                            .title(Self::pretty_owner_path(&owner))
                            .items(items),
                    );
                }

                page
            })
            .collect()
    }

    fn build_pages(&self) -> Vec<SettingPage> {
        let mut pages = self.pages_for_namespace(NS_EDITOR);
        if self.project_path.is_some() {
            pages.extend(self.pages_for_namespace(NS_PROJECT));
        }
        pages
    }
}

impl Render for ModernSettingsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(Settings::new("data-driven-settings").pages(self.build_pages()))
    }
}
