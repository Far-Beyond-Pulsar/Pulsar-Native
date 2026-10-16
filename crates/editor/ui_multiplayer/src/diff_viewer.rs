//! File synchronization review using the same Mockaco surfaces as Git Manager.

use gpui::{prelude::FluentBuilder, *};
use mockaco_gpui::native::WgpuiEditorView;
use mockaco_gpui::{EditorSurface, Language, SurfaceColor, SurfaceGeometry, SurfaceTheme};
use similar::{DiffOp, TextDiff};
use std::collections::HashMap;
use std::path::PathBuf;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName,
};

const FONT_FAMILY: &str = "JetBrains Mono";
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;

#[derive(Clone, Debug)]
pub struct DiffFileEntry {
    pub path: String,
    pub before_content: String,
    pub after_content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Context,
    Added,
    Removed,
    Filler,
}

#[derive(Default)]
struct DiffLayout {
    unified: Vec<String>,
    unified_kinds: Vec<LineKind>,
    left: Vec<String>,
    left_kinds: Vec<LineKind>,
    right: Vec<String>,
    right_kinds: Vec<LineKind>,
    additions: usize,
    deletions: usize,
    first_unified: usize,
    first_split: usize,
}

impl DiffLayout {
    fn compute(old: &str, new: &str) -> Self {
        let diff = TextDiff::from_lines(old, new);
        let old_lines: Vec<_> = old.lines().collect();
        let new_lines: Vec<_> = new.lines().collect();
        let mut out = Self::default();
        let mut first_unified = None;
        let mut first_split = None;
        for op in diff.ops() {
            match *op {
                DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                } => {
                    for i in 0..len {
                        let line = new_lines
                            .get(new_index + i)
                            .or_else(|| old_lines.get(old_index + i))
                            .copied()
                            .unwrap_or("")
                            .to_owned();
                        out.unified.push(line.clone());
                        out.unified_kinds.push(LineKind::Context);
                        out.left.push(line.clone());
                        out.left_kinds.push(LineKind::Context);
                        out.right.push(line);
                        out.right_kinds.push(LineKind::Context);
                    }
                }
                DiffOp::Delete {
                    old_index, old_len, ..
                } => out.push_change(
                    &old_lines[old_index.min(old_lines.len())
                        ..(old_index + old_len).min(old_lines.len())],
                    &[],
                    &mut first_unified,
                    &mut first_split,
                ),
                DiffOp::Insert {
                    new_index, new_len, ..
                } => out.push_change(
                    &[],
                    &new_lines[new_index.min(new_lines.len())
                        ..(new_index + new_len).min(new_lines.len())],
                    &mut first_unified,
                    &mut first_split,
                ),
                DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => out.push_change(
                    &old_lines[old_index.min(old_lines.len())
                        ..(old_index + old_len).min(old_lines.len())],
                    &new_lines[new_index.min(new_lines.len())
                        ..(new_index + new_len).min(new_lines.len())],
                    &mut first_unified,
                    &mut first_split,
                ),
            }
        }
        out.first_unified = first_unified.unwrap_or(0);
        out.first_split = first_split.unwrap_or(0);
        out
    }

    fn push_change(
        &mut self,
        removed: &[&str],
        added: &[&str],
        first_unified: &mut Option<usize>,
        first_split: &mut Option<usize>,
    ) {
        first_unified.get_or_insert(self.unified.len());
        first_split.get_or_insert(self.left.len());
        for line in removed {
            self.unified.push((*line).to_owned());
            self.unified_kinds.push(LineKind::Removed);
        }
        for line in added {
            self.unified.push((*line).to_owned());
            self.unified_kinds.push(LineKind::Added);
        }
        self.deletions += removed.len();
        self.additions += added.len();
        for i in 0..removed.len().max(added.len()) {
            if let Some(line) = removed.get(i) {
                self.left.push((*line).to_owned());
                self.left_kinds.push(LineKind::Removed);
            } else {
                self.left.push(String::new());
                self.left_kinds.push(LineKind::Filler);
            }
            if let Some(line) = added.get(i) {
                self.right.push((*line).to_owned());
                self.right_kinds.push(LineKind::Added);
            } else {
                self.right.push(String::new());
                self.right_kinds.push(LineKind::Filler);
            }
        }
    }
}

pub struct DiffViewer {
    focus_handle: FocusHandle,
    diff_files: Vec<DiffFileEntry>,
    project_root: Option<PathBuf>,
    selected_file_index: Option<usize>,
    split: bool,
    unified: Entity<WgpuiEditorView>,
    left: Entity<WgpuiEditorView>,
    right: Entity<WgpuiEditorView>,
    additions: usize,
    deletions: usize,
}

fn surface_color(color: Hsla) -> SurfaceColor {
    let rgba: Rgba = color.into();
    SurfaceColor::rgba(
        (rgba.r.clamp(0., 1.) * 255.).round() as u8,
        (rgba.g.clamp(0., 1.) * 255.).round() as u8,
        (rgba.b.clamp(0., 1.) * 255.).round() as u8,
        (rgba.a.clamp(0., 1.) * 255.).round() as u8,
    )
}

const SYNTAX_MAP: &[(&str, &[&str])] = &[
    ("keyword", &["keyword"]),
    ("keyword.operator", &["keyword", "operator"]),
    ("function", &["function"]),
    ("function.method", &["function"]),
    ("function.macro", &["function", "preproc"]),
    ("type", &["type", "enum"]),
    ("type.builtin", &["type"]),
    ("constructor", &["constructor", "type"]),
    ("string", &["string"]),
    ("character", &["string"]),
    ("escape", &["string.escape", "string"]),
    ("comment", &["comment"]),
    ("comment.documentation", &["comment.doc", "comment"]),
    ("constant", &["constant", "boolean"]),
    ("constant.builtin", &["constant", "boolean"]),
    ("number", &["number"]),
    ("attribute", &["attribute"]),
    ("property", &["property"]),
    ("variable", &["variable"]),
    ("variable.parameter", &["variable"]),
    ("variable.builtin", &["variable.special", "variable"]),
    ("operator", &["operator"]),
    (
        "punctuation.bracket",
        &["punctuation.bracket", "punctuation"],
    ),
    (
        "punctuation.delimiter",
        &["punctuation.delimiter", "punctuation"],
    ),
    (
        "punctuation.list_marker",
        &["punctuation.list_marker", "punctuation"],
    ),
    ("label", &["label"]),
    ("tag", &["tag"]),
    ("title", &["title"]),
    ("link_text", &["link_text"]),
    ("link_uri", &["link_uri"]),
    ("emphasis", &["emphasis"]),
    ("emphasis.strong", &["emphasis.strong"]),
];

fn syntax_theme(theme: &ui::Theme) -> mockaco_language::Theme {
    let mut syntax = SurfaceTheme::default().syntax;
    for (kind, names) in SYNTAX_MAP {
        for name in *names {
            let Some(style) = theme.highlight_theme.style(name) else {
                continue;
            };
            let Some(color) = style.color else { continue };
            let color = surface_color(color.solid);
            syntax.set_token_style(
                *kind,
                mockaco_language::TokenStyle {
                    foreground: mockaco_language::Rgba {
                        red: color.red,
                        green: color.green,
                        blue: color.blue,
                        alpha: color.alpha,
                    },
                    background: None,
                    font: mockaco_language::FontStyle {
                        bold: style
                            .font_weight
                            .is_some_and(|weight| weight >= FontWeight::SEMIBOLD),
                        italic: matches!(style.font_style, Some(FontStyle::Italic)),
                        underline: false,
                    },
                },
            );
            break;
        }
    }
    syntax
}

fn theme_for(theme: &ui::Theme) -> SurfaceTheme {
    let background = surface_color(theme.background);
    SurfaceTheme {
        background,
        gutter_background: background,
        foreground: surface_color(theme.foreground),
        gutter_foreground: surface_color(theme.muted_foreground),
        selection: surface_color(theme.selection),
        primary_selection: surface_color(theme.selection),
        caret: surface_color(theme.caret),
        decoration: surface_color(theme.warning),
        active_line: surface_color(theme.list_active),
        syntax: syntax_theme(theme),
        ..SurfaceTheme::default()
    }
}

fn new_view(cx: &mut Context<DiffViewer>) -> Entity<WgpuiEditorView> {
    let theme = theme_for(cx.theme());
    cx.new(|cx| {
        let surface = EditorSurface::with_language(
            String::new(),
            Default::default(),
            SurfaceGeometry::default(),
            Language::Plain,
        );
        let mut view = WgpuiEditorView::new_read_only(surface, cx);
        view.set_font(FONT_FAMILY, FONT_SIZE, LINE_HEIGHT);
        view.set_show_minimap(false, cx);
        view.set_theme(theme, cx);
        view
    })
}

impl DiffViewer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let unified = new_view(cx);
        let left = new_view(cx);
        let right = new_view(cx);
        let (a, b) = (left.clone(), right.clone());
        let (c, d) = (left.clone(), right.clone());
        cx.observe(&left, move |_, _, cx| follow(&a, &b, cx))
            .detach();
        cx.observe(&right, move |_, _, cx| follow(&d, &c, cx))
            .detach();
        Self {
            focus_handle: cx.focus_handle(),
            diff_files: Vec::new(),
            project_root: None,
            selected_file_index: None,
            split: true,
            unified,
            left,
            right,
            additions: 0,
            deletions: 0,
        }
    }

    pub fn enter_diff_mode(
        &mut self,
        diff_files: Vec<DiffFileEntry>,
        project_root: PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_files = diff_files;
        self.project_root = Some(project_root);
        self.selected_file_index = (!self.diff_files.is_empty()).then_some(0);
        self.refresh_selected(cx);
        cx.notify();
    }

    pub fn update_diff_file_after_content(
        &mut self,
        file_path: &str,
        content: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.diff_files.iter().position(|f| f.path == file_path) {
            self.diff_files[index].after_content = content;
            if self.selected_file_index == Some(index) {
                self.refresh_selected(cx);
            }
            cx.notify();
        }
    }

    fn refresh_selected(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self
            .selected_file_index
            .and_then(|i| self.diff_files.get(i))
        else {
            self.additions = 0;
            self.deletions = 0;
            for view in [&self.unified, &self.left, &self.right] {
                view.update(cx, |v, cx| {
                    v.replace_text(String::new(), cx);
                    v.set_line_backgrounds(HashMap::new(), cx);
                });
            }
            return;
        };
        let layout = DiffLayout::compute(&file.before_content, &file.after_content);
        self.additions = layout.additions;
        self.deletions = layout.deletions;
        let theme = theme_for(cx.theme());
        let success = surface_color(cx.theme().success);
        let danger = surface_color(cx.theme().danger);
        let neutral = surface_color(cx.theme().muted_foreground);
        let language = Language::from_path(&file.path);
        let apply = |view: &Entity<WgpuiEditorView>,
                     lines: &[String],
                     kinds: &[LineKind],
                     first: usize,
                     cx: &mut Context<DiffViewer>| {
            let backgrounds = kinds
                .iter()
                .enumerate()
                .filter_map(|(line, kind)| match kind {
                    LineKind::Context => None,
                    LineKind::Added => Some((
                        line,
                        SurfaceColor {
                            alpha: 70,
                            ..success
                        },
                    )),
                    LineKind::Removed => Some((
                        line,
                        SurfaceColor {
                            alpha: 70,
                            ..danger
                        },
                    )),
                    LineKind::Filler => Some((
                        line,
                        SurfaceColor {
                            alpha: 28,
                            ..neutral
                        },
                    )),
                })
                .collect();
            view.update(cx, |view, cx| {
                view.set_language(language, cx);
                view.set_theme(theme.clone(), cx);
                view.replace_text(lines.join("\n"), cx);
                view.set_line_backgrounds(backgrounds, cx);
                view.go_to(first.saturating_sub(3), 0, cx);
            });
        };
        apply(
            &self.unified,
            &layout.unified,
            &layout.unified_kinds,
            layout.first_unified,
            cx,
        );
        apply(
            &self.left,
            &layout.left,
            &layout.left_kinds,
            layout.first_split,
            cx,
        );
        apply(
            &self.right,
            &layout.right,
            &layout.right_kinds,
            layout.first_split,
            cx,
        );
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_file_index;
        v_flex()
            .w(px(220.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(px(36.))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("CHANGES"),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.diff_files.len().to_string()),
                    ),
            )
            .child(div().flex_1().overflow_y_scroll().children(
                self.diff_files.iter().enumerate().map(|(index, file)| {
                    let is_selected = selected == Some(index);
                    let name = file
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(file.path.as_str())
                        .to_owned();
                    let path = file.path.clone();
                    div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(cx.theme().list_active))
                        .hover(|d| d.bg(cx.theme().secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.selected_file_index = Some(index);
                                this.refresh_selected(cx);
                                cx.notify();
                            }),
                        )
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if is_selected {
                                            cx.theme().foreground
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(path),
                                ),
                        )
                }),
            ))
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(file) = self
            .selected_file_index
            .and_then(|i| self.diff_files.get(i))
        else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Select a changed file to review"),
                )
                .into_any_element();
        };
        let title = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_owned();
        let path = file.path.clone();
        let header = h_flex()
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(IconName::Code)
                    .size(px(14.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child(title),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child(path),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().success)
                    .child(format!("+{}", self.additions)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(format!("−{}", self.deletions)),
            )
            .child(
                Button::new("sync-diff-layout")
                    .icon(if self.split {
                        IconName::AlignLeft
                    } else {
                        IconName::ViewColumns2
                    })
                    .ghost()
                    .compact()
                    .tooltip(if self.split {
                        "Unified view"
                    } else {
                        "Side by side"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.split = !this.split;
                        cx.notify();
                    })),
            );
        let body = if self.split {
            h_flex()
                .size_full()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .h(px(22.))
                                .px_3()
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child("BEFORE"),
                        )
                        .child(div().flex_1().min_h_0().child(self.left.clone())),
                )
                .child(div().w(px(1.)).h_full().bg(cx.theme().border))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .h(px(22.))
                                .px_3()
                                .text_size(px(10.))
                                .text_color(cx.theme().muted_foreground)
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child("AFTER"),
                        )
                        .child(div().flex_1().min_h_0().child(self.right.clone())),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .child(self.unified.clone())
                .into_any_element()
        };
        v_flex()
            .size_full()
            .min_w_0()
            .child(header)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .into_any_element()
    }
}

fn follow(source: &Entity<WgpuiEditorView>, target: &Entity<WgpuiEditorView>, cx: &mut App) {
    let (x, y) = source.read(cx).scroll_offset();
    let (tx, ty) = target.read(cx).scroll_offset();
    if (x - tx).abs() > 0.5 || (y - ty).abs() > 0.5 {
        target.update(cx, |view, cx| view.set_scroll_offset(x, y, cx));
    }
}

impl Focusable for DiffViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DiffViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(self.render_file_list(cx))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_editor(cx)),
            )
    }
}
