//! Wraps editor panels so they carry their file's icon and path.
//!
//! [`FileTypeDecoratedPanelView`] forwards every [`PanelView`] call to the
//! plugin's panel and overrides only the tab icon and file path;
//! [`PluginManager::decorate_editor_panel_for_path`] is the entry point used
//! by editor creation (see `editors.rs`).

use crate::PluginManager;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::PanelView;

struct FileTypeDecoratedPanelView {
    inner: Arc<dyn PanelView>,
    file_path: PathBuf,
    icon: Option<ui::IconName>,
}

impl PanelView for FileTypeDecoratedPanelView {
    fn panel_name(&self, cx: &gpui::App) -> &'static str {
        self.inner.panel_name(cx)
    }

    fn panel_id(&self, cx: &gpui::App) -> gpui::EntityId {
        self.inner.panel_id(cx)
    }

    fn tab_name(&self, cx: &gpui::App) -> Option<gpui::SharedString> {
        self.inner.tab_name(cx)
    }

    fn tab_icon(&self, cx: &gpui::App) -> Option<ui::IconName> {
        self.inner.tab_icon(cx).or_else(|| self.icon.clone())
    }

    fn tab_unsaved(&self, cx: &gpui::App) -> bool {
        self.inner.tab_unsaved(cx)
    }

    fn panel_file_path(&self, cx: &gpui::App) -> Option<PathBuf> {
        self.inner.panel_file_path(cx).or_else(|| {
            if self.file_path.as_os_str().is_empty() {
                None
            } else {
                Some(self.file_path.clone())
            }
        })
    }

    fn title(&self, window: &gpui::Window, cx: &gpui::App) -> gpui::AnyElement {
        self.inner.title(window, cx)
    }

    fn title_suffix(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<gpui::AnyElement> {
        self.inner.title_suffix(window, cx)
    }

    fn title_style(&self, cx: &gpui::App) -> Option<ui::dock::TitleStyle> {
        self.inner.title_style(cx)
    }

    fn closable(&self, cx: &gpui::App) -> bool {
        self.inner.closable(cx)
    }

    fn zoomable(&self, cx: &gpui::App) -> Option<ui::dock::PanelControl> {
        self.inner.zoomable(cx)
    }

    fn visible(&self, cx: &gpui::App) -> bool {
        self.inner.visible(cx)
    }

    fn set_active(&self, active: bool, window: &mut gpui::Window, cx: &mut gpui::App) {
        self.inner.set_active(active, window, cx)
    }

    fn set_zoomed(&self, zoomed: bool, window: &mut gpui::Window, cx: &mut gpui::App) {
        self.inner.set_zoomed(zoomed, window, cx)
    }

    fn popup_menu(
        &self,
        menu: ui::popup_menu::PopupMenu,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> ui::popup_menu::PopupMenu {
        self.inner.popup_menu(menu, window, cx)
    }

    fn toolbar_buttons(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<Vec<ui::button::Button>> {
        self.inner.toolbar_buttons(window, cx)
    }

    fn view(&self) -> gpui::AnyView {
        self.inner.view()
    }

    fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.inner.focus_handle(cx)
    }

    fn dump(&self, cx: &gpui::App) -> ui::dock::PanelState {
        self.inner.dump(cx)
    }

    fn inner_padding(&self, cx: &gpui::App) -> bool {
        self.inner.inner_padding(cx)
    }

    fn discord_icon_key(&self, cx: &gpui::App) -> &'static str {
        self.inner.discord_icon_key(cx)
    }

    fn life(&self, cx: &gpui::App) -> ui::dock::TabLife {
        self.inner.life(cx)
    }

    fn try_sleep(&self, cx: &mut gpui::App) -> bool {
        self.inner.try_sleep(cx)
    }
}

impl PluginManager {
    /// Give `panel` the icon of the file type registered for `file_path`.
    pub(crate) fn decorate_editor_panel_for_path(
        &self,
        panel: Arc<dyn PanelView>,
        file_path: &Path,
    ) -> Arc<dyn PanelView> {
        let icon = self
            .get_file_type_for_path(file_path)
            .map(|ft| ft.icon.clone());

        if icon.is_none() && file_path.as_os_str().is_empty() {
            panel
        } else {
            Arc::new(FileTypeDecoratedPanelView {
                inner: panel,
                file_path: file_path.to_path_buf(),
                icon,
            })
        }
    }
}
