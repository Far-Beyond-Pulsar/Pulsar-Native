//! The sidebar's content tree: the project's folders, and the files in each
//! expanded folder (#1139).
//!
//! Clicking a folder expands it; double-clicking a file opens it. Nothing here
//! opens the bottom file drawer unless asked to from a row's right-click menu
//! ("Open in file drawer", "Reveal in file drawer"), which also renames,
//! deletes, cuts, copies and pastes through the drawer's operations.

use std::path::{Path, PathBuf};

use gpui::{div, prelude::*, px, AnyElement, Context, ElementId, IntoElement, Window};
use ui::input::TextInput;
use ui::menu::context_menu::ContextMenuExt as _;
use ui::popup_menu::PopupMenu;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};
use ui_file_manager::utils::get_icon_for_file_type;

use super::model::{content_root, ContentFile, ContentRow, FolderRow};
use super::render::{hold_while_open, section_caption};
use super::NavSidebar;

const ROW_HEIGHT: f32 = 26.;
/// Indent per tree level.
const INDENT: f32 = 12.;

/// What a right-click menu is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// The tree's top folder: it can't be renamed, moved or deleted here.
    Root,
    Folder,
    File,
}

impl NavSidebar {
    pub(super) fn render_content_tree(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let drawer = self.drawer.read(cx);
        let tree = drawer.folder_tree().map(content_root);
        let files = |folder: &Path| -> Vec<ContentFile> {
            drawer
                .files_in(folder)
                .into_iter()
                .map(|item| ContentFile {
                    icon: get_icon_for_file_type(&item),
                    color: item.file_type_def.as_ref().map(|def| def.color),
                    path: item.path,
                    name: item.name,
                })
                .collect()
        };
        let rows = tree
            .map(|root| self.model.content_rows(root, &files))
            .unwrap_or_default();
        let root = tree.map(|root| (root.path.clone(), root.name.clone()));

        let mut column = v_flex()
            .px_1()
            .mt_2()
            .pt_1()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(24.))
                    .px_2()
                    .child(section_caption("Content", cx)),
            );

        if let Some((path, name)) = root {
            let row = FolderRow {
                path,
                name,
                depth: 0,
                has_children: false,
                expanded: true,
            };
            column = column.child(self.render_folder_row(&row, true, cx));
        }
        for row in &rows {
            column = column.child(match row {
                ContentRow::Folder(folder) => self.render_folder_row(folder, false, cx),
                ContentRow::File { file, depth } => self.render_file_row(file, *depth, cx),
            });
        }
        if rows.is_empty() {
            column = column.child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No files yet"),
            );
        }
        column.into_any_element()
    }

    /// A folder. Clicking it expands or collapses it.
    fn render_folder_row(
        &self,
        row: &FolderRow,
        is_root: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.selected_path() == Some(row.path.as_path());
        let path = row.path.clone();
        let toggle_path = row.path.clone();
        // The root sits level with the top-level folders' chevrons.
        let indent = if is_root {
            0.
        } else {
            INDENT * (row.depth as f32 + 1.)
        };
        let open = selected || (row.expanded && row.has_children);
        let entry = if is_root { Entry::Root } else { Entry::Folder };

        tree_row(
            ElementId::Name(format!("nav-folder-{}", row.path.display()).into()),
            &row.name,
            indent,
            selected,
            cx,
        )
        .child(
            div()
                .id(ElementId::Name(
                    format!("nav-folder-toggle-{}", row.path.display()).into(),
                ))
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .when(row.has_children, |el| {
                    el.child(
                        Icon::new(if row.expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(12.))
                        .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(move |sidebar, _, _, cx| {
                        cx.stop_propagation();
                        sidebar.toggle_folder(&toggle_path, cx);
                    }))
                }),
        )
        .child(
            Icon::new(if open {
                IconName::FolderOpen
            } else {
                IconName::Folder
            })
            .size(px(14.))
            .text_color(if selected {
                theme.primary
            } else {
                theme.muted_foreground
            }),
        )
        .child(self.render_name(&row.path, &row.name))
        .on_click(cx.listener(move |sidebar, _, _, cx| {
            if is_root {
                sidebar.select(path.clone(), cx);
            } else {
                sidebar.click_folder(path.clone(), cx);
            }
        }))
        .context_menu(self.entry_menu(row.path.clone(), entry, cx))
        .into_any_element()
    }

    /// A file. Clicking selects it; double-clicking opens it.
    fn render_file_row(
        &self,
        file: &ContentFile,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.selected_path() == Some(file.path.as_path());
        let path = file.path.clone();

        tree_row(
            ElementId::Name(format!("nav-file-{}", file.path.display()).into()),
            &file.name,
            INDENT * (depth as f32 + 1.),
            selected,
            cx,
        )
        // In the chevron's column, so names line up with folders'.
        .child(div().size(px(14.)).flex_none())
        .child(
            Icon::new(file.icon.clone())
                .size(px(14.))
                .text_color(file.color.unwrap_or(theme.muted_foreground)),
        )
        .child(self.render_name(&file.path, &file.name))
        .on_click(cx.listener(move |sidebar, event: &gpui::ClickEvent, window, cx| {
            if event.click_count() >= 2 {
                sidebar.open_file(path.clone(), window, cx);
            } else {
                sidebar.select(path.clone(), cx);
            }
        }))
        .context_menu(self.entry_menu(file.path.clone(), Entry::File, cx))
        .into_any_element()
    }

    /// A row's name, or its text field while it is being renamed.
    fn render_name(&self, path: &Path, name: &str) -> AnyElement {
        match self.path_rename_field(path) {
            Some(field) => div()
                .flex_1()
                .min_w_0()
                .child(TextInput::new(field).xsmall())
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(name.to_string())
                .into_any_element(),
        }
    }

    /// A row's right-click menu.
    fn entry_menu(
        &self,
        path: PathBuf,
        entry: Entry,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let sidebar = cx.entity().downgrade();
        move |mut menu, _, cx| {
            hold_while_open(&sidebar, cx);
            let can_paste = sidebar
                .read_with(cx, |sidebar, cx| sidebar.can_paste(cx))
                .unwrap_or(false);
            // Where a paste goes: into a folder, or beside a file.
            let paste_target = match entry {
                Entry::File => path.parent().map(Path::to_path_buf),
                Entry::Root | Entry::Folder => Some(path.clone()),
            };
            let act = |f: fn(&mut NavSidebar, PathBuf, &mut Window, &mut Context<NavSidebar>),
                       path: PathBuf| {
                let sidebar = sidebar.clone();
                move |window: &mut Window, cx: &mut gpui::App| {
                    _ = sidebar.update(cx, |sidebar, cx| f(sidebar, path.clone(), window, cx));
                }
            };

            match entry {
                Entry::File => {
                    menu = menu
                        .menu_handler_with_icon(
                            "Open",
                            Icon::new(IconName::BookOpen),
                            act(|s, p, w, cx| s.open_file(p, w, cx), path.clone()),
                        )
                        .menu_handler_with_icon(
                            "Reveal in file drawer",
                            Icon::new(IconName::FolderOpen),
                            act(|s, p, _, cx| s.reveal_in_drawer(p, cx), path.clone()),
                        );
                }
                Entry::Root | Entry::Folder => {
                    menu = menu.menu_handler_with_icon(
                        "Open in file drawer",
                        Icon::new(IconName::FolderOpen),
                        act(|s, p, _, cx| s.open_in_drawer(p, cx), path.clone()),
                    );
                }
            }
            menu = menu.separator();
            if entry != Entry::Root {
                menu = menu
                    .menu_handler_with_icon(
                        "Cut",
                        Icon::new(IconName::Scissor),
                        act(|s, p, _, cx| s.put_on_clipboard(p, true, cx), path.clone()),
                    )
                    .menu_handler_with_icon(
                        "Copy",
                        Icon::new(IconName::Copy),
                        act(|s, p, _, cx| s.put_on_clipboard(p, false, cx), path.clone()),
                    );
            }
            if let Some(target) = paste_target.filter(|_| can_paste) {
                menu = menu.menu_handler_with_icon(
                    "Paste",
                    Icon::new(IconName::PasteClipboard),
                    act(|s, p, _, cx| s.paste_into(p, cx), target),
                );
            }
            if entry != Entry::Root {
                menu = menu
                    .separator()
                    .menu_handler_with_icon(
                        "Rename",
                        Icon::new(IconName::EditPencil),
                        act(|s, p, w, cx| s.start_path_rename(p, w, cx), path.clone()),
                    )
                    .menu_handler_with_icon(
                        "Delete",
                        Icon::new(IconName::Trash),
                        act(|s, p, _, cx| s.delete_path(p, cx), path.clone()),
                    );
            }
            menu
        }
    }
}

/// A row of the tree, indented by `indent`. Tests find it as
/// `nav-entry-<name>`.
fn tree_row(
    id: ElementId,
    name: &str,
    indent: f32,
    selected: bool,
    cx: &Context<NavSidebar>,
) -> gpui::Stateful<gpui::Div> {
    let theme = cx.theme();
    let selector = format!("nav-entry-{name}");
    h_flex()
        .id(id)
        .debug_selector(move || selector)
        .h(px(ROW_HEIGHT))
        .pl(px(8. + indent))
        .pr_2()
        .gap_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .text_sm()
        .text_color(theme.sidebar_foreground)
        .when(selected, |el| {
            el.bg(theme.sidebar_accent)
                .text_color(theme.sidebar_accent_foreground)
        })
        .when(!selected, |el| el.hover(|el| el.bg(theme.list_hover)))
}
