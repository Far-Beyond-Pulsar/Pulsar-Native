use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt};

use crate::model::{Entry, EntryKind};

/// One row of the list: name, what it contributes, its kind and state.
pub fn render_plugin_item(
    entry: &Entry,
    selected: bool,
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    let open_id = entry.id.clone();
    let summary = entry
        .groups
        .iter()
        .map(|group| format!("{} {}", group.items.len(), group.title.to_lowercase()))
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    let attention = entry.needs_attention();
    let badge = if entry.kind == EntryKind::Engine {
        "Engine".to_string()
    } else if entry.enabled {
        entry.kind.label().to_string()
    } else {
        format!("{} · off", entry.kind.label())
    };

    h_flex()
        .id(SharedString::from(format!("plugin-{}", entry.id)))
        .w_full()
        .h(px(68.))
        .px_2()
        .gap_2()
        .items_center()
        .justify_start()
        .rounded_md()
        .border_1()
        .border_color(if selected {
            cx.theme().border
        } else {
            cx.theme().transparent
        })
        .bg(if selected {
            cx.theme().secondary.opacity(0.55)
        } else {
            cx.theme().transparent
        })
        .when(!entry.enabled, |row| row.opacity(0.6))
        .cursor_pointer()
        .hover(|style| style.bg(cx.theme().secondary.opacity(0.4)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select(Some(open_id.clone()), cx);
        }))
        .child(render_plugin_icon(entry, 44.0, cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(entry.name.clone()),
                        )
                        .when(attention, |this| {
                            this.child(
                                Icon::new(IconName::TriangleAlert)
                                    .size_3()
                                    .text_color(cx.theme().warning),
                            )
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if summary.is_empty() {
                            format!("{} · contributes nothing", entry.id)
                        } else {
                            summary
                        }),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .px_1p5()
                .py_px()
                .rounded_sm()
                .bg(if entry.enabled {
                    cx.theme().success.opacity(0.12)
                } else {
                    cx.theme().secondary.opacity(0.55)
                })
                .text_xs()
                .text_color(if entry.enabled {
                    cx.theme().success
                } else {
                    cx.theme().muted_foreground
                })
                .child(badge),
        )
}

/// The images plugins registered, decoded once each (keyed by the bytes'
/// address: a `PluginIcon`'s bytes are `'static`).
fn icon_image(icon: &'static pulsar_plugins::PluginIcon) -> std::sync::Arc<Image> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static IMAGES: OnceLock<Mutex<HashMap<usize, Arc<Image>>>> = OnceLock::new();
    let format = match icon.format {
        pulsar_plugins::IconFormat::Png => ImageFormat::Png,
        pulsar_plugins::IconFormat::Jpeg => ImageFormat::Jpeg,
        pulsar_plugins::IconFormat::Webp => ImageFormat::Webp,
        pulsar_plugins::IconFormat::Svg => ImageFormat::Svg,
    };
    let mut images = IMAGES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    images
        .entry(icon.bytes.as_ptr() as usize)
        .or_insert_with(|| Arc::new(Image::from_bytes(format, icon.bytes.to_vec())))
        .clone()
}

/// An entry's icon in a `size` px square: the image its plugin registered
/// (`pulsar_plugins::PluginIcon`), else a glyph for its kind.
pub fn render_plugin_icon(
    entry: &Entry,
    size: f32,
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    let frame = div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_lg()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center();
    match pulsar_plugins::icon_of(&entry.id) {
        Some(icon) => frame
            .child(
                img(icon_image(icon))
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element(),
        None => frame
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.5))
            .child(
                Icon::new(match entry.kind {
                    EntryKind::Engine => IconName::Cpu,
                    EntryKind::Library => IconName::Box,
                    _ => IconName::Puzzle,
                })
                .size(px(size * 0.5))
                .text_color(cx.theme().muted_foreground),
            )
            .into_any_element(),
    }
}
