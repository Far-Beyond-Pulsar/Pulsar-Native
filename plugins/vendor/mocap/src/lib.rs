//! Motion capture (`pulsar.mocap`): the plugin's root crate, the one place it
//! registers what it contributes, through its `build`.
//!
//! Contributes:
//!
//! - the `.mocap` file type and its editor (`MocapBuiltinProvider`): a live
//!   capture session (camera, VMC, synthetic) and take review.
//!
//! The crates: `mocap_format` (the take file), `mocap_capture` (sources,
//! processing, recorder), `mocap_editor` (the dock workspace). Design:
//! `.agents/MOCAP.md`.

// The descriptor and the build: linking the root installs the plugin.
pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// Everything the plugin registers.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // ── Asset editors ───────────────────────────────────────────────────────
    p.add(plugin_manager::PluginEditorProvider {
        origin: p.origin(),
        id: mocap_editor::PROVIDER_ID,
        create: mocap_editor::MocapBuiltinProvider::create,
    });
}

// Hosts and tests reach the plugin's crates through its root.
pub use mocap_capture;
pub use mocap_editor;
pub use mocap_format;

#[cfg(test)]
mod tests {
    use mocap_format::EXTENSION;

    #[test]
    fn the_editor_provider_offers_the_mocap_file_type() {
        let provider = mocap_editor::MocapBuiltinProvider::create();
        let types = provider.file_types();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].extension, EXTENSION);
        assert_eq!(provider.editors()[0].supported_file_types.len(), 1);
    }
}
