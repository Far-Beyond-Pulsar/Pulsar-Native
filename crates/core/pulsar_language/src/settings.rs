//! The standard settings group every language server gets (D10): the same
//! controls for every server, registered as plugin settings
//! (`pulsar_settings::plugin`) and read back here as [`ServerSettings`].

use std::path::PathBuf;

use pulsar_config::{DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};
use serde_json::Value;

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
        Self::from_values(|key| handle.value(key))
    }

    /// Build from a key lookup (what the tests drive).
    pub fn from_values(value: impl Fn(&str) -> Option<pulsar_config::ConfigValue>) -> Self {
        let text = |key: &str| {
            value(key)
                .and_then(|v| v.as_str().ok().map(str::to_owned))
                .unwrap_or_default()
        };
        let path = text("path");
        let init = text("initialization_options");
        Self {
            enabled: value("enabled")
                .and_then(|v| v.as_bool().ok())
                .unwrap_or(true),
            path: (!path.trim().is_empty()).then(|| PathBuf::from(path.trim())),
            args: split_args(&text("args")),
            initialization_options: (!init.trim().is_empty())
                .then(|| serde_json::from_str(&init).ok())
                .flatten(),
            trace: TraceLevel::parse(&text("trace")),
            start: StartSetting::parse(&text("start")),
            idle_stop_minutes: value("idle_stop_minutes")
                .and_then(|v| v.as_int().ok())
                .unwrap_or(0)
                .clamp(0, 24 * 60) as u32,
        }
    }

    /// Whether `other` differs in a way that needs the process restarted.
    pub fn needs_restart(&self, other: &Self) -> bool {
        self.path != other.path
            || self.args != other.args
            || self.initialization_options != other.initialization_options
    }
}

/// The standard group: every server exposes the same controls.
pub fn server_settings_schema(display_name: &str, description: &str) -> NamespaceSchema {
    let page = "Language server";
    NamespaceSchema::new(display_name, description)
        .setting(
            "enabled",
            SchemaEntry::new("Run this language server", true)
                .label("Enabled")
                .page(page)
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "path",
            SchemaEntry::new(
                "Executable to use instead of searching for one (empty: find or install it)",
                "",
            )
            .label("Executable path")
            .page(page)
            .field_type(FieldType::PathSelector { directory: false })
            .requires_restart(),
        )
        .setting(
            "args",
            SchemaEntry::new("Extra command-line arguments", "")
                .label("Arguments")
                .page(page)
                .field_type(FieldType::TextInput {
                    placeholder: Some("--flag value".into()),
                    multiline: false,
                }),
        )
        .setting(
            "initialization_options",
            SchemaEntry::new(
                "JSON merged over the server's initialization options (empty: none)",
                "",
            )
            .label("Initialization options")
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
            .register("plugin", "t", server_settings_schema("T", ""))
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
            .register("plugin", "t", server_settings_schema("T", ""))
            .unwrap();
        assert!(handle
            .set("initialization_options", "{ not json")
            .is_err());
        assert!(handle.set("initialization_options", "[1]").is_err());
        assert!(handle.set("initialization_options", "{\"a\":1}").is_ok());
    }

    #[test]
    fn only_path_args_and_options_need_a_restart() {
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
