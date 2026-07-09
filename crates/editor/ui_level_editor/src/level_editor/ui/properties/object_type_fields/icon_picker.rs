//! Object-level icon-asset picker.
//!
//! The icon is stored as a plain JSON string in the object's prop map under
//! [`OBJECT_ICON_PROP_KEY`].  A [`MeshAssetPicker`] entity is created lazily
//! on first render and kept alive in [`ObjectTypeFieldsSection::icon_asset_picker`].

use engine_state;
use gpui::{prelude::*, *};
use serde_json::Value;
use ui::button::ButtonVariants as _;
use ui::popover::Popover;
use ui::{h_flex, ActiveTheme, Sizable};
use ui_common::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

use super::ObjectTypeFieldsSection;

const OBJECT_ICON_PROP_KEY: &str = "icon_asset";

impl ObjectTypeFieldsSection {
    /// Reads the icon asset path from the object's prop map.
    pub(super) fn read_object_icon_path(&self) -> String {
        self.scene_db
            .get_object(&self.object_id)
            .and_then(|obj| obj.props.get(OBJECT_ICON_PROP_KEY).cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Persists an icon asset path into the object's prop map.
    pub(super) fn write_object_icon_path(&self, path: String) {
        let Some(mut obj) = self.scene_db.get_object(&self.object_id) else {
            return;
        };

        if path.is_empty() {
            obj.props.remove(OBJECT_ICON_PROP_KEY);
        } else {
            obj.props
                .insert(OBJECT_ICON_PROP_KEY.to_string(), Value::String(path));
        }

        let _ = self.scene_db.update_object(obj);
    }

    /// Creates the [`MeshAssetPicker`] entity the first time it is needed.
    ///
    /// Subsequent calls are no-ops — the picker is kept alive for the lifetime
    /// of this inspector so thumbnails and selection state are preserved.
    pub(super) fn ensure_object_icon_picker(
        &mut self,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.icon_asset_picker.is_some() {
            return;
        }

        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let queries = vec![
            AssetQuery::extension("png"),
            AssetQuery::extension("jpg"),
            AssetQuery::extension("jpeg"),
            AssetQuery::extension("webp"),
        ];

        let picker = cx.new(|cx| {
            MeshAssetPicker::new(
                current.to_string(),
                vec![],
                project_root,
                queries,
                window,
                cx,
            )
        });

        cx.subscribe(&picker, move |this, picker, _event: &AssetPickedEvent, cx| {
            let selected = picker.read(cx).selected_path().to_string();
            this.write_object_icon_path(selected);
            cx.notify();
        })
        .detach();

        self.icon_asset_picker = Some(picker);
    }

    /// Renders the "Object Icon" picker row, or an empty placeholder if the
    /// picker entity has not been initialised yet.
    pub(super) fn render_icon_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let object_icon_path = self.read_object_icon_path();
        self.ensure_object_icon_picker(&object_icon_path, window, cx);

        let Some(picker) = self.icon_asset_picker.clone() else {
            return div().into_any_element();
        };

        let display = if object_icon_path.is_empty() {
            "Select icon asset...".to_string()
        } else {
            std::path::Path::new(&object_icon_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&object_icon_path)
                .to_string()
        };

        let thumb = picker.read(cx).thumbnail_for_path(&object_icon_path);

        let pop = Popover::<MeshAssetPicker>::new(format!("object-icon-picker-{}", self.object_id))
            .anchor(Corner::BottomRight)
            .trigger(
                ui::button::Button::new(format!("object-icon-btn-{}", self.object_id))
                    .label(display)
                    .small()
                    .ghost()
                    .dropdown_caret(true),
            )
            .content(move |_window, _cx| picker.clone())
            .into_any_element();

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .p_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child("Object Icon"),
            )
            .child(h_flex().items_center().gap_2().child(pop).map(|el| {
                match thumb {
                    Some(render_img) => el.child(
                        div()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded(px(4.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(cx.theme().border)
                            .flex_shrink_0()
                            .child(
                                gpui::img(gpui::ImageSource::Render(render_img))
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .object_fit(gpui::ObjectFit::Cover),
                            ),
                    ),
                    None => el,
                }
            }))
            .into_any_element()
    }
}
