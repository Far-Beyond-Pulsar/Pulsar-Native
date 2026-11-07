//! What the window lists: one entry per action with its live bindings, and the
//! search and filters over them. No GPUI here either.

use super::overrides::{self, Binding, Entry, Override};
use super::registry::ActionInfo;

#[derive(Clone, Debug, PartialEq)]
pub struct BindingView {
    pub binding: Binding,
    /// Added by the user's file rather than shipped.
    pub user: bool,
    /// Another action's later binding of the same keys and context takes it.
    pub shadowed_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionEntry {
    pub info: ActionInfo,
    pub bindings: Vec<BindingView>,
    /// The user changed something about this action.
    pub customized: bool,
    /// One of its bindings never fires because another action took its keys.
    pub conflicted: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Customized,
    Unbound,
    Conflicts,
}

impl Filter {
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Customized,
        Filter::Unbound,
        Filter::Conflicts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Customized => "Changed",
            Filter::Unbound => "Unbound",
            Filter::Conflicts => "Conflicts",
        }
    }
}

/// Entries for `actions`, from the defaults and the user's overrides.
pub fn build_entries(
    actions: &[ActionInfo],
    defaults: &[Entry],
    user: &[Override],
) -> Vec<ActionEntry> {
    let live = overrides::effective_with(defaults, user);
    let shadows = overrides::shadowed_by(&live);
    actions
        .iter()
        .map(|info| {
            let bindings: Vec<BindingView> = live
                .iter()
                .enumerate()
                .filter(|(_, binding)| binding.action == info.name)
                .map(|(ix, binding)| BindingView {
                    user: user.iter().any(|o| {
                        o.action.as_deref() == Some(binding.action.as_str())
                            && o.keystrokes == binding.keystrokes
                            && o.context == binding.context
                    }),
                    shadowed_by: shadows[ix].map(|later| live[later].action.clone()),
                    binding: binding.clone(),
                })
                .collect();
            ActionEntry {
                customized: overrides::is_customized(defaults, user, &info.name),
                conflicted: bindings.iter().any(|b| b.shadowed_by.is_some()),
                bindings,
                info: info.clone(),
            }
        })
        .collect()
}

/// Whether `entry` passes the filter and the search. Every word of the query
/// must appear somewhere in the action's names, description or keystrokes.
pub fn matches(entry: &ActionEntry, query: &str, filter: Filter) -> bool {
    let filter_ok = match filter {
        Filter::All => true,
        Filter::Customized => entry.customized,
        Filter::Unbound => entry.bindings.is_empty(),
        Filter::Conflicts => entry.conflicted,
    };
    if !filter_ok {
        return false;
    }
    let mut haystack = format!(
        "{} {} {} {}",
        entry.info.title,
        entry.info.namespace,
        entry.info.name,
        entry.info.doc.as_deref().unwrap_or("")
    );
    for view in &entry.bindings {
        haystack.push(' ');
        haystack.push_str(&view.binding.keystrokes);
        haystack.push(' ');
        haystack.push_str(&view.binding.keystrokes.replace('-', " "));
    }
    let haystack = haystack.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .map(|word| word.replace('+', " "))
        .all(|word| word.split_whitespace().all(|part| haystack.contains(part)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str) -> ActionInfo {
        let (namespace, title) = super::super::registry::describe_name(name);
        ActionInfo {
            name: name.to_owned(),
            namespace,
            title,
            doc: None,
            bindable: true,
        }
    }

    fn entries(user: &[Override]) -> Vec<ActionEntry> {
        let defaults = [
            Entry::Bind(Binding::new("app::ToggleFileManager", "ctrl-space", None)),
            Entry::Bind(Binding::new("app::GoBack", "alt-left", None)),
        ];
        build_entries(
            &[
                info("app::ToggleFileManager"),
                info("app::GoBack"),
                info("app::Unused"),
            ],
            &defaults,
            user,
        )
    }

    #[test]
    fn entries_list_live_bindings_and_flag_customised_actions() {
        let user = vec![Override {
            keystrokes: "ctrl-space".into(),
            action: None,
            context: None,
            params: None,
        }];
        let list = entries(&user);
        assert!(list[0].bindings.is_empty());
        assert!(list[0].customized);
        assert_eq!(list[1].bindings.len(), 1);
        assert!(!list[1].customized);
        assert!(!list[1].bindings[0].user);
        assert!(list[2].bindings.is_empty());
        assert!(!list[2].customized);
    }

    #[test]
    fn a_taken_binding_is_flagged_on_the_action_that_lost_it() {
        let user = vec![Override {
            keystrokes: "alt-left".into(),
            action: Some("app::Unused".into()),
            context: None,
            params: None,
        }];
        let list = entries(&user);
        assert!(list[1].conflicted);
        assert_eq!(
            list[1].bindings[0].shadowed_by.as_deref(),
            Some("app::Unused")
        );
        assert!(list[2].bindings[0].user);
        assert!(!list[2].conflicted);
    }

    #[test]
    fn filters_select_by_state() {
        let list = entries(&[]);
        let names = |filter| -> Vec<&str> {
            list.iter()
                .filter(|e| matches(e, "", filter))
                .map(|e| e.info.title.as_str())
                .collect()
        };
        assert_eq!(names(Filter::All).len(), 3);
        assert_eq!(names(Filter::Unbound), vec!["Unused"]);
        assert!(names(Filter::Customized).is_empty());
        assert!(names(Filter::Conflicts).is_empty());
    }

    #[test]
    fn search_matches_titles_namespaces_and_keys() {
        let list = entries(&[]);
        let hits = |query: &str| -> Vec<&str> {
            list.iter()
                .filter(|e| matches(e, query, Filter::All))
                .map(|e| e.info.title.as_str())
                .collect()
        };
        assert_eq!(hits("toggle file"), vec!["Toggle File Manager"]);
        assert_eq!(hits("APP").len(), 3);
        assert_eq!(hits("ctrl+space"), vec!["Toggle File Manager"]);
        assert_eq!(hits("alt left"), vec!["Go Back"]);
        assert!(hits("nothing like this").is_empty());
    }
}
