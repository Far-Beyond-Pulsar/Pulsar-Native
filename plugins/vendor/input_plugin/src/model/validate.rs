//! Checking an action or a mapping for mistakes before the game runs.
//!
//! Findings are [`Issue`]s: errors will not work as built, warnings are
//! probably not what was meant, info is worth knowing. Some come with a
//! [`MappingFix`] / [`ActionFix`] the editor offers as one click.

use std::collections::HashMap;

use crate::keys::{catalog, KeyKind};

use super::action::{to_identifier, ActionAsset};
use super::behavior::{Modifier, Tolerant, Trigger, ValueType};
use super::id::{ActionRef, AssetId};
use super::mapping::MappingAsset;
use super::{Issue, Severity};

/// What a mapping needs to know about an action it binds.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionInfo {
    /// The event name (`Jump`).
    pub name: String,
    pub value_type: ValueType,
    /// Where the action's file is now, project-relative with `/`.
    pub path: String,
    pub consume_input: bool,
}

/// Where actions come from: the project's library in the editor, a loaded
/// set in tests or at runtime.
pub trait ActionSource {
    /// The action with this id, if it exists and is unambiguous.
    fn action(&self, id: &AssetId) -> Option<ActionInfo>;

    /// Whether more than one file claims this id.
    fn is_ambiguous(&self, _id: &AssetId) -> bool {
        false
    }

    /// The path of another action that goes by `name`, if there is one.
    fn name_clash(&self, _name: &str, _me: &AssetId) -> Option<String> {
        None
    }
}

/// An [`ActionSource`] with nothing in it.
pub struct NoActions;

impl ActionSource for NoActions {
    fn action(&self, _id: &AssetId) -> Option<ActionInfo> {
        None
    }
}

impl ActionSource for HashMap<AssetId, ActionInfo> {
    fn action(&self, id: &AssetId) -> Option<ActionInfo> {
        self.get(id).cloned()
    }
}

/// A one-click repair for a mapping.
#[derive(Clone, Debug, PartialEq)]
pub enum MappingFix {
    RefreshHint { entry: u32, hint: String },
    RemoveEntry { entry: u32 },
    ClearAction { entry: u32 },
    RenameCategory { category: u32, name: String },
}

/// A one-click repair for an action.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionFix {
    SetIdentifier(String),
    UseDefaultThreshold,
}

impl MappingAsset {
    pub fn apply_fix(&mut self, fix: &MappingFix) {
        match fix {
            MappingFix::RefreshHint { entry, hint } => {
                if let Some(a) = self.entry_mut(*entry).and_then(|e| e.action.as_mut()) {
                    a.hint = hint.clone();
                }
            }
            MappingFix::RemoveEntry { entry } => self.remove_entry(*entry),
            MappingFix::ClearAction { entry } => self.set_action(*entry, None),
            MappingFix::RenameCategory { category, name } => self.rename_category(*category, name),
        }
    }

    pub fn validate(&self, actions: &dyn ActionSource) -> Vec<Issue<MappingFix>> {
        let mut issues = Vec::new();
        self.check_categories(&mut issues);
        self.check_entries(actions, &mut issues);
        self.check_conflicts(actions, &mut issues);
        issues.sort_by(|a, b| b.severity.cmp(&a.severity));
        issues
    }

    fn check_categories(&self, issues: &mut Vec<Issue<MappingFix>>) {
        let mut seen: HashMap<String, u32> = HashMap::new();
        for c in &self.categories {
            if c.name.trim().is_empty() {
                issues.push(
                    Issue::new(Severity::Warning, "Category", "A category has no name")
                        .on_category(c.id)
                        .with_fix(MappingFix::RenameCategory { category: c.id, name: self.unique_category_name("Category") }),
                );
            } else if seen.insert(c.name.to_lowercase(), c.id).is_some() {
                issues.push(
                    Issue::new(Severity::Warning, c.name.clone(), format!("Two categories are called `{}`", c.name))
                        .on_category(c.id)
                        .with_fix(MappingFix::RenameCategory { category: c.id, name: self.unique_category_name(&c.name) }),
                );
            }
        }
    }

    fn check_entries(&self, actions: &dyn ActionSource, issues: &mut Vec<Issue<MappingFix>>) {
        let keys = catalog();
        for (category, entry) in self.entries() {
            let at = |issue: Issue<MappingFix>| issue.on_entry(category.id, entry.id);
            let key = keys.get(&entry.key);
            let subject = if entry.key.is_empty() { "Unbound entry".to_string() } else { key.map_or(entry.key.clone(), |k| k.label.clone()) };

            if entry.key.is_empty() {
                issues.push(at(Issue::new(Severity::Warning, &subject, "No key is chosen, so this entry never fires")));
            } else if key.is_none() {
                issues.push(at(Issue::new(
                    Severity::Error,
                    &subject,
                    format!("`{}` isn't a key this version knows (made by a newer version, or mistyped)", entry.key),
                )));
            }

            let Some(reference) = &entry.action else {
                issues.push(at(Issue::new(Severity::Warning, &subject, "No action is chosen, so this key does nothing")));
                continue;
            };

            let info = if actions.is_ambiguous(&reference.id) {
                issues.push(at(Issue::new(
                    Severity::Error,
                    &subject,
                    "Two action files share this action's id (a copied file), so it can't tell which to use",
                )
                .with_fix(MappingFix::ClearAction { entry: entry.id })));
                None
            } else {
                actions.action(&reference.id)
            };
            let Some(info) = info else {
                if !actions.is_ambiguous(&reference.id) {
                    let was = if reference.hint.is_empty() { String::new() } else { format!(" (it was `{}`)", reference.hint) };
                    issues.push(at(Issue::new(Severity::Error, &subject, format!("The action this key triggers can't be found{was}"))
                        .with_fix(MappingFix::ClearAction { entry: entry.id })));
                }
                continue;
            };

            if reference.hint != info.path {
                issues.push(at(Issue::new(
                    Severity::Info,
                    &subject,
                    format!("`{}` has moved to `{}`; the file's note of where it was is out of date", info.name, info.path),
                )
                .with_fix(MappingFix::RefreshHint { entry: entry.id, hint: info.path.clone() })));
            }

            if let Some(key) = key {
                if let Some(message) = conversion_note(key.kind, info.value_type, entry.modifiers.is_empty()) {
                    issues.push(at(Issue::new(Severity::Info, format!("{} → {}", key.label, info.name), message)));
                }
            }

            for trigger in entry.triggers.iter().filter_map(Tolerant::known) {
                if let Trigger::Chorded { action } = trigger {
                    check_chord(action, &info.name, &reference.id, actions, &subject, issues, |i| at(i));
                }
            }
        }

        // The same key bound to the same action twice.
        let mut seen: HashMap<(String, AssetId), u32> = HashMap::new();
        for (category, entry) in self.entries() {
            let Some(action) = entry.action_id() else { continue };
            if entry.key.is_empty() || !entry.enabled {
                continue;
            }
            if seen.insert((entry.key.clone(), action.clone()), entry.id).is_some() {
                let label = keys.get(&entry.key).map_or(entry.key.clone(), |k| k.label.clone());
                issues.push(
                    Issue::new(Severity::Warning, label, "This key is bound to the same action more than once")
                        .on_entry(category.id, entry.id)
                        .with_fix(MappingFix::RemoveEntry { entry: entry.id }),
                );
            }
        }
    }

    /// One key driving two different actions, when neither is guarded by a
    /// chord or gesture that would tell them apart.
    fn check_conflicts(&self, actions: &dyn ActionSource, issues: &mut Vec<Issue<MappingFix>>) {
        let mut by_key: HashMap<&str, Vec<(u32, u32, &AssetId)>> = HashMap::new();
        for (category, entry) in self.entries() {
            let Some(action) = entry.action_id() else { continue };
            if entry.key.is_empty() || !entry.enabled || !entry.triggers.is_empty() {
                continue;
            }
            by_key.entry(entry.key.as_str()).or_default().push((category.id, entry.id, action));
        }
        for (key, users) in by_key {
            let distinct: Vec<_> = users.iter().map(|u| u.2).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            if distinct.len() < 2 {
                continue;
            }
            let label = catalog().get(key).map_or(key.to_string(), |k| k.label.clone());
            let name_of = |id: &AssetId| actions.action(id).map_or_else(|| "an unknown action".to_string(), |a| a.name);
            // Report on every entry after the first, naming the first.
            let first_action = users[0].2;
            for &(category, entry, action) in users.iter().skip(1) {
                if action != first_action {
                    issues.push(
                        Issue::new(
                            Severity::Warning,
                            label.clone(),
                            format!("{label} also drives {}; both fire together unless one consumes the key or has a trigger that tells them apart", name_of(first_action)),
                        )
                        .on_entry(category, entry),
                    );
                }
            }
        }
    }
}

fn check_chord(
    chord: &ActionRef,
    owner_name: &str,
    owner: &AssetId,
    actions: &dyn ActionSource,
    subject: &str,
    issues: &mut Vec<Issue<MappingFix>>,
    at: impl Fn(Issue<MappingFix>) -> Issue<MappingFix>,
) {
    if !chord.id.is_assigned() {
        issues.push(at(Issue::new(Severity::Warning, subject, format!("A chord on `{owner_name}` has no action chosen"))));
    } else if &chord.id == owner {
        issues.push(at(Issue::new(Severity::Error, subject, format!("`{owner_name}` can't be chorded with itself"))));
    } else if actions.action(&chord.id).is_none() {
        issues.push(at(Issue::new(Severity::Error, subject, format!("The action chorded with `{owner_name}` can't be found"))));
    }
}

/// What gets lost when `key` drives an action of `value`, if anything worth
/// saying. `plain` is true when the entry has no modifiers of its own to fix it.
fn conversion_note(key: KeyKind, value: ValueType, plain: bool) -> Option<&'static str> {
    match (key, value) {
        (KeyKind::Axis2D, ValueType::Axis1D) => Some("A 2D key drives a 1D action: only X is used. Add a Swizzle to use Y instead."),
        (KeyKind::Axis2D, ValueType::Bool) => Some("The action counts as pressed when the stick or mouse moves past the threshold, in any direction."),
        (KeyKind::Button | KeyKind::Axis1D, ValueType::Axis2D | ValueType::Axis3D) if plain => {
            Some("This key only drives X. Use `Add direction keys`, or a Swizzle, to point it somewhere else.")
        }
        _ => None,
    }
}

impl ActionAsset {
    pub fn apply_fix(&mut self, fix: &ActionFix) {
        match fix {
            ActionFix::SetIdentifier(name) => self.identifier = name.clone(),
            ActionFix::UseDefaultThreshold => self.actuation_threshold = super::action::DEFAULT_THRESHOLD,
        }
    }

    /// `file_stem` is the file's name without its extension; `others` finds
    /// other actions for name clashes and chords.
    pub fn validate(&self, file_stem: &str, others: &dyn ActionSource) -> Vec<Issue<ActionFix>> {
        let mut issues = Vec::new();
        let name = self.event_name(file_stem);

        if !self.identifier.trim().is_empty() && !ActionAsset::is_valid_identifier(self.identifier.trim()) {
            issues.push(
                Issue::new(Severity::Error, "Script name", format!("`{}` can't be an event name: use letters, digits and underscores, and don't start with a digit", self.identifier))
                    .with_fix(ActionFix::SetIdentifier(to_identifier(&self.identifier))),
            );
        }
        if let Some(path) = others.name_clash(&name, &self.id) {
            issues.push(Issue::new(
                Severity::Error,
                "Script name",
                format!("`{path}` is also called `{name}`, so scripts can't tell their events apart. Give one a different script name."),
            ));
        }
        if self.actuation_threshold <= 0.0 {
            issues.push(
                Issue::new(Severity::Warning, "Actuation threshold", "At 0 the action counts as pressed even at rest")
                    .with_fix(ActionFix::UseDefaultThreshold),
            );
        }

        let mut kinds = Vec::new();
        for trigger in self.known_triggers() {
            let kind = trigger.kind();
            if kinds.contains(&kind) {
                issues.push(Issue::new(Severity::Info, kind.label(), format!("There is more than one {} trigger", kind.label())));
            }
            kinds.push(kind);
            match trigger {
                Trigger::Hold { seconds, .. } | Trigger::HoldAndRelease { seconds } if *seconds <= 0.0 => {
                    issues.push(Issue::new(Severity::Warning, kind.label(), "A hold of 0 seconds is the same as Down"));
                }
                Trigger::Tap { max_seconds } if *max_seconds <= 0.0 => {
                    issues.push(Issue::new(Severity::Warning, kind.label(), "A tap of 0 seconds can never happen"));
                }
                Trigger::Pulse { interval, .. } if *interval <= 0.0 => {
                    issues.push(Issue::new(Severity::Warning, kind.label(), "A pulse needs an interval above 0 (it is held to one frame)"));
                }
                Trigger::DoubleTap { window } if *window <= 0.0 => {
                    issues.push(Issue::new(Severity::Warning, kind.label(), "A double tap needs a window above 0"));
                }
                Trigger::Chorded { action } => {
                    if !action.id.is_assigned() {
                        issues.push(Issue::new(Severity::Warning, kind.label(), "No action is chosen for the chord"));
                    } else if action.id == self.id {
                        issues.push(Issue::new(Severity::Error, kind.label(), "An action can't be chorded with itself"));
                    } else if others.action(&action.id).is_none() {
                        issues.push(Issue::new(Severity::Error, kind.label(), "The action this is chorded with can't be found"));
                    }
                }
                _ => {}
            }
        }
        if self.triggers.iter().any(|t| t.known().is_none()) || self.modifiers.iter().any(|m| m.known().is_none()) {
            issues.push(Issue::new(Severity::Info, "Newer features", "Made by a newer version: kept as is, but not evaluated"));
        }
        for modifier in self.known_modifiers() {
            match modifier {
                Modifier::DeadZone { lower, upper, .. } if lower >= upper => {
                    issues.push(Issue::new(Severity::Error, "Dead zone", "The lower bound must be below the upper"));
                }
                Modifier::Clamp { min, max } if min > max => {
                    issues.push(Issue::new(Severity::Error, "Clamp", "The minimum is above the maximum"));
                }
                Modifier::Smooth { speed } if *speed <= 0.0 => {
                    issues.push(Issue::new(Severity::Warning, "Smooth", "A speed of 0 never moves"));
                }
                _ => {}
            }
        }
        issues.sort_by(|a, b| b.severity.cmp(&a.severity));
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::behavior::Swizzle;

    fn action(name: &str, value_type: ValueType) -> (ActionRef, (AssetId, ActionInfo)) {
        let id = AssetId::new();
        let path = format!("input/{name}.inputaction");
        (
            ActionRef::new(id.clone(), path.clone()),
            (id, ActionInfo { name: name.to_string(), value_type, path, consume_input: true }),
        )
    }

    fn source(items: Vec<(AssetId, ActionInfo)>) -> HashMap<AssetId, ActionInfo> {
        items.into_iter().collect()
    }

    fn messages<F>(issues: &[Issue<F>]) -> Vec<String> {
        issues.iter().map(|i| i.message.clone()).collect()
    }

    #[test]
    fn a_clean_mapping_has_no_issues() {
        let (jump, info) = action("Jump", ValueType::Bool);
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        m.add_entry(cat, "Keyboard.Space", Some(jump.clone()));
        m.add_entry(cat, "Gamepad.FaceBottom", Some(jump));
        assert_eq!(m.validate(&source(vec![info])), vec![]);
    }

    #[test]
    fn missing_actions_and_unknown_keys_are_errors_and_blank_entries_warnings() {
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        m.add_entry(cat, "Keyboard.Space", Some(ActionRef::new(AssetId::new(), "gone.inputaction")));
        m.add_entry(cat, "Keyboard.Banana", None);
        m.add_entry(cat, "", None);
        let issues = m.validate(&NoActions);
        let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
        let warnings = issues.iter().filter(|i| i.severity == Severity::Warning).count();
        assert_eq!(errors, 2, "{:?}", messages(&issues));
        assert_eq!(warnings, 3, "{:?}", messages(&issues));
        assert_eq!(issues[0].severity, Severity::Error, "worst first");
    }

    #[test]
    fn a_moved_action_offers_to_refresh_the_hint_and_the_fix_works() {
        let (mut jump, info) = action("Jump", ValueType::Bool);
        jump.hint = "old/Jump.inputaction".into();
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        let entry = m.add_entry(cat, "Keyboard.Space", Some(jump)).unwrap();
        let source = source(vec![info.clone()]);
        let issues = m.validate(&source);
        assert_eq!(issues.len(), 1);
        m.apply_fix(issues[0].fix.as_ref().unwrap());
        assert_eq!(m.entry(entry).unwrap().action.as_ref().unwrap().hint, info.1.path);
        assert!(m.validate(&source).is_empty());
    }

    #[test]
    fn two_actions_on_one_key_warn_unless_a_trigger_tells_them_apart() {
        let (jump, a) = action("Jump", ValueType::Bool);
        let (dash, b) = action("Dash", ValueType::Bool);
        let src = source(vec![a, b]);
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        m.add_entry(cat, "Keyboard.Space", Some(jump));
        let second = m.add_entry(cat, "Keyboard.Space", Some(dash)).unwrap();
        let issues = m.validate(&src);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].entry, Some(second));
        assert!(issues[0].message.contains("Jump"));
        m.entry_mut(second).unwrap().triggers = vec![Tolerant::Known(Trigger::DoubleTap { window: 0.3 })];
        assert!(m.validate(&src).is_empty());
    }

    #[test]
    fn the_same_binding_twice_is_flagged_with_a_removal() {
        let (jump, a) = action("Jump", ValueType::Bool);
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        m.add_entry(cat, "Keyboard.Space", Some(jump.clone()));
        let dup = m.add_entry(cat, "Keyboard.Space", Some(jump)).unwrap();
        let src = source(vec![a]);
        let issues = m.validate(&src);
        assert_eq!(issues.len(), 1);
        m.apply_fix(issues[0].fix.as_ref().unwrap());
        assert!(m.entry(dup).is_none());
        assert!(m.validate(&src).is_empty());
    }

    #[test]
    fn lossy_key_to_action_pairings_are_explained() {
        let (mv, a) = action("Move", ValueType::Axis2D);
        let (zoom, z) = action("Zoom", ValueType::Axis1D);
        let src = source(vec![a, z]);
        let mut m = MappingAsset::new();
        let cat = m.categories[0].id;
        m.add_entry(cat, "Keyboard.KeyW", Some(mv.clone()));
        m.add_entry(cat, "Gamepad.LeftStick", Some(zoom));
        let w = m.add_entry(cat, "Keyboard.KeyS", Some(mv)).unwrap();
        m.entry_mut(w).unwrap().modifiers = vec![Tolerant::Known(Modifier::Swizzle { order: Swizzle::Yxz })];
        let issues = m.validate(&src);
        assert_eq!(issues.len(), 2, "{:?}", messages(&issues));
        assert!(issues.iter().all(|i| i.severity == Severity::Info));
    }

    #[test]
    fn duplicate_category_names_get_a_rename() {
        let mut m = MappingAsset::new();
        let second = m.add_category("Other");
        m.rename_category(second, "general");
        let issues = m.validate(&NoActions);
        assert_eq!(issues.len(), 1);
        m.apply_fix(issues[0].fix.as_ref().unwrap());
        assert!(m.validate(&NoActions).is_empty());
    }

    #[test]
    fn an_action_checks_its_name_threshold_and_chord() {
        let mut a = ActionAsset::new();
        a.identifier = "my action".into();
        a.actuation_threshold = 0.0;
        a.triggers = vec![Tolerant::Known(Trigger::Chorded { action: ActionRef::new(a.id.clone(), "") })];
        let issues = a.validate("Sprint", &NoActions);
        assert_eq!(issues.iter().filter(|i| i.severity == Severity::Error).count(), 2);
        assert!(issues.iter().any(|i| i.severity == Severity::Warning));
        for issue in &issues {
            if let Some(fix) = &issue.fix {
                a.apply_fix(fix);
            }
        }
        assert_eq!(a.identifier, "MyAction");
        assert!(a.actuation_threshold > 0.0);
    }

    #[test]
    fn two_actions_with_one_event_name_are_an_error() {
        struct Clash;
        impl ActionSource for Clash {
            fn action(&self, _: &AssetId) -> Option<ActionInfo> {
                None
            }
            fn name_clash(&self, name: &str, _: &AssetId) -> Option<String> {
                (name == "Jump").then(|| "other/Jump.inputaction".to_string())
            }
        }
        let issues = ActionAsset::new().validate("Jump", &Clash);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
        assert!(ActionAsset::new().validate("Run", &Clash).is_empty());
    }

    #[test]
    fn nonsensical_settings_are_flagged() {
        let mut a = ActionAsset::new();
        a.modifiers = vec![
            Tolerant::Known(Modifier::DeadZone { lower: 0.9, upper: 0.5, radial: true }),
            Tolerant::Known(Modifier::Clamp { min: 2.0, max: 1.0 }),
        ];
        a.triggers = vec![Tolerant::Known(Trigger::Tap { max_seconds: 0.0 }), Tolerant::Known(Trigger::Pulse { interval: 0.0, limit: 0, on_start: true })];
        let issues = a.validate("X", &NoActions);
        assert_eq!(issues.len(), 4, "{:?}", messages(&issues));
    }
}
