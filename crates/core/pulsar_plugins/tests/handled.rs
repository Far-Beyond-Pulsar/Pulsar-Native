//! Startup work runs once per registration, also for one that arrives after
//! the first run (a plugin library imported late).

use pulsar_plugins::{Contribution, Handled, PluginBuilder};

struct Probe {
    origin: &'static str,
    name: &'static str,
}

impl Contribution for Probe {
    const CATEGORY: &'static str = "test.handled";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(Probe);

pulsar_plugins::inventory::submit! {
    Probe { origin: "engine:handled_test", name: "linked" }
}

#[test]
fn a_registration_arriving_after_the_first_run_is_handed_out_once() {
    static HANDLED: Handled = Handled::new();
    let names = |batch: Vec<&'static Probe>| -> Vec<&'static str> {
        let mut names: Vec<_> = batch.into_iter().map(|probe| probe.name).collect();
        names.sort_unstable();
        names
    };

    // The host's startup: only the linked registration exists.
    assert_eq!(names(HANDLED.take_new_ungated::<Probe>()), ["linked"]);
    assert!(HANDLED.take_new_ungated::<Probe>().is_empty());

    // A library is imported afterwards: its registration gets its turn,
    // the earlier one does not run again.
    let mut library = PluginBuilder::new("late_plugin");
    library.add(Probe {
        origin: "late_plugin",
        name: "late",
    });
    pulsar_plugins::import_library(library);
    assert_eq!(names(HANDLED.take_new_ungated::<Probe>()), ["late"]);
    assert!(HANDLED.take_new_ungated::<Probe>().is_empty());

    // Another library carrying the same identity again (a second copy of
    // a shared crate) is not a new registration.
    let mut again = PluginBuilder::new("late_plugin");
    again.add(Probe {
        origin: "late_plugin",
        name: "late",
    });
    pulsar_plugins::import_library(again);
    assert!(HANDLED.take_new_ungated::<Probe>().is_empty());
}
