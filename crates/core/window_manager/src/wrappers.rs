use gpui::{AnyView, App, Window};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Kinds of content wrappers that a [`WindowProfile`](crate::configs::WindowProfile) can request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowContentWrapper {
    None,
    Editor,
}

type WindowWrapperFn =
    Arc<dyn Fn(AnyView, &mut Window, &mut App) -> AnyView + Send + Sync + 'static>;

fn wrapper_registry() -> &'static RwLock<HashMap<WindowContentWrapper, WindowWrapperFn>> {
    static REGISTRY: OnceLock<RwLock<HashMap<WindowContentWrapper, WindowWrapperFn>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register (or replace) the wrapper function for a wrapper kind.
pub fn register_window_wrapper(
    wrapper_kind: WindowContentWrapper,
    wrapper: impl Fn(AnyView, &mut Window, &mut App) -> AnyView + Send + Sync + 'static,
) {
    let mut registry = wrapper_registry()
        .write()
        .expect("window wrapper registry lock poisoned");
    registry.insert(wrapper_kind, Arc::new(wrapper));
}

pub fn apply_window_wrapper(
    wrapper_kind: WindowContentWrapper,
    content: AnyView,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    if wrapper_kind == WindowContentWrapper::None {
        return content;
    }

    let wrapper = wrapper_registry()
        .read()
        .expect("window wrapper registry lock poisoned")
        .get(&wrapper_kind)
        .cloned();

    if let Some(wrapper) = wrapper {
        wrapper(content, window, cx)
    } else {
        content
    }
}
