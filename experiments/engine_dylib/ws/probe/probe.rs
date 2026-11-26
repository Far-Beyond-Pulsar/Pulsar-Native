//! Shared probe: what a copy of the engine crates in this module looks like.
//! Included (via #[path]) by the host and by the wrapper plugin libraries, so it is
//! compiled once per crate that includes it. Everything is compared host vs plugin.
use std::any::TypeId;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Probe {
    pub gpui_window: TypeId,
    pub gpui_app: TypeId,
    pub ui_theme: TypeId,
    pub language_servers: TypeId,
    pub entity_language_servers: TypeId,
    pub gpui_runtime_addr: usize,
    pub gpui_module_slot: u32,
    pub engine_settings_addr: usize,
    pub helio_task_host: usize,
    pub helio_handle: [usize; 4],
    pub plugins_descriptor_present: bool,
}

pub fn probe_here() -> Probe {
    let handle = helio_core::runtime::host_handle();
    // HostHandle { layout: u64, shared: &Shared, tasks: &dyn TaskHost } = 32 bytes
    let words: [usize; 4] = unsafe { std::mem::transmute(handle) };
    Probe {
        gpui_window: TypeId::of::<gpui::Window>(),
        gpui_app: TypeId::of::<gpui::App>(),
        ui_theme: TypeId::of::<ui::theme::Theme>(),
        language_servers: TypeId::of::<pulsar_lsp::LanguageServers>(),
        entity_language_servers: TypeId::of::<gpui::Entity<pulsar_lsp::LanguageServers>>(),
        gpui_runtime_addr: gpui::shared_runtime::runtime_address(),
        gpui_module_slot: gpui::shared_runtime::module_slot(),
        engine_settings_addr: engine_state::settings::global_config() as *const _ as usize,
        helio_task_host: helio_tasks::host() as *const _ as *const () as usize,
        helio_handle: words,
        plugins_descriptor_present: pulsar_plugins::library::linked_descriptor().is_some(),
    }
}

#[no_mangle]
pub extern "C" fn pulsar_spike_probe(out: *mut Probe) {
    unsafe { out.write(probe_here()) }
}
