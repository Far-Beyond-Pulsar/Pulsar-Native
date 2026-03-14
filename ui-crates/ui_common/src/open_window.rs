//! open_pulsar_window — open any PulsarWindow through the WindowManager.

use gpui::{App, AppContext as _, UpdateGlobal};
use ui::Root;
use window_manager::{PulsarWindow, WindowManager, WindowRequest};

/// Open any [`PulsarWindow`] through the global [`WindowManager`].
///
/// Telemetry, hooks, and engine-state registration all run automatically.
/// The view is wrapped in `Root` for theming.
///
/// # Example
/// ```ignore
/// open_pulsar_window::<ProblemsWindow>(drawer.clone(), cx);
/// open_pulsar_window::<SettingsWindow>((), cx);
/// ```
pub fn open_pulsar_window<W: PulsarWindow>(params: W::Params, cx: &mut App) {
    let options = W::window_options(&params);
    let type_name = W::window_name();
    let _ = WindowManager::update_global(cx, |wm, cx| {
        wm.create_window(
            WindowRequest::Custom { type_name },
            options,
            move |window, cx: &mut App| {
                let entity = W::build(params, window, cx);
                cx.new(|cx| Root::new(entity.into(), window, cx))
            },
            cx,
        )
    });
}
