use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use ui::button::{Button, ButtonGroup, ButtonVariants as _};
use ui::{h_flex, ActiveTheme as _, Icon, IconName, Selectable as _, StyledExt};

use super::FileManagerDrawer;
use crate::utils::types::*;

impl FileManagerDrawer {
    pub fn render_combined_toolbar(
        &mut self,
        items: &[FileItem],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let vm = self.view_mode;
        h_flex()
            .w_full()
            .h(px(56.))
            .px_4()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(self.render_clickable_breadcrumb(items, w, cx))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().accent.opacity(0.1))
                    .border_1()
                    .border_color(cx.theme().accent.opacity(0.3))
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().accent)
                    .child(t!("FileManager.Items", count => items.len()).to_string()),
            )
            .when(engine_fs::virtual_fs::is_remote(), |e| {
                e.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .bg(cx.theme().success.opacity(0.12))
                        .border_1()
                        .border_color(cx.theme().success.opacity(0.4))
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().success)
                        .child(format!("☁ {}", engine_fs::virtual_fs::current_label())),
                )
            })
            .child(ui::divider::Divider::vertical().h(px(24.)))
            .child(
                ButtonGroup::new("view-mode-group")
                    .child(
                        Button::new("toggle-view")
                            .icon(IconName::LayoutDashboard)
                            .tooltip(t!("FileManager.GridView").to_string())
                            .selected(vm == ViewMode::Grid),
                    )
                    .child(
                        Button::new("toggle-list")
                            .icon(IconName::List)
                            .tooltip(t!("FileManager.ListView").to_string())
                            .selected(vm == ViewMode::List),
                    )
                    .ghost()
                    .on_click(cx.listener(|d, s: &Vec<usize>, _w, cx| {
                        if s.contains(&0) {
                            d.view_mode = ViewMode::Grid;
                        } else if s.contains(&1) {
                            d.view_mode = ViewMode::List;
                        }
                        cx.notify();
                    })),
            )
            .child(ui::divider::Divider::vertical().h(px(24.)))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("new-file")
                            .icon(IconName::PagePlus)
                            .ghost()
                            .tooltip(t!("FileManager.NewFile").to_string())
                            .on_click(cx.listener(|d, _e, _w, cx| d.start_new_file(cx))),
                    )
                    .child(
                        Button::new("new-folder")
                            .icon(IconName::FolderPlus)
                            .ghost()
                            .tooltip(t!("FileManager.NewFolder").to_string())
                            .on_click(cx.listener(|d, _e, _w, cx| d.start_new_folder(cx))),
                    ),
            )
            .child(ui::divider::Divider::vertical().h(px(24.)))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("toggle-hidden")
                            .icon(if self.show_hidden_files {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .ghost()
                            .tooltip(if self.show_hidden_files {
                                t!("FileManager.HideHidden").to_string()
                            } else {
                                t!("FileManager.ShowHidden").to_string()
                            })
                            .on_click(cx.listener(|d, _e, _w, cx| {
                                d.show_hidden_files = !d.show_hidden_files;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("refresh")
                            .icon(IconName::Refresh)
                            .ghost()
                            .tooltip(t!("FileManager.Refresh").to_string())
                            .on_click(cx.listener(|d, _e, _w, cx| d.refresh(cx))),
                    ),
            )
            .child(ui::divider::Divider::vertical().h(px(24.)))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("external")
                            .icon(IconName::ExternalLink)
                            .ghost()
                            .tooltip(t!("FileManager.OpenInFileManager").to_string())
                            .on_click(cx.listener(|d, _e, _w, _cx| {
                                if let Some(ref f) = d.selected_folder {
                                    #[cfg(target_os = "windows")]
                                    let _ = std::process::Command::new("explorer").arg(f).spawn();
                                    #[cfg(target_os = "macos")]
                                    let _ = std::process::Command::new("open").arg(f).spawn();
                                    #[cfg(target_os = "linux")]
                                    let _ = std::process::Command::new("xdg-open").arg(f).spawn();
                                }
                            })),
                    )
                    .child(
                        Button::new("popout")
                            .icon(IconName::ArrowUpRightSquare)
                            .ghost()
                            .tooltip("Pop Out to New Window")
                            .on_click(cx.listener(|_d, _e, w: &mut Window, cx| {
                                cx.emit(PopoutFileManagerEvent {
                                    position: w.mouse_position(),
                                })
                            })),
                    ),
            )
    }

    pub fn render_clickable_breadcrumb(
        &mut self,
        _items: &[FileItem],
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut parts = Vec::new();
        if let Some(ref sel) = self.selected_folder {
            if let Some(ref proj) = self.project_path {
                if let Ok(rel) = sel.strip_prefix(proj) {
                    let mut cur = proj.clone();
                    parts.push(("Project".to_string(), cur.clone()));
                    for c in rel.components() {
                        if let Some(n) = c.as_os_str().to_str() {
                            cur = cur.join(n);
                            parts.push((n.to_string(), cur.clone()));
                        }
                    }
                }
            }
        }
        if parts.is_empty() {
            parts.push((
                "Project".to_string(),
                self.project_path.clone().unwrap_or_default(),
            ));
        }
        h_flex()
            .flex_1()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(px(8.))
            .bg(cx.theme().muted.opacity(0.3))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(IconName::Folder)
                    .size_4()
                    .text_color(cx.theme().accent),
            )
            .children(parts.into_iter().enumerate().flat_map(|(i, (name, path))| {
                let mut els: Vec<AnyElement> = Vec::new();
                if i > 0 {
                    els.push(
                        Icon::new(IconName::ChevronRight)
                            .size_3()
                            .text_color(cx.theme().muted_foreground)
                            .into_any_element(),
                    );
                }
                let cp = path.clone();
                let hp = path.clone();
                els.push(
                    div()
                        .text_sm()
                        .px_1()
                        .py_px()
                        .rounded(px(4.))
                        .text_color(cx.theme().foreground)
                        .font_medium()
                        .cursor_pointer()
                        .hover(|s| s.bg(cx.theme().accent.opacity(0.15)))
                        .child(name)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |d, _: &MouseDownEvent, _w: &mut Window, cx| {
                                d.selected_folder = Some(cp.clone());
                                cx.notify();
                            }),
                        )
                        .drag_over::<DraggedFile>(|s, _, _, cx| {
                            s.bg(cx.theme().accent.opacity(0.3))
                                .border_1()
                                .border_color(cx.theme().accent)
                        })
                        .on_drop(cx.listener(move |_d, _: &DraggedFile, _w, _cx| {}))
                        .on_mouse_move(cx.listener(move |d, _: &MouseMoveEvent, _w, cx| {
                            d.start_breadcrumb_hover_timer(&hp, cx)
                        }))
                        .into_any_element(),
                );
                els
            }))
    }
}
