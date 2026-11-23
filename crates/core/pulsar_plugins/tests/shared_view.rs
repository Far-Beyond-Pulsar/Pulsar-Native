//! A plugin library's copy of this crate answers from the host's view.
//!
//! A library links its own copy, so code of one plugin that asks what is
//! registered saw only that library's registrations (another plugin's
//! asset loaders, say, were invisible), and its enabled set was never the
//! project's. This is its own test binary: attaching is process-wide.

use pulsar_plugins::shared::{attach, HostPlugins, AttachError};
use pulsar_plugins::{Contribution, PluginDescriptor};

struct Probe {
    origin: &'static str,
    name: &'static str,
}

impl Contribution for Probe {
    const CATEGORY: &'static str = "test.shared_view";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(Probe);

/// What the "host" holds: one registration another plugin's build added.
static OTHER_PLUGINS: Probe = Probe {
    origin: "other_plugin",
    name: "from-the-host-store",
};

fn host_built(category: &str, fingerprint: u64, sink: &mut dyn FnMut(*const ())) {
    if category == Probe::CATEGORY && fingerprint == pulsar_plugins::library::fingerprint::<Probe>()
    {
        sink((&OTHER_PLUGINS as *const Probe).cast());
    }
}

fn host_installed() -> &'static [PluginDescriptor] {
    static HOST: std::sync::OnceLock<Vec<PluginDescriptor>> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        vec![PluginDescriptor::parse(
            "id = \"host.only\"\nname = \"Host only\"\nroot = \"host_only\"\n",
        )
        .unwrap()]
    })
}

fn host() -> &'static HostPlugins {
    Box::leak(Box::new(HostPlugins {
        abi: pulsar_plugins::shared::ABI_VERSION,
        built: host_built,
        origin_enabled: |origin| origin != "switched_off",
        is_enabled: |plugin| plugin == "host.only",
        generation: || 41,
        installed: host_installed,
    }))
}

#[test]
fn an_attached_copy_reads_the_hosts_registrations_installed_and_enabled_sets() {
    // Alone, the copy knows only itself.
    assert!(pulsar_plugins::all_registered::<Probe>().next().is_none());
    assert!(pulsar_plugins::installed().is_empty());
    assert!(pulsar_plugins::origin_enabled("switched_off"));
    assert_ne!(pulsar_plugins::generation(), 41);

    let host = host();
    // SAFETY: `host` is leaked, so it lives for the process.
    unsafe { attach(host) }.unwrap();
    // The same table again is fine; another one is refused.
    // SAFETY: as above.
    unsafe { attach(host) }.unwrap();
    // SAFETY: as above.
    assert_eq!(
        unsafe { attach(self::host()) },
        Err(AttachError::AlreadyAttached)
    );

    let seen: Vec<&str> = pulsar_plugins::all_registered::<Probe>()
        .map(|probe| probe.name)
        .collect();
    assert_eq!(seen, ["from-the-host-store"]);
    assert_eq!(pulsar_plugins::installed()[0].id, "host.only");
    assert!(!pulsar_plugins::origin_enabled("switched_off"));
    assert!(pulsar_plugins::is_enabled("host.only"));
    assert_eq!(pulsar_plugins::generation(), 41);
}
