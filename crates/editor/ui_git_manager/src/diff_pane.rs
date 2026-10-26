//! Read-only diff viewing on Mockaco — the editor the code editor plugin uses.
//!
//! A diff is two documents shown by read-only [`WgpuiEditorView`]s: one merged
//! document for the unified mode, and an old / new pair aligned line for line
//! (filler lines pad the shorter side of every change) for side-by-side mode.
//! Changed lines are tinted with Mockaco's line backgrounds. The views are
//! created once and re-pointed at each file, so Mockaco's highlighting,
//! folding, search and scrolling come for free.

use crate::GitManager;
use gpui::*;
use mockaco_gpui::native::WgpuiEditorView;
use mockaco_gpui::{EditorSurface, Language, SurfaceColor, SurfaceGeometry, SurfaceTheme};
use similar::{DiffOp, TextDiff};
use std::collections::HashMap;
use ui::ActiveTheme as _;

const FONT_FAMILY: &str = "JetBrains Mono";
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// Padding that keeps the two sides of a split diff aligned.
    Filler,
}

/// The documents a diff is shown with.
#[derive(Debug, Clone, Default)]
pub struct DiffLayout {
    pub unified: Vec<String>,
    pub unified_kinds: Vec<LineKind>,
    pub left: Vec<String>,
    pub left_kinds: Vec<LineKind>,
    pub right: Vec<String>,
    pub right_kinds: Vec<LineKind>,
    pub additions: usize,
    pub deletions: usize,
    /// First changed line of the unified document.
    pub first_change_unified: usize,
    /// First changed line of the aligned documents.
    pub first_change_split: usize,
}

impl DiffLayout {
    pub fn compute(old: &str, new: &str) -> Self {
        let diff = TextDiff::from_lines(old, new);
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();
        let mut layout = DiffLayout::default();
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
                            .unwrap_or("");
                        layout.unified.push(line.to_string());
                        layout.unified_kinds.push(LineKind::Context);
                        layout.left.push(line.to_string());
                        layout.left_kinds.push(LineKind::Context);
                        layout.right.push(line.to_string());
                        layout.right_kinds.push(LineKind::Context);
                    }
                }
                DiffOp::Delete {
                    old_index, old_len, ..
                } => layout.push_change(
                    &old_lines[old_index.min(old_lines.len())
                        ..(old_index + old_len).min(old_lines.len())],
                    &[],
                    &mut first_unified,
                    &mut first_split,
                ),
                DiffOp::Insert {
                    new_index, new_len, ..
                } => layout.push_change(
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
                } => layout.push_change(
                    &old_lines[old_index.min(old_lines.len())
                        ..(old_index + old_len).min(old_lines.len())],
                    &new_lines[new_index.min(new_lines.len())
                        ..(new_index + new_len).min(new_lines.len())],
                    &mut first_unified,
                    &mut first_split,
                ),
            }
        }
        layout.first_change_unified = first_unified.unwrap_or(0);
        layout.first_change_split = first_split.unwrap_or(0);
        layout
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
            self.unified.push((*line).to_string());
            self.unified_kinds.push(LineKind::Removed);
        }
        for line in added {
            self.unified.push((*line).to_string());
            self.unified_kinds.push(LineKind::Added);
        }
        self.deletions += removed.len();
        self.additions += added.len();

        let rows = removed.len().max(added.len());
        for i in 0..rows {
            match removed.get(i) {
                Some(line) => {
                    self.left.push((*line).to_string());
                    self.left_kinds.push(LineKind::Removed);
                }
                None => {
                    self.left.push(String::new());
                    self.left_kinds.push(LineKind::Filler);
                }
            }
            match added.get(i) {
                Some(line) => {
                    self.right.push((*line).to_string());
                    self.right_kinds.push(LineKind::Added);
                }
                None => {
                    self.right.push(String::new());
                    self.right_kinds.push(LineKind::Filler);
                }
            }
        }
    }
}

/// The three read-only editor views of the diff panel.
pub struct DiffPane {
    pub unified: Entity<WgpuiEditorView>,
    pub left: Entity<WgpuiEditorView>,
    pub right: Entity<WgpuiEditorView>,
    /// Statistics of the file currently shown.
    pub additions: usize,
    pub deletions: usize,
}

fn new_view(cx: &mut Context<GitManager>) -> Entity<WgpuiEditorView> {
    let theme = surface_theme(cx.theme());
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

impl DiffPane {
    pub fn new(cx: &mut Context<GitManager>) -> Self {
        Self {
            unified: new_view(cx),
            left: new_view(cx),
            right: new_view(cx),
            additions: 0,
            deletions: 0,
        }
    }

    /// Keep the two sides of the split view scrolled together.
    pub fn link_scrolling(&self, cx: &mut Context<GitManager>) {
        let (left, right) = (self.left.clone(), self.right.clone());
        let (a, b) = (left.clone(), right.clone());
        cx.observe(&left, move |_, _, cx| follow(&a, &b, cx))
            .detach();
        let (a, b) = (right.clone(), left.clone());
        cx.observe(&right, move |_, _, cx| follow(&a, &b, cx))
            .detach();
    }

    /// Show `old` → `new` for the file at `path`.
    pub fn show(&mut self, path: &str, old: &str, new: &str, cx: &mut Context<GitManager>) {
        let layout = DiffLayout::compute(old, new);
        self.additions = layout.additions;
        self.deletions = layout.deletions;

        let (theme, tints) = {
            let theme = cx.theme();
            (surface_theme(theme), Tints::from_theme(theme))
        };
        let language = Language::from_path(path);

        let apply = |view: &Entity<WgpuiEditorView>,
                     lines: &[String],
                     kinds: &[LineKind],
                     first: usize,
                     cx: &mut Context<GitManager>| {
            let backgrounds = tints.backgrounds(kinds);
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
            layout.first_change_unified,
            cx,
        );
        apply(
            &self.left,
            &layout.left,
            &layout.left_kinds,
            layout.first_change_split,
            cx,
        );
        apply(
            &self.right,
            &layout.right,
            &layout.right_kinds,
            layout.first_change_split,
            cx,
        );
    }

    /// Empty the views (nothing selected).
    pub fn clear(&mut self, cx: &mut Context<GitManager>) {
        self.additions = 0;
        self.deletions = 0;
        for view in [&self.unified, &self.left, &self.right] {
            view.update(cx, |view, cx| {
                view.replace_text(String::new(), cx);
                view.set_line_backgrounds(HashMap::new(), cx);
            });
        }
    }
}

fn follow(source: &Entity<WgpuiEditorView>, target: &Entity<WgpuiEditorView>, cx: &mut App) {
    let (x, y) = source.read(cx).scroll_offset();
    let (tx, ty) = target.read(cx).scroll_offset();
    if (x - tx).abs() > 0.5 || (y - ty).abs() > 0.5 {
        target.update(cx, |view, cx| view.set_scroll_offset(x, y, cx));
    }
}

/// Line tints for the three kinds of changed line.
struct Tints {
    added: SurfaceColor,
    removed: SurfaceColor,
    filler: SurfaceColor,
}

impl Tints {
    fn from_theme(theme: &ui::Theme) -> Self {
        Self {
            added: with_alpha(surface_color(theme.success), 70),
            removed: with_alpha(surface_color(theme.danger), 70),
            filler: with_alpha(surface_color(theme.muted_foreground), 28),
        }
    }

    fn backgrounds(&self, kinds: &[LineKind]) -> HashMap<usize, SurfaceColor> {
        kinds
            .iter()
            .enumerate()
            .filter_map(|(line, kind)| match kind {
                LineKind::Context => None,
                LineKind::Added => Some((line, self.added)),
                LineKind::Removed => Some((line, self.removed)),
                LineKind::Filler => Some((line, self.filler)),
            })
            .collect()
    }
}

// ── Theme bridge (the same mapping the code editor plugin uses) ─────────────

fn surface_color(color: Hsla) -> SurfaceColor {
    let rgba: Rgba = color.into();
    SurfaceColor::rgba(
        (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn with_alpha(color: SurfaceColor, alpha: u8) -> SurfaceColor {
    SurfaceColor { alpha, ..color }
}

/// Mockaco token kinds mapped to the UI theme's syntax names, first match
/// wins. Kinds the theme does not style keep Mockaco's built-in color.
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
            let Some(color) = style.color else {
                continue;
            };
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

fn surface_theme(theme: &ui::Theme) -> SurfaceTheme {
    let editor = &theme.highlight_theme.style;
    let active_line = editor
        .editor_active_line
        .map(surface_color)
        .unwrap_or_else(|| with_alpha(surface_color(theme.list_active), 90));
    let gutter_foreground = editor
        .editor_line_number
        .map(surface_color)
        .unwrap_or_else(|| surface_color(theme.muted_foreground));
    let background = surface_color(theme.background);
    SurfaceTheme {
        background,
        gutter_background: background,
        foreground: surface_color(theme.foreground),
        gutter_foreground,
        selection: surface_color(theme.selection),
        primary_selection: surface_color(theme.selection),
        caret: surface_color(theme.caret),
        decoration: surface_color(theme.warning),
        active_line,
        syntax: syntax_theme(theme),
    }
}

#[cfg(test)]
mod tests {
    use super::{DiffLayout, LineKind};

    #[test]
    fn replace_pads_the_shorter_side() {
        let layout = DiffLayout::compute("a\nb\nc\n", "a\nx\ny\nz\nc\n");
        assert_eq!(layout.left.len(), layout.right.len());
        assert_eq!(layout.deletions, 1);
        assert_eq!(layout.additions, 3);
        assert_eq!(layout.first_change_split, 1);
        assert!(layout.left_kinds.contains(&LineKind::Filler));
    }

    #[test]
    fn identical_text_has_no_changes() {
        let layout = DiffLayout::compute("same\n", "same\n");
        assert_eq!(layout.additions + layout.deletions, 0);
        assert_eq!(layout.unified, vec!["same".to_string()]);
    }

    #[test]
    fn new_file_is_all_additions() {
        let layout = DiffLayout::compute("", "one\ntwo\n");
        assert_eq!(layout.additions, 2);
        assert_eq!(layout.right_kinds, vec![LineKind::Added; 2]);
        assert_eq!(layout.left_kinds, vec![LineKind::Filler; 2]);
    }
}
