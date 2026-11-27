//! The pickers of `helio_component`'s asset-path types: that crate has no GPUI
//! code, so they are registered here.

use crate::asset_path_editor::{asset_path_editor, AssetPathValue};
use gpui::{App, Window};
use helio_component::asset_paths::{LevelPath, TexturePath};
use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyEditorFactory};

macro_rules! path_picker {
    ($ty:ty, $editor:ident, $empty:literal) => {
        impl AssetPathValue for $ty {
            fn from_path(path: String) -> Self {
                Self(path)
            }

            fn path(&self) -> &str {
                &self.0
            }
        }

        fn $editor(
            args: &PropertyEditorArgs<'_>,
            window: &mut Window,
            cx: &mut App,
        ) -> BoundPropertyEditor {
            asset_path_editor::<$ty>(args, $empty, <$ty>::EXTENSIONS, window, cx)
        }

        pulsar_reflection::inventory::submit! {
            pulsar_reflection::UiPropertyEditorHint {
                type_id: std::any::TypeId::of::<$ty>(),
                fn_ptr: pulsar_reflection::erase_property_editor_fn_ptr(
                    $editor as PropertyEditorFactory
                ),
            }
        }
    };
}

path_picker!(TexturePath, texture_path_editor, "No image");
path_picker!(LevelPath, level_path_editor, "No level");
