//! Work to do before a window is closed from its own title bar.
//!
//! The title bar's close button removes the window outright
//! (`Window::remove_window`), which does not go through
//! `Window::on_window_should_close`: that callback only runs for close requests
//! the platform makes. A window that must save state while everything it owns is
//! still alive (the editor window flushing its layout, pop-outs included)
//! registers a hook here, and every title-bar close button runs the hooks first.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Global, Window, WindowId};

type Hook = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Default)]
struct CloseHooks(HashMap<WindowId, Vec<Hook>>);

impl Global for CloseHooks {}

/// Run `hook` when `window` is closed from its title bar, before it goes away.
pub fn on_before_close(
    window: &Window,
    hook: impl Fn(&mut Window, &mut App) + 'static,
    cx: &mut App,
) {
    if !cx.has_global::<CloseHooks>() {
        cx.set_global(CloseHooks::default());
    }
    cx.global_mut::<CloseHooks>()
        .0
        .entry(window.window_handle().window_id())
        .or_default()
        .push(Rc::new(hook));
}

/// Close `window` from its title bar: run its hooks, then remove it.
pub fn close_window(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let hooks = cx
        .try_global::<CloseHooks>()
        .and_then(|hooks| hooks.0.get(&id).cloned())
        .unwrap_or_default();
    for hook in hooks {
        hook(window, cx);
    }
    if cx.has_global::<CloseHooks>() {
        cx.global_mut::<CloseHooks>().0.remove(&id);
    }
    window.remove_window();
}
