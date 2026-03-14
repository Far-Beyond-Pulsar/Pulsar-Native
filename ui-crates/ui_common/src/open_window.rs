//! open_pulsar_window — open any PulsarWindow directly via cx.open_window.
//!
//! Uses `cx.open_window` instead of routing through the `WindowManager` global,
//! which avoids panics when the global is not yet registered and keeps window
//! opening simple and reliable.

use gpui::{App, AppContext as _};
use ui::Root;
use window_manager::PulsarWindow;

/// Open any [`PulsarWindow`] by calling `cx.open_window` directly.
///
/// The window is wrapped in `Root` for theming. This does **not** require the
/// `WindowManager` global to be registered.
///
/// # Example
/// ```ignore
/// open_pulsar_window::<ProblemsWindow>(drawer.clone(), cx);
/// open_pulsar_window::<SettingsWindow>((), cx);
/// ```
pub fn open_pulsar_window<W: PulsarWindow>(params: W::Params, cx: &mut App) {
    let options = W::window_options(&params);
    let _ = cx.open_window(options, move |window, cx| {
        let entity = W::build(params, window, cx);
        cx.new(|cx| Root::new(entity.into(), window, cx))
    });
}
