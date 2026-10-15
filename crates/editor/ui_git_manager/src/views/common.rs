//! Widgets shared by the Git Manager views.

use crate::{
    CopyFullPath, CopyRelativePath, DiscardFileChanges, IgnoreExtension, IgnoreFile, IgnoreFolder,
    ListState, OpenInExplorer, models::*,
};
use chrono::{DateTime, Utc};
use gpui::*;
use std::sync::Arc;
use ui::{
    ActiveTheme as _, Icon, IconName, VirtualList, h_flex,
    menu::popup_menu::PopupMenu,
    scroll::Scrollbar,
    v_flex,
};

pub(crate) const FONT_MONO: &str = "JetBrains Mono";

/// A virtualized list with its scrollbar, filling the space it is given.
pub(crate) fn virtual_panel(list: VirtualList, state: &ListState) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(list)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&state.bar, &state.scroll)),
        )
}

pub(crate) fn status_color(status: ChangeStatus, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        ChangeStatus::Added | ChangeStatus::Untracked => theme.success,
        ChangeStatus::Deleted => theme.danger,
        ChangeStatus::Renamed | ChangeStatus::Copied => theme.info,
        ChangeStatus::Modified => theme.warning,
    }
}

/// Colour of a graph lane; neighbouring lane ids get well separated hues.
pub(crate) fn lane_color(index: usize) -> Hsla {
    const HUES: [f32; 8] = [0.58, 0.08, 0.38, 0.83, 0.14, 0.50, 0.97, 0.70];
    hsla(HUES[index % HUES.len()], 0.68, 0.58, 1.0)
}

/// Deterministic hue from author name for the initials avatar.
fn author_hue(name: &str) -> f32 {
    let hash: u32 = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_add((b as u32).wrapping_mul(31)));
    (hash % 360) as f32 / 360.0
}

fn author_initials(name: &str) -> String {
    let mut parts = name.split_whitespace().filter(|s| !s.is_empty());
    let first = parts
        .next()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let last = parts
        .next_back()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    format!("{}{}", first, last)
}

/// A round avatar: the GitHub image when it has loaded, initials otherwise.
pub(crate) fn avatar(
    image: Option<Arc<gpui::RenderImage>>,
    author: &str,
    diameter: f32,
) -> AnyElement {
    match image {
        Some(image) => img(gpui::ImageSource::Render(image))
            .w(px(diameter))
            .h(px(diameter))
            .rounded_full()
            .flex_shrink_0()
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element(),
        None => h_flex()
            .w(px(diameter))
            .h(px(diameter))
            .rounded_full()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .bg(hsla(author_hue(author), 0.55, 0.45, 1.0))
            .text_size(px((diameter * 0.36).max(8.0)))
            .font_weight(FontWeight::BOLD)
            .text_color(white())
            .child(author_initials(author))
            .into_any_element(),
    }
}

/// "3 hours ago", "Mar 4", "Mar 4, 2023".
pub(crate) fn relative_time(time: DateTime<Utc>) -> String {
    let now = Utc::now();
    let seconds = (now - time).num_seconds();
    if seconds < 0 {
        return "just now".to_string();
    }
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    match () {
        _ if seconds < 45 => "just now".to_string(),
        _ if minutes < 60 => format!("{}m ago", minutes.max(1)),
        _ if hours < 24 => format!("{}h ago", hours),
        _ if days < 7 => format!("{}d ago", days),
        _ if time.format("%Y").to_string() == now.format("%Y").to_string() => {
            time.format("%b %-d").to_string()
        }
        _ => time.format("%b %-d, %Y").to_string(),
    }
}

/// A small rounded label.
pub(crate) fn pill(text: impl Into<SharedString>, color: Hsla, filled: bool) -> Div {
    let base = div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(4.))
        .flex_shrink_0()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into());
    if filled {
        base.bg(color.opacity(0.9)).text_color(white())
    } else {
        base.bg(color.opacity(0.14))
            .border_1()
            .border_color(color.opacity(0.4))
            .text_color(color)
    }
}

/// Centered icon + message.
pub(crate) fn placeholder(icon: IconName, message: impl Into<SharedString>, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_4()
        .child(Icon::new(icon).size(px(28.)).text_color(muted.opacity(0.7)))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .text_center()
                .child(message.into()),
        )
}

/// The file context menu of a working-tree file.
pub(crate) fn file_menu(
    path: String,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, window, cx| {
        let fp = path.clone();
        let ext = std::path::Path::new(&fp)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let folders: Vec<String> = {
            let norm = fp.replace('\\', "/");
            let mut parts: Vec<&str> = norm.split('/').collect();
            parts.pop();
            let mut folders = Vec::new();
            while !parts.is_empty() {
                folders.push(parts.join("/"));
                parts.pop();
            }
            folders
        };

        let menu = menu
            .menu(
                "Discard File Changes",
                Box::new(DiscardFileChanges { path: fp.clone() }),
            )
            .separator()
            .menu("Ignore File", Box::new(IgnoreFile { path: fp.clone() }));
        let menu = if folders.is_empty() {
            menu
        } else {
            menu.submenu("Ignore Folder", window, cx, move |mut sub, _w, _c| {
                for folder in &folders {
                    sub = sub.menu(
                        SharedString::from(folder.clone()),
                        Box::new(IgnoreFolder {
                            folder: folder.clone(),
                        }),
                    );
                }
                sub
            })
        };
        let menu = if ext.is_empty() {
            menu
        } else {
            menu.menu(
                SharedString::from(format!("Ignore *.{}", ext)),
                Box::new(IgnoreExtension { path: fp.clone() }),
            )
        };
        menu.separator()
            .menu(
                "Copy Relative Path",
                Box::new(CopyRelativePath { path: fp.clone() }),
            )
            .menu("Copy Full Path", Box::new(CopyFullPath { path: fp.clone() }))
            .separator()
            .menu("Show in Explorer", Box::new(OpenInExplorer { path: fp }))
    }
}

/// Copy-only menu for files of a commit.
pub(crate) fn commit_file_menu(
    path: String,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        menu.menu(
            "Copy Relative Path",
            Box::new(CopyRelativePath { path: path.clone() }),
        )
        .menu("Copy Full Path", Box::new(CopyFullPath { path: path.clone() }))
    }
}
