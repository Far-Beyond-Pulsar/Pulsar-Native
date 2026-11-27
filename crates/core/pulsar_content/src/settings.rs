//! Runtime project settings: `Pulsar/project.json`.
//!
//! What a game needs to start, read the same way from a project (dev) and
//! from shipped content (the packager writes a cooked copy with
//! [`BuildProfile`] and a content-relative `startup_level` filled in):
//!
//! ```json
//! {
//!   "name": "My Game",
//!   "startup_level": "scenes/main.level",
//!   "profile": "dev",
//!   "window": { "title": "My Game", "width": 1280, "height": 720 },
//!   "scripting": {
//!     "dev":      { "instruction_budget": 1000000, "max_call_depth": 256, "checked_arithmetic": true },
//!     "shipping": { "instruction_budget": 100000,  "max_call_depth": 64,  "checked_arithmetic": false },
//!     "allowed_capabilities": null,
//!     "class_budgets": { "BossAI": 500000 }
//!   }
//! }
//! ```
//!
//! Every field is optional. Global scripts stay in `Pulsar/scripting.json`.
//! A missing or unreadable file means the defaults.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The settings file, content-relative.
pub const PROJECT_SETTINGS_FILE: &str = "Pulsar/project.json";

/// The project settings file is not valid settings JSON.
#[derive(Debug, thiserror::Error)]
pub struct SettingsError(#[source] serde_json::Error);

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{PROJECT_SETTINGS_FILE}: {}", self.0)
    }
}

/// Which script limits and checks apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    /// Editor, Play-in-Editor, `cargo run`, `--profile dev` packages.
    #[default]
    Dev,
    /// `--profile shipping` packages.
    Shipping,
}

impl BuildProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Shipping => "shipping",
        }
    }
}

impl std::str::FromStr for BuildProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dev" | "development" | "debug" => Ok(Self::Dev),
            "shipping" | "release" | "ship" => Ok(Self::Shipping),
            other => Err(format!(
                "unknown profile `{other}` (expected `dev` or `shipping`)"
            )),
        }
    }
}

/// `Pulsar/project.json`. See the module doc.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// Display name (window title fallback).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The level loaded at startup, content-relative (`scenes/main.level`).
    /// Absent means "the default level files"; the empty string means
    /// explicitly none ([`Self::startup_choice`]). The two are different and
    /// both survive cooking, so the string is kept as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_level: Option<String>,
    /// The game instance class: a Blueprint class implementing the
    /// `pulsar.GameInstance` trait, spawned before any level loads and kept
    /// across level loads. In a project, the class folder
    /// (`classes/MainGame.class`); in cooked content, its class id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_instance: Option<String>,
    pub profile: BuildProfile,
    pub window: WindowSettings,
    pub simulation: SimulationSettings,
    pub scripting: ScriptSettings,
}

impl ProjectSettings {
    /// Parse settings; `Err` carries the parse error.
    pub fn from_json(bytes: &[u8]) -> Result<Self, SettingsError> {
        serde_json::from_slice(bytes).map_err(SettingsError)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// The script limits of this settings' own [`profile`](Self::profile).
    pub fn script_limits(&self) -> &ScriptProfileLimits {
        self.scripting.limits_for(self.profile)
    }

    /// What `startup_level` says.
    pub fn startup_choice(&self) -> StartupChoice<'_> {
        match self.startup_level.as_deref().map(str::trim) {
            None => StartupChoice::Default,
            Some("") => StartupChoice::None,
            Some(level) => StartupChoice::Level(level),
        }
    }

    /// The game instance class named, if any.
    pub fn game_instance_class(&self) -> Option<&str> {
        self.game_instance
            .as_deref()
            .map(str::trim)
            .filter(|class| !class.is_empty())
    }

    /// The window title: `window.title`, else `name`, else `fallback`.
    pub fn window_title(&self, fallback: &str) -> String {
        self.window
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| (!self.name.is_empty()).then(|| self.name.clone()))
            .unwrap_or_else(|| fallback.to_owned())
    }
}

/// The level a project asks to start in (`startup_level`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupChoice<'a> {
    /// The key is absent: the project's default level
    /// ([`DEFAULT_LEVELS`], after the editor's `project.default_map`).
    Default,
    /// The key is the empty string: no level. Nothing loads until the game
    /// asks for a level.
    None,
    /// This level, content-relative.
    Level(&'a str),
}

/// The level files a project without a `startup_level` starts in, in order,
/// after the editor's `project.default_map` setting.
pub const DEFAULT_LEVELS: [&str; 3] = [
    "scene/default.level",
    "scenes/default.level",
    "scenes/default_level.json",
];

/// The level a project with no `startup_level` starts in: `default_map` (the
/// editor's setting) when it exists, else the first existing
/// [`DEFAULT_LEVELS`] entry. `exists` answers for a content-relative path.
pub fn default_level(default_map: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<String> {
    default_map
        .filter(|map| !map.is_empty() && exists(map))
        .map(str::to_owned)
        .or_else(|| {
            DEFAULT_LEVELS
                .into_iter()
                .find(|rel| exists(rel))
                .map(str::to_owned)
        })
}

/// Change keys of a project's `Pulsar/project.json` in place, keeping every
/// key `edit` does not touch (the ones this engine does not know included).
/// A missing file is created; one that is not a JSON object is refused and
/// left as it is. The file is written only when `edit` changed something.
pub fn edit_project_file(
    project_root: &std::path::Path,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> Result<(), String> {
    let path = project_root.join(PROJECT_SETTINGS_FILE);
    let before = read_project_file(project_root)?;
    let mut after = before.clone();
    edit(&mut after);
    if after == before {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(after))
        .map_err(|error| format!("{PROJECT_SETTINGS_FILE}: {error}"))?;
    if let Some(parent) = path.parent() {
        engine_fs::virtual_fs::create_dir_all(parent)
            .map_err(|error| format!("{PROJECT_SETTINGS_FILE}: {error:#}"))?;
    }
    engine_fs::virtual_fs::write_file_atomically(&path, format!("{text}\n").as_bytes())
        .map_err(|error| format!("{PROJECT_SETTINGS_FILE}: {error:#}"))
}

/// The keys of a project's `Pulsar/project.json`, as written; empty when
/// there is no file.
pub fn read_project_file(
    project_root: &std::path::Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let path = project_root.join(PROJECT_SETTINGS_FILE);
    if !engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
        return Ok(Default::default());
    }
    let bytes = engine_fs::virtual_fs::read_file(&path)
        .map_err(|error| format!("{PROJECT_SETTINGS_FILE}: {error:#}"))?;
    match serde_json::from_slice(&bytes) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{PROJECT_SETTINGS_FILE}: not a JSON object")),
        Err(error) => Err(format!("{PROJECT_SETTINGS_FILE}: {error}")),
    }
}

/// Primary window defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub resizable: bool,
    /// How the window's frames reach the display.
    pub present_mode: PresentModeSetting,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: None,
            width: 1280,
            height: 720,
            fullscreen: false,
            resizable: true,
            present_mode: PresentModeSetting::default(),
        }
    }
}

/// How a window's frames reach the display. A mode the display does not offer
/// uses `fifo`, which every display has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentModeSetting {
    /// Wait for the display's refresh; no tearing.
    #[default]
    Fifo,
    /// Like `fifo`, but a late frame is shown at once (it may tear).
    FifoRelaxed,
    /// The newest frame replaces the one waiting: no tearing, the least delay the
    /// compositor allows, where offered.
    Mailbox,
    /// Show the frame at once; it may tear.
    Immediate,
    /// `fifo` or whatever vsynced mode the display offers.
    AutoVsync,
    /// The least delay the display offers.
    AutoNoVsync,
}

/// How the simulation and the view meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimulationSettings {
    /// The frame turns the player's view by the mouse motion that arrived after the
    /// last tick, when it reads the camera, and the next tick applies the same
    /// motion to the simulation (the late-latched look, `pulsar_game::late_look`):
    /// the view shows the mouse as of the frame, a few milliseconds fresher. A pawn
    /// whose look cannot be told ahead (a smoothed or bent look mapping, a stick)
    /// shows the tick's view either way.
    pub late_look: bool,
}

impl Default for SimulationSettings {
    fn default() -> Self {
        Self { late_look: true }
    }
}

/// Script VM limits of one profile. In the file, a profile
/// may set only some fields; the others keep that profile's defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScriptProfileLimits {
    /// Instructions one event on one instance may run.
    pub instruction_budget: u64,
    /// Script call depth limit.
    pub max_call_depth: u32,
    /// Integer overflow is a runtime error instead of wrapping.
    pub checked_arithmetic: bool,
}

impl ScriptProfileLimits {
    /// Editor / Play-in-Editor / dev builds: generous, checked.
    pub fn dev() -> Self {
        Self {
            instruction_budget: 1_000_000,
            max_call_depth: 256,
            checked_arithmetic: true,
        }
    }

    /// Shipping builds: strict, unchecked (like a Rust release build).
    pub fn shipping() -> Self {
        Self {
            instruction_budget: 100_000,
            max_call_depth: 64,
            checked_arithmetic: false,
        }
    }
}

/// A profile as written: any field may be missing.
#[derive(Deserialize)]
struct PartialLimits {
    instruction_budget: Option<u64>,
    max_call_depth: Option<u32>,
    checked_arithmetic: Option<bool>,
}

impl PartialLimits {
    fn over(self, base: ScriptProfileLimits) -> ScriptProfileLimits {
        ScriptProfileLimits {
            instruction_budget: self.instruction_budget.unwrap_or(base.instruction_budget),
            max_call_depth: self.max_call_depth.unwrap_or(base.max_call_depth),
            checked_arithmetic: self.checked_arithmetic.unwrap_or(base.checked_arithmetic),
        }
    }
}

fn dev_limits<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ScriptProfileLimits, D::Error> {
    Ok(PartialLimits::deserialize(d)?.over(ScriptProfileLimits::dev()))
}

fn shipping_limits<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<ScriptProfileLimits, D::Error> {
    Ok(PartialLimits::deserialize(d)?.over(ScriptProfileLimits::shipping()))
}

/// Script settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptSettings {
    #[serde(default = "ScriptProfileLimits::dev", deserialize_with = "dev_limits")]
    pub dev: ScriptProfileLimits,
    #[serde(
        default = "ScriptProfileLimits::shipping",
        deserialize_with = "shipping_limits"
    )]
    pub shipping: ScriptProfileLimits,
    /// Native capabilities scripts may import: `null` (the
    /// default) allows every capability, a list allows only those.
    /// Checked when classes link.
    pub allowed_capabilities: Option<Vec<String>>,
    /// Per-class instruction budgets, by class name, in every profile.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub class_budgets: BTreeMap<String, u64>,
}

impl Default for ScriptSettings {
    fn default() -> Self {
        Self {
            dev: ScriptProfileLimits::dev(),
            shipping: ScriptProfileLimits::shipping(),
            allowed_capabilities: None,
            class_budgets: BTreeMap::new(),
        }
    }
}

impl ScriptSettings {
    pub fn limits_for(&self, profile: BuildProfile) -> &ScriptProfileLimits {
        match profile {
            BuildProfile::Dev => &self.dev,
            BuildProfile::Shipping => &self.shipping,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_presents_with_fifo_and_the_view_is_late_looked_unless_the_project_says_otherwise() {
        let defaults = ProjectSettings::from_json(b"{}").unwrap();
        assert_eq!(defaults.window.present_mode, PresentModeSetting::Fifo);
        assert!(defaults.simulation.late_look);
        let set = ProjectSettings::from_json(
            br#"{ "window": { "present_mode": "mailbox" }, "simulation": { "late_look": false } }"#,
        )
        .unwrap();
        assert_eq!(set.window.present_mode, PresentModeSetting::Mailbox);
        assert!(!set.simulation.late_look);
        assert_eq!(set.window.width, 1280, "the rest of the window keeps its defaults");
        let back = ProjectSettings::from_json(set.to_json().as_bytes()).unwrap();
        assert_eq!(back, set);
    }

    #[test]
    fn the_startup_level_keeps_absent_and_none_apart() {
        let read = |json: &str| ProjectSettings::from_json(json.as_bytes()).unwrap();
        assert_eq!(read("{}").startup_choice(), StartupChoice::Default);
        let none = read(r#"{ "startup_level": "" }"#);
        assert_eq!(none.startup_choice(), StartupChoice::None);
        assert!(none.to_json().contains(r#""startup_level": """#), "{}", none.to_json());
        assert_eq!(
            read(r#"{ "startup_level": "a.level" }"#).startup_choice(),
            StartupChoice::Level("a.level")
        );
        assert_eq!(read("{}").game_instance_class(), None);
        assert_eq!(read(r#"{ "game_instance": " " }"#).game_instance_class(), None);
        assert_eq!(
            read(r#"{ "game_instance": "classes/Main.class" }"#).game_instance_class(),
            Some("classes/Main.class")
        );
    }

    #[test]
    fn the_default_level_is_the_editors_map_then_the_conventional_files() {
        let exists = |rel: &str| rel == "scenes/default.level" || rel == "maps/a.level";
        assert_eq!(default_level(Some("maps/a.level"), exists).as_deref(), Some("maps/a.level"));
        assert_eq!(
            default_level(Some("maps/gone.level"), exists).as_deref(),
            Some("scenes/default.level")
        );
        assert_eq!(default_level(None, |_| false), None);
    }

    #[test]
    fn editing_the_project_file_keeps_the_keys_it_does_not_touch() {
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join(PROJECT_SETTINGS_FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{ "future_key": [1, 2], "startup_level": "a.level" }"#).unwrap();
        edit_project_file(project.path(), |map| {
            map.insert("startup_level".into(), "".into());
            map.insert("game_instance".into(), "classes/Main.class".into());
        })
        .unwrap();
        let map = read_project_file(project.path()).unwrap();
        assert_eq!(map["future_key"], serde_json::json!([1, 2]));
        assert_eq!(map["startup_level"], serde_json::json!(""));
        edit_project_file(project.path(), |map| {
            map.remove("startup_level");
        })
        .unwrap();
        let map = read_project_file(project.path()).unwrap();
        assert!(!map.contains_key("startup_level"));
        assert_eq!(map["game_instance"], serde_json::json!("classes/Main.class"));
        std::fs::write(&file, "[1]").unwrap();
        assert!(edit_project_file(project.path(), |_| {}).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[1]", "left as it is");
    }

    #[test]
    fn missing_fields_take_defaults() {
        let settings = ProjectSettings::from_json(br#"{ "startup_level": "scenes/a.level", "scripting": { "shipping": { "instruction_budget": 5 } } }"#).unwrap();
        assert_eq!(settings.startup_level.as_deref(), Some("scenes/a.level"));
        assert_eq!(settings.profile, BuildProfile::Dev);
        assert_eq!(settings.window, WindowSettings::default());
        assert_eq!(settings.scripting.dev, ScriptProfileLimits::dev());
        let shipping = settings.scripting.limits_for(BuildProfile::Shipping);
        assert_eq!(shipping.instruction_budget, 5);
        assert!(
            !shipping.checked_arithmetic,
            "unset fields keep the profile's own defaults"
        );
        assert_eq!(
            shipping.max_call_depth,
            ScriptProfileLimits::shipping().max_call_depth
        );
        assert_eq!(
            ProjectSettings::from_json(b"{}").unwrap(),
            ProjectSettings::default()
        );
        assert!(
            !ProjectSettings::default()
                .scripting
                .shipping
                .checked_arithmetic
        );
    }

    #[test]
    fn round_trips_and_names_the_window() {
        let mut settings = ProjectSettings {
            name: "Game".into(),
            profile: BuildProfile::Shipping,
            ..Default::default()
        };
        settings.scripting.allowed_capabilities = Some(vec!["fs".into()]);
        let back = ProjectSettings::from_json(settings.to_json().as_bytes()).unwrap();
        assert_eq!(back, settings);
        assert_eq!(back.script_limits(), &ScriptProfileLimits::shipping());
        assert_eq!(back.window_title("fallback"), "Game");
        assert_eq!(
            "release".parse::<BuildProfile>().unwrap(),
            BuildProfile::Shipping
        );
    }
}
