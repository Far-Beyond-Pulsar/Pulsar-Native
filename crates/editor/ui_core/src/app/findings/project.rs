//! Project health the editor can see without a renderer: the open project's
//! plugin problems (a requirement nothing provides, a provider that is off,
//! two providers of one contract) and the plugin catalog's (a registration no
//! plugin owns, a duplicate id, an undeclared read).

use pulsar_findings::{Action, Category, Finding, Severity};

pub(super) const SOURCE: &str = "Plugins";

fn extensions() -> Action {
    Action::OpenWindow {
        name: "PluginManagerWindow",
    }
}

pub(super) fn collect() -> Vec<Finding> {
    let mut out = Vec::new();
    if let Some(resolution) = pulsar_plugins::active_resolution() {
        for problem in &resolution.problems {
            let text = problem.to_string();
            out.push(
                Finding::new(format!("plugin:{text}"), Category::Project, Severity::Error, text)
                    .detail(
                        "The project's plugin list cannot be honoured as written: the plugin this names is off until it is fixed. Change the [plugins] table in Pulsar.toml.",
                    )
                    .subject("Plugins")
                    .action("Open Extensions", extensions()),
            );
        }
    }
    if let Some(manager) = plugin_manager::global() {
        let catalog = manager.read().catalog();
        for problem in &catalog.problems {
            let text = problem.to_string();
            out.push(
                Finding::new(format!("catalog:{text}"), Category::Project, Severity::Warning, text)
                    .detail("A registration does not hold together with what its plugin declares.")
                    .subject("Plugins")
                    .action("Open Extensions", extensions()),
            );
        }
    }
    out
}
