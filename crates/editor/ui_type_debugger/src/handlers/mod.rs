use gpui::{Context, Window};

use plugin_editor_api::FileTypeId;

use crate::components::TypeDebuggerDrawer;
use crate::utils::{
    FilterAliases, FilterAll, FilterEnums, FilterStructs, FilterTraits, NavigateToType,
};

pub fn on_filter_all(
    d: &mut TypeDebuggerDrawer,
    _: &FilterAll,
    _: &mut Window,
    cx: &mut Context<TypeDebuggerDrawer>,
) {
    d.set_filter(None, cx);
}

pub fn on_filter_aliases(
    d: &mut TypeDebuggerDrawer,
    _: &FilterAliases,
    _: &mut Window,
    cx: &mut Context<TypeDebuggerDrawer>,
) {
    d.set_filter(Some(FileTypeId::new("alias")), cx);
}

pub fn on_filter_structs(
    d: &mut TypeDebuggerDrawer,
    _: &FilterStructs,
    _: &mut Window,
    cx: &mut Context<TypeDebuggerDrawer>,
) {
    d.set_filter(Some(FileTypeId::new("struct")), cx);
}

pub fn on_filter_enums(
    d: &mut TypeDebuggerDrawer,
    _: &FilterEnums,
    _: &mut Window,
    cx: &mut Context<TypeDebuggerDrawer>,
) {
    d.set_filter(Some(FileTypeId::new("enum")), cx);
}

pub fn on_filter_traits(
    d: &mut TypeDebuggerDrawer,
    _: &FilterTraits,
    _: &mut Window,
    cx: &mut Context<TypeDebuggerDrawer>,
) {
    d.set_filter(Some(FileTypeId::new("trait")), cx);
}
