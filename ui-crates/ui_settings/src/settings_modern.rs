use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Styled, Render, SharedString, Window, div,
};

use engine_state::{
    global_config, ConfigValue, DropdownOption, FieldType, GlobalSettings,
    ProjectSettings, SettingInfo, NS_EDITOR, NS_PROJECT,
};

use std::path::PathBuf;

use ui::{
    ActiveTheme, Icon, IconName, Sizable, Size, StyledExt as _, Theme, ThemeMode,
    button::{Button, ButtonVariants as _},
    group_box::GroupBoxVariant,
    h_flex, v_flex,
    setting::{
        NumberFieldOptions, SettingField, SettingGroup,
        SettingItem, SettingPage, Settings,
    },
};

pub struct ModernSettingsScreen {
    focus_handle: FocusHandle,
    group_variant: GroupBoxVariant,
    size: Size,
    project_path: Option<PathBuf>,
}

impl ModernSettingsScreen {
    fn group_variant_to_value(variant: GroupBoxVariant) -> SharedString {
        match variant {
            GroupBoxVariant::Normal => "normal".into(),
            GroupBoxVariant::Outline => "outline".into(),
            GroupBoxVariant::Fill => "fill".into(),
        }
    }

    fn group_variant_from_value(value: &str) -> GroupBoxVariant {
        match value {
            "normal" => GroupBoxVariant::Normal,
            "fill" => GroupBoxVariant::Fill,
            _ => GroupBoxVariant::Outline,
        }
    }

    fn size_to_value(size: Size) -> SharedString {
        match size {
            Size::XSmall => "xsmall".into(),
            Size::Small => "small".into(),
            Size::Large => "large".into(),
            _ => "medium".into(),
        }
    }

    fn size_from_value(value: &str) -> Size {
        match value {
            "xsmall" => Size::XSmall,
            "small" => Size::Small,
            "large" => Size::Large,
            _ => Size::Medium,
        }
    }

    pub fn new(
        project_path: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = window;
        Self {
            focus_handle: cx.focus_handle(),
            group_variant: GroupBoxVariant::Outline,
            size: Size::default(),
            project_path,
        }
    }

    /// Build a `SettingItem` from a `SettingInfo` pulled from the global config.
    fn item_from_info(info: &SettingInfo) -> Option<SettingItem> {
        let ns = info.namespace.clone();
        let owner = info.owner.clone();
        let key = info.key.clone();
        let label: SharedString = info.label.clone().unwrap_or_else(|| info.key.clone()).into();
        let desc: SharedString = info.description.clone().into();

        let field_type = info.field_type.clone()?;

        let item: SettingItem = match field_type {
            FieldType::Checkbox => {
                let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
                SettingItem::new(
                    label,
                    SettingField::checkbox(
                        move |_cx: &App| {
                            global_config().get(&ns, &owner, &key)
                                .ok().and_then(|v| v.as_bool().ok()).unwrap_or(false)
                        },
                        move |val: bool, _cx: &mut App| {
                            if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                                let _ = h.set(&key2, ConfigValue::Bool(val));
                            }
                        },
                    ),
                )
                .description(desc)
            }
            FieldType::TextInput { .. } => {
                let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
                SettingItem::new(
                    label,
                    SettingField::input(
                        move |_cx: &App| {
                            global_config().get(&ns, &owner, &key)
                                .ok()
                                .and_then(|v| v.as_str().ok().map(|s| SharedString::from(s.to_owned())))
                                .unwrap_or_default()
                        },
                        move |val: SharedString, _cx: &mut App| {
                            if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                                let _ = h.set(&key2, ConfigValue::String(val.to_string()));
                            }
                        },
                    ),
                )
                .description(desc)
            }
            FieldType::NumberInput { min, max, step } => {
                let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
                let opts = NumberFieldOptions {
                    min: min.unwrap_or(f64::MIN),
                    max: max.unwrap_or(f64::MAX),
                    step: step.unwrap_or(1.0),
                    ..Default::default()
                };
                SettingItem::new(
                    label,
                    SettingField::number_input(
                        opts,
                        move |_cx: &App| {
                            global_config().get(&ns, &owner, &key)
                                .ok().and_then(|v| v.as_float().ok()).unwrap_or(0.0)
                        },
                        move |val: f64, _cx: &mut App| {
                            if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                                let _ = h.set(&key2, ConfigValue::Float(val));
                            }
                        },
                    ),
                )
                .description(desc)
            }
            FieldType::Slider { min, max, step } => {
                // Map slider to number_input — no dedicated slider SettingField
                let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
                let opts = NumberFieldOptions {
                    min,
                    max,
                    step,
                    ..Default::default()
                };
                SettingItem::new(
                    label,
                    SettingField::number_input(
                        opts,
                        move |_cx: &App| {
                            global_config().get(&ns, &owner, &key)
                                .ok().and_then(|v| v.as_float().ok()).unwrap_or(0.0)
                        },
                        move |val: f64, _cx: &mut App| {
                            if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                                let _ = h.set(&key2, ConfigValue::Float(val));
                            }
                        },
                    ),
                )
                .description(desc)
            }
            FieldType::Dropdown { options } => {
                let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
                let opts: Vec<(SharedString, SharedString)> = options
                    .iter()
                    .map(|o| (SharedString::from(o.value.clone()), SharedString::from(o.label.clone())))
                    .collect();
                SettingItem::new(
                    label,
                    SettingField::dropdown(
                        opts,
                        move |_cx: &App| {
                            global_config().get(&ns, &owner, &key)
                                .ok()
                                .and_then(|v| v.as_str().ok().map(|s| SharedString::from(s.to_owned())))
                                .unwrap_or_default()
                        },
                        move |val: SharedString, _cx: &mut App| {
                            if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                                let _ = h.set(&key2, ConfigValue::String(val.to_string()));
                            }
                        },
                    ),
                )
                .description(desc)
            }
            // Unsupported field types: skip
            _ => return None,
        };

        Some(item)
    }

    /// Build all SettingGroups for one namespace, grouping settings by owner.
    fn groups_for_namespace(ns: &str) -> Vec<SettingGroup> {
        let mut owners = global_config().list_owners(ns);
        // Sort for stable ordering
        owners.sort();

        owners.into_iter().filter_map(|owner_segs| {
            let owner_path = owner_segs.join("/");
            let mut settings = global_config().list_settings(ns, &owner_path)?;
            // Sort settings by their label for predictable order
            settings.sort_by(|a, b| {
                let la = a.label.as_deref().unwrap_or(&a.key);
                let lb = b.label.as_deref().unwrap_or(&b.key);
                la.cmp(lb)
            });

            // Use the display name of this owner as the group title
            // (capitalize first segment of owner path)
            let group_title = owner_segs.first()
                .map(|s| {
                    let mut c = s.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().replace('_', " "),
                    }
                })
                .unwrap_or_else(|| owner_path.clone());

            let items: Vec<SettingItem> = settings.iter()
                .filter_map(Self::item_from_info)
                .collect();

            if items.is_empty() {
                return None;
            }

            Some(SettingGroup::new().title(group_title).items(items))
        }).collect()
    }

    fn setting_pages(&self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<SettingPage> {
        let view = cx.entity();

        // ── UI Controls page (appearance of this settings screen itself) ──
        let ui_controls_page = {
            let view2 = view.clone();
            SettingPage::new("UI Controls")
                .default_open(true)
                .icon(Icon::new(IconName::Settings2))
                .group(
                    SettingGroup::new().title("Settings Display").items(vec![
                        SettingItem::new(
                            "Dark Mode",
                            SettingField::switch(
                                |cx: &App| cx.theme().mode.is_dark(),
                                |val: bool, cx: &mut App| {
                                    let mode = if val { ThemeMode::Dark } else { ThemeMode::Light };
                                    Theme::global_mut(cx).mode = mode;
                                    Theme::change(mode, None, cx);
                                },
                            ),
                        )
                        .description("Switch between light and dark themes."),
                        SettingItem::new(
                            "Group Variant",
                            SettingField::dropdown(
                                vec![
                                    ("normal".into(), "Normal".into()),
                                    ("outline".into(), "Outline".into()),
                                    ("fill".into(), "Fill".into()),
                                ],
                                {
                                    let v = view.clone();
                                    move |cx: &App| Self::group_variant_to_value(v.read(cx).group_variant)
                                },
                                {
                                    let v = view2.clone();
                                    move |val: SharedString, cx: &mut App| {
                                        v.update(cx, |this, cx| {
                                            this.group_variant = Self::group_variant_from_value(val.as_ref());
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .default_value("outline"),
                        )
                        .description("Select the variant for setting groups."),
                        SettingItem::new(
                            "Group Size",
                            SettingField::dropdown(
                                vec![
                                    ("medium".into(), "Medium".into()),
                                    ("small".into(), "Small".into()),
                                    ("xsmall".into(), "XSmall".into()),
                                ],
                                {
                                    let v = view.clone();
                                    move |cx: &App| Self::size_to_value(v.read(cx).size)
                                },
                                {
                                    let v = view2.clone();
                                    move |val: SharedString, cx: &mut App| {
                                        v.update(cx, |this, cx| {
                                            this.size = Self::size_from_value(val.as_ref());
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .default_value("medium"),
                        )
                        .description("Select the size for the setting group."),
                    ]),
                )
        };

        // ── Editor settings page (NS_EDITOR) ──
        let editor_groups = Self::groups_for_namespace(NS_EDITOR);
        let editor_page = SettingPage::new("Editor")
            .icon(Icon::new(IconName::Code))
            .groups(editor_groups);

        // ── Project settings page (NS_PROJECT) ──
        let project_groups = Self::groups_for_namespace(NS_PROJECT);
        let project_page = SettingPage::new("Project")
            .icon(Icon::new(IconName::Folder))
            .groups(project_groups);

        vec![ui_controls_page, editor_page, project_page]
    }
}

impl Focusable for ModernSettingsScreen {
    fn focus_handle(&self, _: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ModernSettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .child(
                // Persistent action bar with Save button
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2()
                    .justify_end()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.sidebar)
                    .child(
                        Button::new("save-settings")
                            .primary()
                            .small()
                            .icon(IconName::Check)
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, _cx| {
                                match GlobalSettings::new().save_all() {
                                    Ok(_) => tracing::info!("Editor settings saved."),
                                    Err(e) => tracing::error!("Failed to save editor settings: {e}"),
                                }
                                if let Some(ref path) = screen.project_path {
                                    match ProjectSettings::new(path).save_all() {
                                        Ok(_) => tracing::info!("Project settings saved."),
                                        Err(e) => tracing::error!("Failed to save project settings: {e}"),
                                    }
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .child(
                        Settings::new("app-settings")
                            .with_size(self.size)
                            .with_group_variant(self.group_variant)
                            .pages(self.setting_pages(window, cx)),
                    ),
            )
    }
}

