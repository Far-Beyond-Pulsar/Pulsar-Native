//! Panel Window - Displays a single panel in a popup window

use gpui::*;
use std::sync::Arc;
use ui::{
    v_flex, ActiveTheme as _, TitleBar, dock::TabPanel,
};

pub struct PanelWindow {
    panel: Arc<dyn ui::dock::PanelView>,
    center_tabs: Entity<TabPanel>,
    parent_window_handle: AnyWindowHandle,
}

impl PanelWindow {
    pub fn new(
        panel: Arc<dyn ui::dock::PanelView>,
        center_tabs: Entity<TabPanel>,
        parent_window_handle: AnyWindowHandle,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { 
            panel,
            center_tabs,
            parent_window_handle,
        }
    }
}

impl Render for PanelWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let panel = self.panel.clone();
        let center_tabs = self.center_tabs.clone();
        let parent_window_handle = self.parent_window_handle;

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new()
                    .on_close_window(move |_, window, cx| {
                        tracing::trace!("[POPOUT] Close button clicked, restoring panel to main window");
                        
                        // Restore the panel to the main window
                        let panel_to_restore = panel.clone();
                        let _ = cx.update_window(parent_window_handle, |_root, window, cx| {
                            cx.update_entity(&center_tabs, |tab_panel, cx| {
                                tracing::trace!("[POPOUT] Adding panel back to center tabs");
                                tab_panel.add_panel(panel_to_restore.clone(), window, cx);
                            });
                        });
                        
                        // Close this window
                        window.remove_window();
                    })
                    .child(self.panel.title(window, cx))
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.panel.view())
            )
    }
}
