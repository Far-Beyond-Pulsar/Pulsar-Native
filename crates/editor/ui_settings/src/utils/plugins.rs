//! The Settings window's Plugins section: one page per plugin that has
//! registered settings (`pulsar_settings::plugin`), its groups the schema's
//! `page`s. Edits go to the scope the person picked (user or project,
//! `engine_state::plugin_settings`); setting a value back to its default
//! removes the override in that scope, which is also what the reset button
//! does. A row says which scope provides its value.

use std::collections::BTreeMap;
use std::sync::Arc;

use engine_state::plugin_settings::{self, SettingScope};
use engine_state::{global_config, ConfigValue, FieldType, SettingInfo};
use gpui::{App, SharedString};
use pulsar_settings::plugin::{page_title, NS};
use ui::{
    setting::{SettingField, SettingGroup, SettingItem, SettingPage},
    Icon, IconName,
};

use super::config::number_setting_field;

pub fn scope_to_value(scope: SettingScope) -> SharedString {
    match scope {
        SettingScope::User => "user".into(),
        SettingScope::Project => "project".into(),
    }
}

pub fn scope_from_value(value: &str) -> SettingScope {
    match value {
        "project" => SettingScope::Project,
        _ => SettingScope::User,
    }
}

/// What the row says about where its value comes from.
pub fn scope_note(owner: &str, key: &str) -> &'static str {
    match plugin_settings::scope_of(owner, key) {
        Some(SettingScope::User) => "Your override",
        Some(SettingScope::Project) => "Set by the project",
        None => "Default",
    }
}

/// Write `value` into `scope`, or, when it is the schema default, drop the
/// scope's override. Returns whether it applied.
fn write_scoped(
    owner: &str,
    key: &str,
    scope: SettingScope,
    value: ConfigValue,
    default: &ConfigValue,
) -> Result<(), String> {
    if &value == default {
        plugin_settings::reset(owner, key, scope);
        return Ok(());
    }
    plugin_settings::set(owner, key, scope, value).map_err(|error| format!("{error:#}"))
}

fn effective(owner: &str, key: &str) -> Option<ConfigValue> {
    global_config().get(NS, owner, key).ok()
}

fn item_for(
    info: &SettingInfo,
    scope: SettingScope,
    mark: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Option<SettingItem> {
    let owner = info.owner.clone();
    let key = info.key.clone();
    let default = info.default_value.clone();
    let label: SharedString = info.label.clone().unwrap_or_else(|| info.key.clone()).into();
    let restart = if info.requires_restart() {
        " Restart the editor to apply."
    } else {
        ""
    };
    let hidden = plugin_settings::overrides(&owner, &key, SettingScope::Project)
        && plugin_settings::overrides(&owner, &key, SettingScope::User);
    let note = if hidden {
        format!("{}, hides the project's value", scope_note(&owner, &key))
    } else {
        scope_note(&owner, &key).to_string()
    };
    let description: SharedString = format!("{} ({note}){restart}", info.description).into();
    let field_type = info.field_type.clone()?;

    let item = match field_type {
        FieldType::Checkbox => {
            let (get_owner, get_key) = (owner.clone(), key.clone());
            SettingItem::new(
                label,
                SettingField::checkbox(
                    move |_cx: &App| {
                        effective(&get_owner, &get_key)
                            .and_then(|v| v.as_bool().ok())
                            .unwrap_or(false)
                    },
                    move |value: bool, cx: &mut App| {
                        if let Err(error) =
                            write_scoped(&owner, &key, scope, ConfigValue::Bool(value), &default)
                        {
                            tracing::error!("plugin setting {owner}.{key}: {error}");
                        }
                        mark(cx);
                    },
                )
                .default_value(matches!(info.default_value, ConfigValue::Bool(true))),
            )
        }
        FieldType::TextInput { .. } | FieldType::PathSelector { .. } => {
            let (get_owner, get_key) = (owner.clone(), key.clone());
            let default_text: SharedString = default.as_str().map(str::to_owned).unwrap_or_default().into();
            SettingItem::new(
                label,
                SettingField::input(
                    move |_cx: &App| {
                        effective(&get_owner, &get_key)
                            .and_then(|v| v.as_str().ok().map(|s| SharedString::from(s.to_owned())))
                            .unwrap_or_default()
                    },
                    move |value: SharedString, cx: &mut App| {
                        if let Err(error) = write_scoped(
                            &owner,
                            &key,
                            scope,
                            ConfigValue::String(value.to_string()),
                            &default,
                        ) {
                            tracing::error!("plugin setting {owner}.{key}: {error}");
                        }
                        mark(cx);
                    },
                )
                .default_value(default_text),
            )
        }
        FieldType::Dropdown { options } => {
            let (get_owner, get_key) = (owner.clone(), key.clone());
            let default_text: SharedString = default.as_str().map(str::to_owned).unwrap_or_default().into();
            let options: Vec<(SharedString, SharedString)> = options
                .iter()
                .map(|o| (SharedString::from(o.value.clone()), SharedString::from(o.label.clone())))
                .collect();
            SettingItem::new(
                label,
                SettingField::dropdown(
                    options,
                    move |_cx: &App| {
                        effective(&get_owner, &get_key)
                            .and_then(|v| v.as_str().ok().map(|s| SharedString::from(s.to_owned())))
                            .unwrap_or_default()
                    },
                    move |value: SharedString, cx: &mut App| {
                        if let Err(error) = write_scoped(
                            &owner,
                            &key,
                            scope,
                            ConfigValue::String(value.to_string()),
                            &default,
                        ) {
                            tracing::error!("plugin setting {owner}.{key}: {error}");
                        }
                        mark(cx);
                    },
                )
                .default_value(default_text),
            )
        }
        FieldType::NumberInput { .. } | FieldType::Slider { .. }
            if matches!(info.default_value, ConfigValue::Int(_) | ConfigValue::Float(_)) =>
        {
            let (min, max, step) = match &info.field_type {
                Some(FieldType::Slider { min, max, step }) => (Some(*min), Some(*max), Some(*step)),
                Some(FieldType::NumberInput { min, max, step }) => (*min, *max, *step),
                _ => (None, None, None),
            };
            let write: Arc<dyn Fn(ConfigValue) -> Result<(), String>> = {
                let (owner, key, default) = (owner.clone(), key.clone(), default.clone());
                Arc::new(move |value| write_scoped(&owner, &key, scope, value, &default))
            };
            SettingItem::new(
                label,
                number_setting_field(
                    NS.to_string(),
                    owner,
                    key,
                    info.current_value.clone(),
                    info.default_value.clone(),
                    min.unwrap_or(f64::MIN),
                    max.unwrap_or(f64::MAX),
                    step.unwrap_or(1.0),
                    mark,
                    write,
                ),
            )
        }
        _ => return None,
    };
    Some(item.description(description))
}

/// One page per plugin with registered settings, titled by the plugin.
/// `scope` is where this window's edits are written.
pub fn plugin_pages(
    scope: SettingScope,
    scope_group: SettingGroup,
    mark_dirty: Arc<dyn Fn(&mut App) + Send + Sync>,
    mark_restart: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Vec<(String, SettingPage)> {
    let cfg = global_config();
    let mut by_page: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for owner in cfg.list_owners(NS) {
        if let Some(first) = owner.first() {
            by_page.entry(first.clone()).or_default().push(owner);
        }
    }

    let mut pages: Vec<(String, SettingPage)> = Vec::new();
    for (page_key, mut owners) in by_page {
        owners.sort();
        let multi = owners.len() > 1;
        let mut groups: Vec<SettingGroup> = Vec::new();
        for owner in owners {
            let owner_path = owner.join("/");
            let Some(mut settings) = cfg.list_settings(NS, &owner_path) else {
                continue;
            };
            settings.sort_by(|a, b| {
                let la = a.label.as_deref().unwrap_or(&a.key);
                let lb = b.label.as_deref().unwrap_or(&b.key);
                la.cmp(lb)
            });
            let mut by_schema_page: BTreeMap<String, Vec<SettingItem>> = BTreeMap::new();
            for info in &settings {
                let mark = if info.requires_restart() {
                    mark_restart.clone()
                } else {
                    mark_dirty.clone()
                };
                if let Some(item) = item_for(info, scope, mark) {
                    by_schema_page
                        .entry(info.page.clone().unwrap_or_else(|| "General".to_string()))
                        .or_default()
                        .push(item);
                }
            }
            for (schema_page, items) in by_schema_page {
                let title = if multi {
                    format!("{} - {schema_page}", owner.last().cloned().unwrap_or_default())
                } else {
                    schema_page
                };
                groups.push(SettingGroup::new().title(title).items(items));
            }
        }
        if groups.is_empty() {
            continue;
        }
        let title = page_title(cfg, &page_key);
        let page_title = format!("Plugin: {title}");
        pages.push((
            page_title.clone(),
            SettingPage::new(page_title)
                .icon(Icon::new(IconName::Box))
                .group(scope_group.clone())
                .groups(groups),
        ));
    }
    pages.sort_by_key(|(title, _)| title.to_lowercase());
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_round_trip_through_their_dropdown_values() {
        for scope in [SettingScope::User, SettingScope::Project] {
            assert_eq!(scope_from_value(&scope_to_value(scope)), scope);
        }
        assert_eq!(scope_from_value("anything else"), SettingScope::User);
    }

    #[test]
    fn a_value_equal_to_the_default_removes_the_override() {
        use pulsar_settings::plugin::{sync_with, PluginSettings};
        fn schema() -> engine_state::NamespaceSchema {
            engine_state::NamespaceSchema::new("Page test", "").setting(
                "flag",
                engine_state::SchemaEntry::new("Flag", false)
                    .field_type(FieldType::Checkbox),
            )
        }
        const REG: PluginSettings = PluginSettings {
            origin: "engine:ui_settings_page_test",
            id: "",
            schema,
        };
        sync_with(global_config(), [&REG], |_| true);
        let owner = "engine.ui_settings_page_test";
        let default = ConfigValue::Bool(false);
        // No project open in a test: the user scope is the one writable here.
        write_scoped(owner, "flag", SettingScope::User, ConfigValue::Bool(true), &default).unwrap();
        assert_eq!(scope_note(owner, "flag"), "Your override");
        write_scoped(owner, "flag", SettingScope::User, ConfigValue::Bool(false), &default).unwrap();
        assert_eq!(scope_note(owner, "flag"), "Default");
        assert!(write_scoped(owner, "flag", SettingScope::Project, ConfigValue::Bool(true), &default).is_err(), "no project scope without a project");
    }
}
