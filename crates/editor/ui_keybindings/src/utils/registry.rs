//! The bridge to GPUI: read the live keymap and the action registry, and bind
//! the user's overrides over the defaults.
//!
//! Bindings added from the user's file carry [`USER_META`]. That tag is how a
//! rebuild tells them from everything else, so it never needs a stored copy of
//! the defaults: bindings a plugin adds later are simply untagged and survive.
//! It is also the value (`0`) the keymap itself treats as "a user binding" when
//! deciding whether an unbinding (`NoAction`) blocks lower-precedence matches.

use gpui::{
    Action, App, Global, KeyBinding, KeyBindingContextPredicate, KeyBindingMetaIndex, Keystroke, NoAction,
    is_no_action,
};

use super::overrides::{Binding, Entry, Override};

pub const USER_META: KeyBindingMetaIndex = KeyBindingMetaIndex(0);

/// Keystrokes in the form `Keystroke::parse` reads back, so two spellings of
/// one chord compare equal.
pub fn normalize_keystrokes(text: &str) -> Result<String, String> {
    let strokes: Vec<String> = text
        .split_whitespace()
        .map(|stroke| {
            Keystroke::parse(stroke)
                .map(|parsed| parsed.unparse())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    if strokes.is_empty() {
        return Err("no keys".to_owned());
    }
    Ok(strokes.join(" "))
}

/// A context in the form `KeyBindingContextPredicate` prints; blank is none.
pub fn normalize_context(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    KeyBindingContextPredicate::parse(text)
        .map(|predicate| Some(predicate.to_string()))
        .map_err(|error| error.to_string())
}

fn keystrokes_of(binding: &KeyBinding) -> String {
    binding
        .keystrokes()
        .iter()
        .map(|stroke| stroke.inner().unparse())
        .collect::<Vec<_>>()
        .join(" ")
}

fn entry_of(binding: &KeyBinding) -> Entry {
    let keystrokes = keystrokes_of(binding);
    let context = binding.predicate().map(|predicate| predicate.to_string());
    if is_no_action(binding.action()) {
        return Entry::Mask {
            keystrokes,
            context,
        };
    }
    Entry::Bind(Binding {
        action: binding.action().name().to_owned(),
        keystrokes,
        context,
        params: binding
            .action_input()
            .and_then(|input| serde_json::from_str(&input).ok()),
    })
}

/// Defaults the user's file removed from the keymap. Kept so that changing the
/// file again (resetting an action, say) can bring them back.
#[derive(Default)]
struct RemovedDefaults(Vec<KeyBinding>);

impl Global for RemovedDefaults {}

/// Everything the keymap holds that did not come from the user's file: what
/// the editor ships with, what plugins added, and what the file removed.
fn base_bindings(cx: &App) -> Vec<KeyBinding> {
    let mut out = cx
        .try_global::<RemovedDefaults>()
        .map(|removed| removed.0.clone())
        .unwrap_or_default();
    out.extend(
        cx.key_bindings()
            .borrow()
            .bindings()
            .filter(|binding| binding.meta() != Some(USER_META))
            .cloned(),
    );
    out
}

/// The keymap without the user's bindings.
pub fn defaults(cx: &App) -> Vec<Entry> {
    base_bindings(cx).iter().map(entry_of).collect()
}

fn context_of(binding: &KeyBinding) -> Option<String> {
    binding.predicate().map(|predicate| predicate.to_string())
}

fn build(override_: &Override, cx: &App) -> Result<KeyBinding, String> {
    let keystrokes = normalize_keystrokes(&override_.keystrokes)?;
    let predicate = match override_.context.as_deref() {
        Some(context) => Some(
            KeyBindingContextPredicate::parse(context)
                .map_err(|error| error.to_string())?
                .into(),
        ),
        None => None,
    };
    // An unbinding is only a carrier for its keystrokes and context here: it is
    // never bound (see `apply`).
    let (action, input): (Box<dyn Action>, _) = match &override_.action {
        Some(name) => (
            cx.build_action(name, override_.params.clone())
                .map_err(|error| error.to_string())?,
            override_.params.as_ref().map(|params| params.to_string().into()),
        ),
        None => (Box::new(NoAction {}), None),
    };
    KeyBinding::load(
        &keystrokes,
        action,
        predicate,
        false,
        input,
        cx.keyboard_mapper().as_ref(),
    )
    .map(|binding| binding.with_meta(USER_META))
    .map_err(|error| error.to_string())
}

/// Rebuild the keymap from its base bindings and `overrides`, in order: a
/// binding is added, an unbinding removes every earlier binding of the same
/// keystrokes in the same context. Entries that cannot be applied (an action no
/// loaded plugin provides, a bad context) are skipped and reported; they stay
/// in the file.
///
/// Unbinding removes the binding from the keymap rather than adding a
/// `NoAction`: at dispatch a context-less entry outranks contextual ones, so a
/// `NoAction` for a global shortcut would silence the contextual bindings that
/// share its keys (text fields' own Ctrl+A, say).
pub fn apply(overrides: &[Override], cx: &mut App) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut current = base_bindings(cx);
    let mut removed: Vec<KeyBinding> = Vec::new();
    for entry in overrides {
        let binding = match build(entry, cx) {
            Ok(binding) => binding,
            Err(error) => {
                warnings.push(format!(
                    "{} {}: {error}",
                    entry.keystrokes,
                    entry.action.as_deref().unwrap_or("(unbind)")
                ));
                continue;
            }
        };
        if entry.action.is_some() {
            current.push(binding);
        } else {
            let (keystrokes, context) = (keystrokes_of(&binding), context_of(&binding));
            current.retain(|existing| {
                let hit = keystrokes_of(existing) == keystrokes && context_of(existing) == context;
                if hit && existing.meta() != Some(USER_META) {
                    removed.push(existing.clone());
                }
                !hit
            });
        }
    }
    cx.set_global(RemovedDefaults(removed));
    cx.clear_key_bindings();
    cx.bind_keys(current);
    warnings
}

/// An action the editor can bind keys to.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionInfo {
    /// Registry name, e.g. `file_manager::CreateAsset`.
    pub name: String,
    /// `File Manager`.
    pub namespace: String,
    /// `Create Asset`.
    pub title: String,
    pub doc: Option<String>,
    /// Whether it can be built without parameters, so the window can bind it.
    pub bindable: bool,
}

fn words(camel: &str) -> String {
    let mut out = String::new();
    let mut previous: Option<char> = None;
    for ch in camel.chars() {
        let boundary = previous
            .is_some_and(|p| (p.is_lowercase() || p.is_ascii_digit()) && ch.is_uppercase());
        if boundary {
            out.push(' ');
        }
        out.push(ch);
        previous = Some(ch);
    }
    out
}

fn title_case(snake: &str) -> String {
    snake
        .split(['_', ':'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Split a registry name into its namespace and title.
pub fn describe_name(name: &str) -> (String, String) {
    match name.rsplit_once("::") {
        Some((namespace, action)) => (title_case(namespace), words(action)),
        None => ("General".to_owned(), words(name)),
    }
}

/// Every registered action except aliases and the unbind marker, by
/// namespace then title.
pub fn actions(cx: &App) -> Vec<ActionInfo> {
    let aliases = cx.deprecated_actions_to_preferred_actions();
    let docs = cx.action_documentation();
    let mut out: Vec<ActionInfo> = cx
        .all_action_names()
        .iter()
        .filter(|name| !aliases.contains_key(*name))
        .filter(|name| **name != NoAction {}.name())
        .map(|name| {
            let (namespace, title) = describe_name(name);
            ActionInfo {
                name: (*name).to_owned(),
                namespace,
                title,
                doc: docs
                    .get(name)
                    .map(|doc| doc.trim().to_owned())
                    .filter(|doc| !doc.is_empty()),
                bindable: cx.build_action(name, None).is_ok(),
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.namespace, &a.title).cmp(&(&b.namespace, &b.title)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_normalize_to_one_spelling() {
        assert_eq!(normalize_keystrokes("ctrl-k   shift-ctrl-s").unwrap(), "ctrl-k ctrl-shift-s");
        assert!(normalize_keystrokes("").is_err());
    }

    #[test]
    fn contexts_normalize_and_blank_is_none() {
        assert_eq!(normalize_context("  ").unwrap(), None);
        assert_eq!(normalize_context("Editor").unwrap().as_deref(), Some("Editor"));
        assert!(normalize_context("Editor &&").is_err());
    }

    #[test]
    fn names_split_into_namespace_and_title() {
        assert_eq!(
            describe_name("file_manager::CreateAsset"),
            ("File Manager".to_owned(), "Create Asset".to_owned())
        );
        assert_eq!(
            describe_name("ToggleConsole"),
            ("General".to_owned(), "Toggle Console".to_owned())
        );
    }
}
