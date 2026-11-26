//! Another server a person may choose for a language.
//!
//! A plugin adds one from its root's `build`
//! (`p.add(ServerAlternative { .. })`), for any language server, as many as it
//! likes. A server with alternatives gets a **Server** dropdown in its settings
//! group, and launch settings for each alternative; the host runs the chosen
//! spec. The host names none of them.
//!
//! A language's server is a slot, named by its spec's `id`: the settings group,
//! the running server's identity (`ServerId`) and its diagnostics are the
//! slot's, so an alternative's spec carries the same `id`. Everything else
//! (name, executable, roots, readiness, requests, install) is the alternative's
//! own, because choosing another server replaces the running one outright.

use crate::spec::LanguageServerSpec;

/// A server that can run in place of a language's own, chosen in that server's
/// settings group.
pub struct ServerAlternative {
    /// The package that submitted it (`pulsar_plugins::origin!()`): the same
    /// as the language whose server it can replace.
    pub origin: &'static str,
    /// The stable value the setting saves, and the prefix of its launch
    /// settings' keys.
    pub name: &'static str,
    /// The dropdown entry.
    pub label: &'static str,
    /// What choosing it means, shown with the dropdown.
    pub description: &'static str,
    /// What runs while it is chosen. Its `id` is the slot's: the `id` of the
    /// server it can replace (one with another `id` is never offered).
    pub spec: &'static LanguageServerSpec,
}

impl pulsar_plugins::Contribution for ServerAlternative {
    const CATEGORY: &'static str = "language server alternative";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        format!("{}.{}", self.spec.id, self.name)
    }

    fn label(&self) -> String {
        self.spec.display_name.to_string()
    }
}

pulsar_plugins::registration!(ServerAlternative);

/// The alternatives `origin` registered for the slot `id`, whatever the
/// plugin's state.
pub fn alternatives_of<'a>(
    origin: &'a str,
    id: &'a str,
) -> impl Iterator<Item = &'static ServerAlternative> + 'a {
    pulsar_plugins::all_registered::<ServerAlternative>()
        .filter(move |alternative| alternative.origin == origin && alternative.spec.id == id)
}
