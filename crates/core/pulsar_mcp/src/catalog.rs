//! The tool list: every registered tool, plus what the host's providers add at
//! run time.
//!
//! Two sources, one list:
//!
//! - **Registrations** ([`McpToolRegistration`], category `mcp.tools`): the
//!   engine's crates and the enabled plugins' roots. They cross plugin library
//!   boundaries through `pulsar_plugins`' registry, so a plugin's tool shows up
//!   with no code here.
//! - **Providers** ([`ToolProvider`]): tools that exist only while something is
//!   running and that are not registrations, like the editor's existing AI tools
//!   (plugin and level editor tools, found through the plugin manager). The host
//!   adds them with [`add_provider`]; the server lives in the host's copy of
//!   this crate, so the provider list needs no library-boundary seam.
//!
//! A name is taken by the first source that has it (registrations, in name order,
//! then providers in the order they were added); a duplicate is logged once and
//! ignored, never an error for the list.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::tool::{McpToolRegistration, ToolInfo, ToolResult, schema_of};

/// A source of tools that is not a registration.
pub trait ToolProvider: Send + Sync {
    /// Names the provider (`ai-tools`): a second provider with the same name
    /// replaces the first.
    fn name(&self) -> &str;
    /// The tools it offers now.
    fn list(&self) -> Vec<ToolInfo>;
    /// Run `tool` with the arguments as JSON text; `None` when the tool is not
    /// this provider's.
    fn call(&self, tool: &str, arguments: &str) -> Option<ToolResult>;
}

fn providers() -> &'static RwLock<Vec<Arc<dyn ToolProvider>>> {
    static PROVIDERS: RwLock<Vec<Arc<dyn ToolProvider>>> = RwLock::new(Vec::new());
    &PROVIDERS
}

/// Add a provider, or replace the one with the same name.
pub fn add_provider(provider: Arc<dyn ToolProvider>) {
    let mut all = providers().write();
    match all.iter().position(|p| p.name() == provider.name()) {
        Some(at) => all[at] = provider,
        None => all.push(provider),
    }
}

/// Take a provider away.
pub fn remove_provider(name: &str) {
    providers().write().retain(|p| p.name() != name);
}

fn source_of(registration: &McpToolRegistration) -> String {
    match pulsar_plugins::owner_of(registration.origin) {
        pulsar_plugins::Owner::Engine(_) => "engine".to_owned(),
        pulsar_plugins::Owner::Plugin(plugin) => plugin.id.clone(),
        pulsar_plugins::Owner::Unknown(origin) => origin.to_owned(),
    }
}

fn warn_once(message: String) {
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if SEEN.lock().insert(message.clone()) {
        tracing::warn!("{message}");
    }
}

/// Every tool now, by name.
pub fn list() -> Vec<ToolInfo> {
    let mut by_name: BTreeMap<String, ToolInfo> = BTreeMap::new();
    let mut registrations: Vec<&McpToolRegistration> =
        pulsar_plugins::registered::<McpToolRegistration>().collect();
    registrations.sort_by_key(|registration| registration.name);
    for registration in registrations {
        let info = ToolInfo {
            name: registration.name.to_owned(),
            title: registration.title.to_owned(),
            description: registration.description.to_owned(),
            input_schema: schema_of(registration.name, registration.input_schema),
            annotations: registration.annotations,
            source: source_of(registration),
        };
        if by_name.contains_key(&info.name) {
            warn_once(format!(
                "tool `{}` is registered twice; the first stays",
                info.name
            ));
            continue;
        }
        by_name.insert(info.name.clone(), info);
    }
    for provider in providers().read().iter() {
        for mut info in provider.list() {
            if by_name.contains_key(&info.name) {
                warn_once(format!(
                    "tool `{}` of provider `{}` is already taken; ignored",
                    info.name,
                    provider.name()
                ));
                continue;
            }
            if info.source.is_empty() {
                info.source = provider.name().to_owned();
            }
            by_name.insert(info.name.clone(), info);
        }
    }
    by_name.into_values().collect()
}

/// Run the tool `name`; `None` when no tool has that name.
pub fn call(name: &str, arguments: &str) -> Option<ToolResult> {
    if let Some(registration) = pulsar_plugins::registered::<McpToolRegistration>()
        .filter(|registration| registration.name == name)
        .min_by_key(|registration| registration.origin)
    {
        return Some(guarded(name, || (registration.call)(arguments)));
    }
    let candidates: Vec<Arc<dyn ToolProvider>> = providers().read().iter().cloned().collect();
    for provider in candidates {
        if let Some(result) = guarded_option(name, || provider.call(name, arguments)) {
            return Some(result);
        }
    }
    None
}

/// A tool that panics is a failed call, not a dead worker.
fn guarded(name: &str, run: impl FnOnce() -> ToolResult) -> ToolResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .unwrap_or_else(|_| ToolResult::error(format!("the tool `{name}` panicked; see the engine log")))
}

fn guarded_option(name: &str, run: impl FnOnce() -> Option<ToolResult>) -> Option<ToolResult> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| {
        Some(ToolResult::error(format!(
            "the tool `{name}` panicked; see the engine log"
        )))
    })
}
