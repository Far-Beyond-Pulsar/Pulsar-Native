use dashmap::DashMap;
use gpui::AnyWindowHandle;
use std::sync::Arc;
use ui_types_common::window_types::{WindowId, WindowRequest};

#[derive(Clone)]
pub struct WindowState {
    windows: Arc<DashMap<WindowId, WindowInfo>>,
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub window_id: WindowId,
    pub window_type: WindowRequest,
    pub parent_window: Option<WindowId>,
    pub created_at: std::time::Instant,
    pub handle: AnyWindowHandle,
}

impl WindowState {
    pub fn new() -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
        }
    }

    pub fn register_window(
        &self,
        window_id: WindowId,
        window_type: WindowRequest,
        parent: Option<WindowId>,
        handle: AnyWindowHandle,
    ) {
        self.windows.insert(
            window_id,
            WindowInfo {
                window_id,
                window_type,
                parent_window: parent,
                created_at: std::time::Instant::now(),
                handle,
            },
        );
    }

    pub fn unregister_window(&self, window_id: WindowId) -> Option<WindowInfo> {
        self.windows.remove(&window_id).map(|(_, info)| info)
    }

    pub fn window_exists(&self, window_id: WindowId) -> bool {
        self.windows.contains_key(&window_id)
    }

    pub fn get_window(&self, window_id: WindowId) -> Option<WindowInfo> {
        self.windows
            .get(&window_id)
            .map(|entry| entry.value().clone())
    }

    /// Find a window by its request, matching `Custom` variants by `type_name`.
    pub fn find_by_request(&self, request: &WindowRequest) -> Option<WindowInfo> {
        let type_name = match request {
            WindowRequest::Custom { type_name } => type_name,
            other => {
                return self
                    .windows
                    .iter()
                    .find(|entry| {
                        std::mem::discriminant(&entry.window_type)
                            == std::mem::discriminant(other)
                    })
                    .map(|entry| entry.value().clone());
            }
        };
        self.windows.iter().find(|entry| {
            matches!(&entry.window_type, WindowRequest::Custom { type_name: n } if n == type_name)
        }).map(|entry| entry.value().clone())
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub fn windows_by_type(&self, window_type: &WindowRequest) -> Vec<WindowInfo> {
        self.windows
            .iter()
            .filter(|entry| {
                std::mem::discriminant(&entry.window_type) == std::mem::discriminant(window_type)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub fn all_windows(&self) -> Vec<WindowInfo> {
        self.windows
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub fn child_windows(&self, parent_id: WindowId) -> Vec<WindowInfo> {
        self.windows
            .iter()
            .filter(|entry| entry.parent_window == Some(parent_id))
            .map(|entry| entry.value().clone())
            .collect()
    }
}

impl Default for WindowState {
    fn default() -> Self {
        Self::new()
    }
}
