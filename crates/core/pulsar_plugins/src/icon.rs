//! A plugin's icon: an image the editor shows beside the plugin (the
//! Extensions list and details). A plugin registers it from its root
//! `build`, with the image compiled in:
//!
//! ```ignore
//! p.add(pulsar_plugins::PluginIcon {
//!     origin: p.origin(),
//!     format: pulsar_plugins::IconFormat::Png,
//!     bytes: include_bytes!("../icon.png"),
//! });
//! ```
//!
//! A square image reads best (it is shown at 44 px and 64 px); SVG scales
//! cleanly. A plugin without one gets a generic icon.

use crate::{Contribution, Owner};

/// The encoding of a [`PluginIcon`]'s bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconFormat {
    Png,
    Jpeg,
    Webp,
    Svg,
}

/// A plugin's icon image.
pub struct PluginIcon {
    /// The plugin's root crate (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub format: IconFormat,
    /// The encoded image (`include_bytes!`).
    pub bytes: &'static [u8],
}

impl Contribution for PluginIcon {
    const CATEGORY: &'static str = "plugin.icon";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        "icon".to_string()
    }
}

crate::registration!(PluginIcon);

/// The icon plugin `plugin_id` registered, whether or not the plugin is
/// enabled (a disabled plugin is still listed with its icon).
pub fn icon_of(plugin_id: &str) -> Option<&'static PluginIcon> {
    crate::all_registered::<PluginIcon>().find(|icon| {
        matches!(crate::owner_of(icon.origin), Owner::Plugin(plugin) if plugin.id == plugin_id)
    })
}
