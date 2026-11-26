//! The standard settings group every language server gets (D10): the same
//! controls for every server, registered as plugin settings
//! (`pulsar_settings::plugin`) and read back here as [`ServerSettings`].

use std::path::PathBuf;

use pulsar_config::{DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};
use serde_json::Value;

use crate::alternative::{alternatives_of, ServerAlternative};
use crate::spec::LanguageServerSpec;

/// How much of the protocol conversation reaches the server's log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TraceLevel {
    #[default]
    Off,
    /// Each message's method and id.
    Messages,
    /// Each message in full.
    Verbose,
}

impl TraceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Messages => "messages",
            Self::Verbose => "verbose",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "messages" => Self::Messages,
            "verbose" => Self::Verbose,
            _ => Self::Off,
        }
    }
}

/// When a server starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartSetting {
    /// As the spec says (`eager`).
    #[default]
    Auto,
    /// When the project opens.
    Project,
    /// When a file it handles is first opened.
    FirstFile,
}

impl StartSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Project => "project",
            Self::FirstFile => "first_file",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "project" => Self::Project,
            "first_file" => Self::FirstFile,
            _ => Self::Auto,
        }
    }
}

/// A server's settings, as the person configured them.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerSettings {
    pub enabled: bool,
    /// The executable to use instead of looking for one.
    pub path: Option<PathBuf>,
    /// Extra command-line arguments (split on whitespace, `"` groups).
    pub args: Vec<String>,
    /// JSON merged over the spec's `initializationOptions`.
    pub initialization_options: Option<Value>,
    pub trace: TraceLevel,
    pub start: StartSetting,
    /// Stop the server after this many idle minutes (no open document of it);
    /// 0 never stops it.
    pub idle_stop_minutes: u32,
    /// The [`ServerAlternative`] chosen, by name: its spec runs instead of
    /// the language's own, and `path`, `args` and `initialization_options`
    /// are the ones set for it.
    pub alternative: Option<&'static str>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
            args: Vec::new(),
            initialization_options: None,
            trace: TraceLevel::Off,
            start: StartSetting::Auto,
            idle_stop_minutes: 0,
            alternative: None,
        }
    }
}

/// Split a command line the way a person types arguments: whitespace
/// separates, double quotes group.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

impl ServerSettings {
    /// Read the group `origin`/`id` registered; the defaults while it is not
    /// registered (the plugin is disabled, or nothing registered it).
    pub fn read(origin: &'static str, id: &str) -> Self {
        let Some(handle) = pulsar_settings::plugin::handle(origin, id) else {
            return Self::default();
        };
        let value = |key: &str| handle.value(key);
        let mut settings = Self::from_values(value);
        settings.choose_alternative(alternatives_of(origin, id), value);
        settings
    }

    /// Apply the server dropdown: the one of `alternatives` it names, with the
    /// launch settings made for that one. A name none of them has leaves the
    /// language's own server.
    pub fn choose_alternative<'a>(
        &mut self,
        alternatives: impl IntoIterator<Item = &'a ServerAlternative>,
        value: impl Fn(&str) -> Option<pulsar_config::ConfigValue>,
    ) {
        let chosen = text_of(&value, SERVER_KEY);
        let Some(alternative) = alternatives
            .into_iter()
            .find(|alternative| alternative.name == chosen)
        else {
            return;
        };
        self.alternative = Some(alternative.name);
        self.read_launch(&value, &launch_prefix(alternative));
    }

    /// The settings that say how one server is launched, under `prefix`.
    fn read_launch(
        &mut self,
        value: &impl Fn(&str) -> Option<pulsar_config::ConfigValue>,
        prefix: &str,
    ) {
        let text = |key: &str| text_of(value, &format!("{prefix}{key}"));
        let path = text("path");
        let init = text("initialization_options");
        self.path = (!path.trim().is_empty()).then(|| PathBuf::from(path.trim()));
        self.args = split_args(&text("args"));
        self.initialization_options = (!init.trim().is_empty())
            .then(|| serde_json::from_str(&init).ok())
            .flatten();
    }

    /// The spec that runs under these settings: `base`, or the chosen
    /// alternative of it.
    pub fn spec_for(
        &self,
        origin: &str,
        base: &'static LanguageServerSpec,
    ) -> &'static LanguageServerSpec {
        self.alternative
            .and_then(|name| {
                alternatives_of(origin, base.id).find(|alternative| alternative.name == name)
            })
            .map_or(base, |alternative| alternative.spec)
    }

    /// Build from a key lookup (what the tests drive).
    pub fn from_values(value: impl Fn(&str) -> Option<pulsar_config::ConfigValue>) -> Self {
        let mut settings = Self {
            enabled: value("enabled")
                .and_then(|v| v.as_bool().ok())
                .unwrap_or(true),
            trace: TraceLevel::parse(&text_of(&value, "trace")),
            start: StartSetting::parse(&text_of(&value, "start")),
            idle_stop_minutes: value("idle_stop_minutes")
                .and_then(|v| v.as_int().ok())
                .unwrap_or(0)
                .clamp(0, 24 * 60) as u32,
            ..Self::default()
        };
        settings.read_launch(&value, "");
        settings
    }

    /// Whether `other` differs in a way that needs the process restarted.
    pub fn needs_restart(&self, other: &Self) -> bool {
        self.alternative != other.alternative
            || self.path != other.path
            || self.args != other.args
            || self.initialization_options != other.initialization_options
    }
}

/// The key of the dropdown that chooses between a language's own server and
/// its alternatives.
const SERVER_KEY: &str = "server";

fn text_of(value: &impl Fn(&str) -> Option<pulsar_config::ConfigValue>, key: &str) -> String {
    value(key)
        .and_then(|v| v.as_str().ok().map(str::to_owned))
        .unwrap_or_default()
}

fn launch_prefix(alternative: &ServerAlternative) -> String {
    format!("{}_", alternative.name)
}

/// The settings that say how one server is launched: its executable, its
/// arguments and its initialization options, keyed under `prefix`. `server`
/// names it in the labels when the group configures more than one.
fn launch_settings(schema: NamespaceSchema, prefix: &str, server: Option<&str>) -> NamespaceSchema {
    let page = "Language server";
    let label = |text: &str| match server {
        Some(server) => format!("{server}: {}", text.to_lowercase()),
        None => text.to_string(),
    };
    schema
        .setting(
            format!("{prefix}path"),
            SchemaEntry::new(
                "Executable to use instead of searching for one (empty: find or install it)",
                "",
            )
            .label(label("Executable path"))
            .page(page)
            .field_type(FieldType::PathSelector { directory: false })
            .requires_restart(),
        )
        .setting(
            format!("{prefix}args"),
            SchemaEntry::new("Extra command-line arguments", "")
                .label(label("Arguments"))
                .page(page)
                .field_type(FieldType::TextInput {
                    placeholder: Some("--flag value".into()),
                    multiline: false,
                }),
        )
        .setting(
            format!("{prefix}initialization_options"),
            SchemaEntry::new(
                "JSON merged over the server's initialization options (empty: none)",
                "",
            )
            .label(label("Initialization options"))
            .page(page)
            .field_type(FieldType::TextInput {
                placeholder: Some("{ \"key\": \"value\" }".into()),
                multiline: true,
            })
            .validator(Validator::custom(|value| {
                let text = value.as_str().map_err(|e| e.to_string())?;
                if text.trim().is_empty() {
                    return Ok(());
                }
                match serde_json::from_str::<Value>(text) {
                    Ok(Value::Object(_)) => Ok(()),
                    Ok(_) => Err("expected a JSON object".to_string()),
                    Err(error) => Err(format!("not valid JSON: {error}")),
                }
            })),
        )
}

/// The dropdown choosing between `own` (the language's server, the empty
/// value) and `alternatives`.
fn server_choice(own: &str, alternatives: &[&ServerAlternative]) -> SchemaEntry {
    let mut description = format!("Which server runs ({own} is the default).");
    for alternative in alternatives {
        description.push_str(&format!(
            " {}: {}",
            alternative.label, alternative.description
        ));
    }
    let options = std::iter::once(DropdownOption::new(own, ""))
        .chain(
            alternatives
                .iter()
                .map(|alternative| DropdownOption::new(alternative.label, alternative.name)),
        )
        .collect();
    let names: Vec<&str> = std::iter::once("")
        .chain(alternatives.iter().map(|alternative| alternative.name))
        .collect();
    SchemaEntry::new(description, "")
        .label("Server")
        .page("Language server")
        .field_type(FieldType::Dropdown { options })
        .validator(Validator::string_one_of(names))
        .requires_restart()
}

/// The standard group: every server exposes the same controls. A server with
/// `alternatives` also gets the dropdown that chooses between them, and launch
/// settings for each.
pub fn server_settings_schema(
    display_name: &str,
    description: &str,
    alternatives: &[&ServerAlternative],
) -> NamespaceSchema {
    let page = "Language server";
    let mut schema = NamespaceSchema::new(display_name, description).setting(
        "enabled",
        SchemaEntry::new("Run this language server", true)
            .label("Enabled")
            .page(page)
            .field_type(FieldType::Checkbox),
    );
    let several = !alternatives.is_empty();
    if several {
        schema = schema.setting(SERVER_KEY, server_choice(display_name, alternatives));
    }
    schema = launch_settings(schema, "", several.then_some(display_name));
    for alternative in alternatives {
        schema = launch_settings(
            schema,
            &launch_prefix(alternative),
            Some(alternative.spec.display_name),
        );
    }
    schema
        .setting(
            "trace",
            SchemaEntry::new("How much of the protocol conversation is logged", "off")
                .label("Trace")
                .page(page)
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Off", "off"),
                        DropdownOption::new("Messages", "messages"),
                        DropdownOption::new("Verbose", "verbose"),
                    ],
                })
                .validator(Validator::string_one_of(["off", "messages", "verbose"])),
        )
        .setting(
            "start",
            SchemaEntry::new("When the server starts", "auto")
                .label("Start")
                .page(page)
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Automatic (the language decides)", "auto"),
                        DropdownOption::new("When the project opens", "project"),
                        DropdownOption::new("When a file is first opened", "first_file"),
                    ],
                })
                .validator(Validator::string_one_of(["auto", "project", "first_file"])),
        )
        .setting(
            "idle_stop_minutes",
            SchemaEntry::new(
                "Stop the server after this many minutes with no file of it open (0: never)",
                0_i64,
            )
            .label("Stop when idle (minutes)")
            .page(page)
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(1440.0),
                step: Some(5.0),
            })
            .validator(Validator::int_range(0, 1440)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_config::{ConfigManager, ConfigValue};

    fn settings_with(entries: &[(&str, ConfigValue)]) -> ServerSettings {
        let cfg = ConfigManager::new();
        let handle = cfg
            .register("plugin", "t", server_settings_schema("T", "", &[]))
            .unwrap();
        for (key, value) in entries {
            handle.set(key, value.clone()).unwrap();
        }
        ServerSettings::from_values(|key| handle.get(key).ok())
    }

    #[test]
    fn the_defaults_run_the_server_as_the_spec_says() {
        assert_eq!(settings_with(&[]), ServerSettings::default());
    }

    #[test]
    fn values_are_read_back_typed() {
        let settings = settings_with(&[
            ("enabled", ConfigValue::Bool(false)),
            ("path", ConfigValue::String(" C:/tools/ra.exe ".into())),
            ("args", ConfigValue::String("--log \"a b\" -v".into())),
            (
                "initialization_options",
                ConfigValue::String("{\"check\":{\"command\":\"check\"}}".into()),
            ),
            ("trace", ConfigValue::String("verbose".into())),
            ("start", ConfigValue::String("first_file".into())),
            ("idle_stop_minutes", ConfigValue::Int(15)),
        ]);
        assert!(!settings.enabled);
        assert_eq!(settings.path, Some(PathBuf::from("C:/tools/ra.exe")));
        assert_eq!(settings.args, ["--log", "a b", "-v"]);
        assert_eq!(
            settings.initialization_options,
            Some(serde_json::json!({ "check": { "command": "check" } }))
        );
        assert_eq!(settings.trace, TraceLevel::Verbose);
        assert_eq!(settings.start, StartSetting::FirstFile);
        assert_eq!(settings.idle_stop_minutes, 15);
    }

    #[test]
    fn invalid_initialization_options_are_refused_by_the_schema() {
        let cfg = ConfigManager::new();
        let handle = cfg
            .register("plugin", "t", server_settings_schema("T", "", &[]))
            .unwrap();
        assert!(handle.set("initialization_options", "{ not json").is_err());
        assert!(handle.set("initialization_options", "[1]").is_err());
        assert!(handle.set("initialization_options", "{\"a\":1}").is_ok());
    }

    static OTHER: LanguageServerSpec = LanguageServerSpec {
        id: "t",
        display_name: "Other",
        find: |_| None,
        locate: |_| Err(crate::spec::LocateError::new("not used")),
        roots: crate::spec::RootStrategy::ProjectRoot,
        initialization_options: |_| Value::Null,
        configuration: |_, _| Value::Null,
        client_capabilities: || Value::Null,
        hooks: crate::spec::ServerHooks::DEFAULT,
        readiness: crate::spec::ReadinessRule::Immediate,
        max_indexing: std::time::Duration::from_secs(1),
        eager: false,
        commands: &[],
        prepare_workspace: None,
        installer: None,
    };

    const ALTERNATIVE: ServerAlternative = ServerAlternative {
        origin: "t",
        name: "other",
        label: "Other (experimental)",
        description: "A second server.",
        spec: &OTHER,
    };

    #[test]
    fn the_dropdown_chooses_a_server_and_its_own_launch_settings() {
        let cfg = ConfigManager::new();
        let schema = server_settings_schema("T", "", &[&ALTERNATIVE]);
        let handle = cfg.register("plugin", "t", schema).unwrap();
        handle.set("path", "C:/tools/own.exe").unwrap();
        handle.set("other_args", "--other").unwrap();
        let read = || {
            let mut settings = ServerSettings::from_values(|key| handle.get(key).ok());
            settings.choose_alternative([&ALTERNATIVE], |key| handle.get(key).ok());
            settings
        };
        let own = read();
        assert_eq!(own.alternative, None);
        assert_eq!(own.path, Some(PathBuf::from("C:/tools/own.exe")));
        assert!(own.args.is_empty());

        assert!(handle.set("server", "nobody").is_err());
        handle.set("server", "other").unwrap();
        let other = read();
        assert_eq!(other.alternative, Some("other"));
        assert_eq!(other.path, None, "the other server's path does not apply");
        assert_eq!(other.args, ["--other"]);
        assert!(own.needs_restart(&other));
    }

    #[test]
    fn a_server_without_alternatives_has_no_dropdown() {
        let cfg = ConfigManager::new();
        let handle = cfg
            .register("plugin", "t", server_settings_schema("T", "", &[]))
            .unwrap();
        assert!(handle.get(SERVER_KEY).is_err());
    }

    #[test]
    fn only_path_args_options_and_the_server_need_a_restart() {
        let base = ServerSettings::default();
        let traced = ServerSettings {
            trace: TraceLevel::Verbose,
            ..base.clone()
        };
        let moved = ServerSettings {
            path: Some(PathBuf::from("x")),
            ..base.clone()
        };
        assert!(!base.needs_restart(&traced));
        assert!(base.needs_restart(&moved));
    }
}
