use std::sync::Arc;

use engine_state::{global_config, ConfigValue, FieldType, SettingInfo};
use gpui::{
    prelude::FluentBuilder as _, App, AppContext as _, Entity, IntoElement as _, SharedString,
    Styled as _, Subscription, Window,
};
use ui::{
    group_box::GroupBoxVariant,
    input::{InputEvent, InputState, NumberInput, NumberInputEvent, StepAction},
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem},
    AxisExt as _, Sizable as _, Size,
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

fn number_config_value(
    value: f64,
    current_value: &ConfigValue,
    default_value: &ConfigValue,
) -> Result<ConfigValue, &'static str> {
    let value_type = match current_value {
        ConfigValue::Int(_) | ConfigValue::Float(_) => current_value,
        _ => default_value,
    };

    match value_type {
        ConfigValue::Int(_) => {
            if !value.is_finite() || value.fract() != 0.0 {
                return Err("integer settings require a finite whole number");
            }
            if value < i64::MIN as f64 || value >= i64::MAX as f64 {
                return Err("integer setting value is outside the i64 range");
            }
            Ok(ConfigValue::Int(value as i64))
        }
        ConfigValue::Float(_) => Ok(ConfigValue::Float(value)),
        _ => Err("number setting has a non-numeric current and default value"),
    }
}

struct ConfigNumberState {
    input: Entity<InputState>,
    initial_value: f64,
    _subscriptions: Vec<Subscription>,
}

fn current_number_value(namespace: &str, owner: &str, key: &str) -> Option<f64> {
    global_config()
        .get(namespace, owner, key)
        .ok()
        .and_then(|value| value.as_float().ok())
}

fn replacement_number_value(input_value: f64, actual_value: f64) -> Option<SharedString> {
    (actual_value != input_value).then(|| actual_value.to_string().into())
}

#[allow(clippy::too_many_arguments)]
fn number_setting_field(
    namespace: String,
    owner: String,
    key: String,
    current_value: ConfigValue,
    default_value: ConfigValue,
    min: f64,
    max: f64,
    step: f64,
    mark_dirty: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> SettingField<SharedString> {
    SettingField::render(move |options, window: &mut Window, cx: &mut App| {
        let initial_value = current_number_value(&namespace, &owner, &key).unwrap_or(0.0);
        let state_key = SharedString::from(format!(
            "config-number-{namespace}-{owner}-{key}-{}-{}-{}",
            options.page_ix, options.group_ix, options.item_ix
        ));
        let namespace = namespace.clone();
        let owner = owner.clone();
        let key = key.clone();
        let current_value = current_value.clone();
        let default_value = default_value.clone();
        let mark_dirty = mark_dirty.clone();

        let state = window
            .use_keyed_state(state_key, cx, move |window, cx| {
                let input = cx
                    .new(|cx| InputState::new(window, cx).default_value(initial_value.to_string()));
                let step_subscription = cx.subscribe_in(
                    &input,
                    window,
                    move |_, input, event: &NumberInputEvent, window, cx| {
                        let NumberInputEvent::Step { action, fine } = event;
                        input.update(cx, |input, cx| {
                            let Ok(value) = input.value().parse::<f64>() else {
                                return;
                            };
                            let step = if *fine { step * 0.1 } else { step };
                            let next = match action {
                                StepAction::Increment => value + step,
                                StepAction::Decrement => value - step,
                            };
                            input.set_value(next.to_string(), window, cx);
                        });
                    },
                );
                let change_subscription = cx.subscribe_in(
                    &input,
                    window,
                    move |state: &mut ConfigNumberState, input, event: &InputEvent, window, cx| {
                        if !matches!(event, InputEvent::Change) {
                            return;
                        }

                        input.update(cx, |input, cx| {
                            let raw = input.value();
                            if raw == state.initial_value.to_string() {
                                return;
                            }
                            let Ok(input_value) = raw.parse::<f64>() else {
                                return;
                            };
                            let clamped_value = input_value.clamp(min, max);

                            let result =
                                number_config_value(clamped_value, &current_value, &default_value)
                                    .map_err(str::to_owned)
                                    .and_then(|value| {
                                        let handle = global_config()
                                            .owner_handle(&namespace, &owner)
                                            .ok_or_else(|| {
                                                "setting owner is not registered".to_string()
                                            })?;
                                        handle.set(&key, value).map_err(|error| error.to_string())
                                    });

                            if let Err(error) = &result {
                                tracing::error!(
                                    namespace = %namespace,
                                    owner = %owner,
                                    key = %key,
                                    %error,
                                    "Failed to update number setting"
                                );
                            }

                            let actual_value = current_number_value(&namespace, &owner, &key)
                                .unwrap_or(state.initial_value);
                            state.initial_value = actual_value;
                            if let Some(value) = replacement_number_value(input_value, actual_value)
                            {
                                input.set_value(value, window, cx);
                            }

                            if result.is_ok() {
                                mark_dirty(cx);
                            }
                        });
                    },
                );

                ConfigNumberState {
                    input,
                    initial_value,
                    _subscriptions: vec![step_subscription, change_subscription],
                }
            })
            .read(cx);

        NumberInput::new(&state.input)
            .with_size(options.size)
            .map(|this| {
                if options.layout.is_horizontal() {
                    this.w_32()
                } else {
                    this.w_full()
                }
            })
            .into_any_element()
    })
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
            let current_value = info.current_value.clone();
            let default_value = info.default_value.clone();
            SettingItem::new(
                label,
                number_setting_field(
                    ns,
                    owner,
                    key,
                    current_value,
                    default_value,
                    min.unwrap_or(f64::MIN),
                    max.unwrap_or(f64::MAX),
                    step.unwrap_or(1.0),
                    mark_dirty,
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

#[cfg(test)]
mod tests {
    use super::{number_config_value, replacement_number_value};
    use engine_state::ConfigValue;

    #[test]
    fn number_config_value_preserves_integer_type() {
        let value = number_config_value(12.0, &ConfigValue::Int(5), &ConfigValue::Int(1));

        assert_eq!(value, Ok(ConfigValue::Int(12)));
    }

    #[test]
    fn number_config_value_preserves_float_type() {
        let value = number_config_value(12.5, &ConfigValue::Float(5.0), &ConfigValue::Float(1.0));

        assert_eq!(value, Ok(ConfigValue::Float(12.5)));
    }

    #[test]
    fn number_config_value_uses_default_type_as_fallback() {
        let value = number_config_value(
            12.0,
            &ConfigValue::String(String::new()),
            &ConfigValue::Int(1),
        );

        assert_eq!(value, Ok(ConfigValue::Int(12)));
    }

    #[test]
    fn number_config_value_rejects_fractional_integer() {
        let value = number_config_value(12.5, &ConfigValue::Int(5), &ConfigValue::Int(1));

        assert_eq!(value, Err("integer settings require a finite whole number"));
    }

    #[test]
    fn accepted_number_input_needs_no_display_replacement() {
        assert_eq!(replacement_number_value(12.0, 12.0), None);
    }

    #[test]
    fn rejected_number_input_restores_the_actual_value() {
        assert_eq!(replacement_number_value(12.5, 5.0).as_deref(), Some("5"));
    }

    #[test]
    fn clamped_number_input_displays_the_clamped_value() {
        assert_eq!(replacement_number_value(75.0, 60.0).as_deref(), Some("60"));
    }
}
