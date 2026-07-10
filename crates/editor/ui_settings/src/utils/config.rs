use std::sync::Arc;

use engine_state::{
    global_config, ConfigValue, FieldType, SettingInfo,
};
use gpui::{App, SharedString};
use ui::{
    group_box::GroupBoxVariant,
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem},
    Size,
};

pub fn group_variant_to_value(variant: GroupBoxVariant) -> SharedString {
    match variant {
        GroupBoxVariant::Normal => "normal".into(),
        GroupBoxVariant::Outline => "outline".into(),
        GroupBoxVariant::Fill => "fill".into(),
    }
}

pub fn group_variant_from_value(value: &str) -> GroupBoxVariant {
    match value {
        "normal" => GroupBoxVariant::Normal,
        "fill" => GroupBoxVariant::Fill,
        _ => GroupBoxVariant::Outline,
    }
}

pub fn size_to_value(size: Size) -> SharedString {
    match size {
        Size::XSmall => "xsmall".into(),
        Size::Small => "small".into(),
        Size::Large => "large".into(),
        _ => "medium".into(),
    }
}

pub fn size_from_value(value: &str) -> Size {
    match value {
        "xsmall" => Size::XSmall,
        "small" => Size::Small,
        "large" => Size::Large,
        _ => Size::Medium,
    }
}

pub fn item_from_info(
    info: &SettingInfo,
    mark_dirty: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Option<SettingItem> {
    let ns = info.namespace.clone();
    let owner = info.owner.clone();
    let key = info.key.clone();
    let label: SharedString = info
        .label
        .clone()
        .unwrap_or_else(|| info.key.clone())
        .into();
    let desc: SharedString = info.description.clone().into();

    let field_type = info.field_type.clone()?;

    let item: SettingItem = match field_type {
        FieldType::Checkbox => {
            let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
            let notify = mark_dirty.clone();
            SettingItem::new(
                label,
                SettingField::checkbox(
                    move |_cx: &App| {
                        global_config()
                            .get(&ns, &owner, &key)
                            .ok()
                            .and_then(|v| v.as_bool().ok())
                            .unwrap_or(false)
                    },
                    move |val: bool, cx: &mut App| {
                        if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                            let _ = h.set(&key2, ConfigValue::Bool(val));
                        }
                        if key2 == "allow_unsafe_process" {
                            pulsar_std::set_unsafe_process_allowed(val);
                        }
                        notify(cx);
                    },
                ),
            )
            .description(desc)
        }
        FieldType::TextInput { .. } => {
            let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
            let notify = mark_dirty.clone();
            SettingItem::new(
                label,
                SettingField::input(
                    move |_cx: &App| {
                        global_config()
                            .get(&ns, &owner, &key)
                            .ok()
                            .and_then(|v| {
                                v.as_str().ok().map(|s| SharedString::from(s.to_owned()))
                            })
                            .unwrap_or_default()
                    },
                    move |val: SharedString, cx: &mut App| {
                        if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                            let _ = h.set(&key2, ConfigValue::String(val.to_string()));
                        }
                        notify(cx);
                    },
                ),
            )
            .description(desc)
        }
        FieldType::NumberInput { min, max, step } => {
            let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
            let notify = mark_dirty.clone();
            let opts = NumberFieldOptions {
                min: min.unwrap_or(f64::MIN),
                max: max.unwrap_or(f64::MAX),
                step: step.unwrap_or(1.0),
            };
            SettingItem::new(
                label,
                SettingField::number_input(
                    opts,
                    move |_cx: &App| {
                        global_config()
                            .get(&ns, &owner, &key)
                            .ok()
                            .and_then(|v| v.as_float().ok())
                            .unwrap_or(0.0)
                    },
                    move |val: f64, cx: &mut App| {
                        if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                            let _ = h.set(&key2, ConfigValue::Float(val));
                        }
                        notify(cx);
                    },
                ),
            )
            .description(desc)
        }
        FieldType::Slider { min, max, step } => {
            let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
            let notify = mark_dirty.clone();
            let opts = NumberFieldOptions { min, max, step };
            SettingItem::new(
                label,
                SettingField::number_input(
                    opts,
                    move |_cx: &App| {
                        global_config()
                            .get(&ns, &owner, &key)
                            .ok()
                            .and_then(|v| v.as_float().ok())
                            .unwrap_or(0.0)
                    },
                    move |val: f64, cx: &mut App| {
                        if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                            let _ = h.set(&key2, ConfigValue::Float(val));
                        }
                        notify(cx);
                    },
                ),
            )
            .description(desc)
        }
        FieldType::Dropdown { options } => {
            let (ns2, owner2, key2) = (ns.clone(), owner.clone(), key.clone());
            let notify = mark_dirty.clone();
            let opts: Vec<(SharedString, SharedString)> = options
                .iter()
                .map(|o| {
                    (
                        SharedString::from(o.value.clone()),
                        SharedString::from(o.label.clone()),
                    )
                })
                .collect();
            SettingItem::new(
                label,
                SettingField::dropdown(
                    opts,
                    move |_cx: &App| {
                        global_config()
                            .get(&ns, &owner, &key)
                            .ok()
                            .and_then(|v| {
                                v.as_str().ok().map(|s| SharedString::from(s.to_owned()))
                            })
                            .unwrap_or_default()
                    },
                    move |val: SharedString, cx: &mut App| {
                        if let Some(h) = global_config().owner_handle(&ns2, &owner2) {
                            let _ = h.set(&key2, ConfigValue::String(val.to_string()));
                        }
                        notify(cx);
                    },
                ),
            )
            .description(desc)
        }
        _ => return None,
    };

    Some(item)
}

pub fn groups_for_namespace(
    ns: &str,
    mark_dirty: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Vec<SettingGroup> {
    let mut owners = global_config().list_owners(ns);
    owners.sort();

    owners
        .into_iter()
        .filter_map(|owner_segs| {
            let owner_path = owner_segs.join("/");
            let mut settings = global_config().list_settings(ns, &owner_path)?;
            settings.sort_by(|a, b| {
                let la = a.label.as_deref().unwrap_or(&a.key);
                let lb = b.label.as_deref().unwrap_or(&b.key);
                la.cmp(lb)
            });

            let group_title = owner_segs
                .first()
                .map(|s| {
                    let mut c = s.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => {
                            f.to_uppercase().collect::<String>() + &c.as_str().replace('_', " ")
                        }
                    }
                })
                .unwrap_or_else(|| owner_path.clone());

            let items: Vec<SettingItem> = settings
                .iter()
                .filter_map(|info| item_from_info(info, mark_dirty.clone()))
                .collect();

            if items.is_empty() {
                return None;
            }

            Some(SettingGroup::new().title(group_title).items(items))
        })
        .collect()
}
