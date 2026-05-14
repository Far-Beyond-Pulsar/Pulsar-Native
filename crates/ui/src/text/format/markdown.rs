use gpui::SharedString;
use markdown::{
    mdast::{self, Node},
    Constructs, ParseOptions,
};
use mathjax_svg_rs::{
    render_tex as render_mathjax_tex, HorizontalAlign, Options as MathJaxOptions,
};
use mermaid_rs_renderer::render as render_mermaid;
use once_cell::sync::Lazy;
use regex::Regex;
use std::{collections::HashMap, sync::Mutex};

use crate::{
    highlighter::HighlightTheme,
    text::{
        node::{
            self, CodeBlock, ImageNode, InlineNode, LinkMark, NodeContext, Paragraph, Span, Table,
            TableRow, TextMark,
        },
        TextViewStyle,
    },
};

static MATH_SVG_CACHE: Lazy<Mutex<HashMap<(bool, String), SharedString>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static MERMAID_SVG_CACHE: Lazy<Mutex<HashMap<String, SharedString>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn normalize_svg(svg: &str) -> String {
    static SVG_OPEN_TAG_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)^<svg\b([^>]*)>").expect("valid svg tag regex"));
    static WIDTH_EX_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"\bwidth="([0-9]*\.?[0-9]+)ex""#).expect("valid width ex regex"));
    static HEIGHT_EX_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"\bheight="([0-9]*\.?[0-9]+)ex""#).expect("valid height ex regex")
    });
    static STYLE_ATTR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"\sstyle="[^"]*""#).expect("valid style attr regex"));
    static XLINK_HREF_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"\bxlink:href="#).expect("valid xlink href regex"));

    let Some(captures) = SVG_OPEN_TAG_RE.captures(svg) else {
        return svg.to_string();
    };

    let mut attributes = captures
        .get(1)
        .map(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    // GPUI's SVG decoding path does not consistently honor CSS `ex` lengths.
    // Convert them to explicit px dimensions derived from MathJax's 16px font size.
    attributes = WIDTH_EX_RE
        .replace_all(&attributes, |caps: &regex::Captures| {
            let ex = caps
                .get(1)
                .and_then(|v| v.as_str().parse::<f32>().ok())
                .unwrap_or(0.0);
            format!(r#"width="{:.3}px""#, ex * 8.0)
        })
        .into_owned();

    attributes = HEIGHT_EX_RE
        .replace_all(&attributes, |caps: &regex::Captures| {
            let ex = caps
                .get(1)
                .and_then(|v| v.as_str().parse::<f32>().ok())
                .unwrap_or(0.0);
            format!(r#"height="{:.3}px""#, ex * 8.0)
        })
        .into_owned();

    // `vertical-align` is an HTML/CSS concern; remove inline style to avoid parser quirks.
    let normalized_attributes = STYLE_ATTR_RE.replace_all(&attributes, "");
    let open_tag_end = captures.get(0).map(|value| value.end()).unwrap_or(0);

    let normalized = format!("<svg{}>{}", normalized_attributes, &svg[open_tag_end..]);

    // Some SVG renderers only honor `href` on <use>, not legacy `xlink:href`.
    XLINK_HREF_RE.replace_all(&normalized, "href=").into_owned()
}

fn render_math_svg(value: &str, display_mode: bool) -> Option<SharedString> {
    let cache_key = (display_mode, value.to_string());
    if let Some(cached) = MATH_SVG_CACHE.lock().ok()?.get(&cache_key).cloned() {
        return Some(cached);
    }

    let svg = render_mathjax_tex(
        value,
        &MathJaxOptions {
            font_size: 16.0,
            horizontal_align: HorizontalAlign::Center,
        },
    )
    .ok()?;

    let svg: SharedString = normalize_svg(&svg).into();

    if let Ok(mut cache) = MATH_SVG_CACHE.lock() {
        cache.insert(cache_key, svg.clone());
    }

    Some(svg)
}

fn render_mermaid_svg(value: &str) -> Option<SharedString> {
    if let Some(cached) = MERMAID_SVG_CACHE.lock().ok()?.get(value).cloned() {
        return Some(cached);
    }

    let svg: SharedString = normalize_svg(&render_mermaid(value).ok()?).into();

    if let Ok(mut cache) = MERMAID_SVG_CACHE.lock() {
        cache.insert(value.to_string(), svg.clone());
    }

    Some(svg)
}

#[cfg(test)]
mod tests {
    use super::render_math_svg;

    #[test]
    fn print_math_svg_debug_output() {
        let inline = render_math_svg(r"x^2 + y^2", false).expect("inline svg");
        let block = render_math_svg(r"\\frac{a}{b}", true).expect("block svg");

        tracing::debug!("INLINE SVG:\n{}", inline);
        tracing::debug!("BLOCK SVG:\n{}", block);
    }
}

static BLOCK_DELIM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\\\[(?s:(.*?))\\\]").expect("valid block math regex"));
static INLINE_DELIM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\\\((.*?)\\\)").expect("valid inline math regex"));

fn normalize_katex_default_delimiters(raw: &str) -> String {
    fn normalize_non_code_span(segment: &str) -> String {
        // Convert KaTeX default delimiters to markdown-math delimiters.
        // \[...\] -> $$...$$, \(...\) -> $...$
        let with_blocks = BLOCK_DELIM_RE
            .replace_all(segment, |caps: &regex::Captures| {
                format!("$$\n{}\n$$", &caps[1])
            })
            .into_owned();

        INLINE_DELIM_RE
            .replace_all(&with_blocks, |caps: &regex::Captures| {
                format!("${}$", &caps[1])
            })
            .into_owned()
    }

    // Skip fenced code blocks when normalizing delimiters.
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    let mut in_fence = false;

    while let Some(ix) = rest.find("```") {
        let (segment, tail) = rest.split_at(ix);
        if in_fence {
            out.push_str(segment);
        } else {
            out.push_str(&normalize_non_code_span(segment));
        }

        out.push_str("```");
        rest = &tail[3..];
        in_fence = !in_fence;
    }

    if in_fence {
        out.push_str(rest);
    } else {
        out.push_str(&normalize_non_code_span(rest));
    }

    out
}

/// Parse Markdown into a tree of nodes.
pub(crate) fn parse(
    raw: &str,
    style: &TextViewStyle,
    cx: &mut NodeContext,
    highlight_theme: &HighlightTheme,
) -> Result<node::Node, SharedString> {
    let normalized = normalize_katex_default_delimiters(raw);
    let parse_options = ParseOptions {
        constructs: Constructs {
            math_flow: true,
            math_text: true,
            ..Constructs::gfm()
        },
        ..ParseOptions::gfm()
    };

    markdown::to_mdast(&normalized, &parse_options)
        .map(|n| ast_to_node(n, style, cx, highlight_theme))
        .map_err(|e| e.to_string().into())
}

fn parse_table_row(table: &mut Table, node: &mdast::TableRow, cx: &mut NodeContext) {
    let mut row = TableRow::default();
    node.children.iter().for_each(|c| {
        match c {
            Node::TableCell(cell) => {
                parse_table_cell(&mut row, cell, cx);
            }
            _ => {}
        };
    });
    table.children.push(row);
}

fn parse_table_cell(row: &mut node::TableRow, node: &mdast::TableCell, cx: &mut NodeContext) {
    let mut paragraph = Paragraph::default();
    node.children.iter().for_each(|c| {
        parse_paragraph(&mut paragraph, c, cx);
    });
    let table_cell = node::TableCell {
        children: paragraph,
        ..Default::default()
    };
    row.children.push(table_cell);
}

fn parse_paragraph(paragraph: &mut Paragraph, node: &mdast::Node, cx: &mut NodeContext) -> String {
    let span = node.position().map(|pos| Span {
        start: pos.start.offset,
        end: pos.end.offset,
    });
    if let Some(span) = span {
        paragraph.set_span(span);
    }

    let mut text = String::new();

    match node {
        Node::Paragraph(val) => {
            val.children.iter().for_each(|c| {
                text.push_str(&parse_paragraph(paragraph, c, cx));
            });
        }
        Node::Text(val) => {
            text = val.value.clone();
            paragraph.push_str(&val.value)
        }
        Node::Emphasis(val) => {
            let mut child_paragraph = Paragraph::default();
            for child in val.children.iter() {
                text.push_str(&parse_paragraph(&mut child_paragraph, &child, cx));
            }
            paragraph.push(
                InlineNode::new(&text).marks(vec![(0..text.len(), TextMark::default().italic())]),
            );
        }
        Node::Strong(val) => {
            let mut child_paragraph = Paragraph::default();
            for child in val.children.iter() {
                text.push_str(&parse_paragraph(&mut child_paragraph, &child, cx));
            }
            paragraph.push(
                InlineNode::new(&text).marks(vec![(0..text.len(), TextMark::default().bold())]),
            );
        }
        Node::Delete(val) => {
            let mut child_paragraph = Paragraph::default();
            for child in val.children.iter() {
                text.push_str(&parse_paragraph(&mut child_paragraph, &child, cx));
            }
            paragraph.push(
                InlineNode::new(&text)
                    .marks(vec![(0..text.len(), TextMark::default().strikethrough())]),
            );
        }
        Node::InlineCode(val) => {
            text = val.value.clone();
            paragraph.push(
                InlineNode::new(&text).marks(vec![(0..text.len(), TextMark::default().code())]),
            );
        }
        Node::Link(val) => {
            let link_mark = Some(LinkMark {
                url: val.url.clone().into(),
                title: val.title.clone().map(|s| s.into()),
                ..Default::default()
            });

            let mut child_paragraph = Paragraph::default();
            for child in val.children.iter() {
                text.push_str(&parse_paragraph(&mut child_paragraph, &child, cx));
            }

            // FIXME: GPUI InteractiveText does not support inline images yet.
            // So here we push images to the paragraph directly.
            for child in child_paragraph.children.iter_mut() {
                if let Some(image) = child.image.as_mut() {
                    image.link = link_mark.clone();
                }

                child.marks.push((
                    0..child.text.len(),
                    TextMark {
                        link: link_mark.clone(),
                        ..Default::default()
                    },
                ));
            }

            paragraph.merge(child_paragraph);
        }
        Node::Image(raw) => {
            paragraph.push_image(ImageNode {
                url: raw.url.clone().into(),
                title: raw.title.clone().map(|t| t.into()),
                alt: Some(raw.alt.clone().into()),
                ..Default::default()
            });
        }
        Node::InlineMath(raw) => {
            text = raw.value.clone();
            if let Some(svg) = render_math_svg(&raw.value, false) {
                paragraph.push_image(ImageNode {
                    url: raw.value.clone().into(),
                    alt: Some(raw.value.clone().into()),
                    title: Some(raw.value.clone().into()),
                    math_tex: Some(raw.value.clone().into()),
                    math_svg: Some(svg),
                    math_display_mode: false,
                    ..Default::default()
                });
            } else {
                paragraph.push(
                    InlineNode::new(&text).marks(vec![(0..text.len(), TextMark::default().code())]),
                );
            }
        }
        Node::MdxTextExpression(raw) => {
            text = raw.value.clone();
            paragraph
                .push(InlineNode::new(&text).marks(vec![(0..text.len(), TextMark::default())]));
        }
        Node::Html(val) => match super::html::parse(&val.value, cx) {
            Ok(el) => {
                if el.is_break() {
                    text = "\n".to_owned();
                    paragraph.push(InlineNode::new(&text));
                } else {
                    if cfg!(debug_assertions) {
                        tracing::warn!("unsupported inline html tag: {:#?}", el);
                    }
                }
            }
            Err(err) => {
                if cfg!(debug_assertions) {
                    tracing::warn!("failed parsing html: {:#?}", err);
                }

                text.push_str(&val.value);
            }
        },
        Node::FootnoteReference(foot) => {
            let prefix = format!("[{}]", foot.identifier);
            paragraph.push(InlineNode::new(&prefix).marks(vec![(
                0..prefix.len(),
                TextMark {
                    italic: true,
                    ..Default::default()
                },
            )]));
        }
        Node::LinkReference(link) => {
            let mut child_paragraph = Paragraph::default();
            let mut child_text = String::new();
            for child in link.children.iter() {
                child_text.push_str(&parse_paragraph(&mut child_paragraph, child, cx));
            }

            let link_mark = LinkMark {
                url: "".into(),
                title: link.label.clone().map(Into::into),
                identifier: Some(link.identifier.clone().into()),
            };

            paragraph.push(InlineNode::new(&child_text).marks(vec![(
                0..child_text.len(),
                TextMark {
                    link: Some(link_mark),
                    ..Default::default()
                },
            )]));
        }
        _ => {
            if cfg!(debug_assertions) {
                tracing::warn!("unsupported inline node: {:#?}", node);
            }
        }
    }

    text
}

fn ast_to_node(
    value: mdast::Node,
    style: &TextViewStyle,
    cx: &mut NodeContext,
    highlight_theme: &HighlightTheme,
) -> node::Node {
    match value {
        Node::Root(val) => {
            let children = val
                .children
                .into_iter()
                .map(|c| ast_to_node(c, style, cx, highlight_theme))
                .collect();
            node::Node::Root { children }
        }
        Node::Paragraph(val) => {
            let mut paragraph = Paragraph::default();
            val.children.iter().for_each(|c| {
                parse_paragraph(&mut paragraph, c, cx);
            });

            node::Node::Paragraph(paragraph)
        }
        Node::Blockquote(val) => {
            let children = val
                .children
                .into_iter()
                .map(|c| ast_to_node(c, style, cx, highlight_theme))
                .collect();
            node::Node::Blockquote { children }
        }
        Node::List(list) => {
            let children = list
                .children
                .into_iter()
                .map(|c| ast_to_node(c, style, cx, highlight_theme))
                .collect();
            node::Node::List {
                ordered: list.ordered,
                children,
            }
        }
        Node::ListItem(val) => {
            let children = val
                .children
                .into_iter()
                .map(|c| ast_to_node(c, style, cx, highlight_theme))
                .collect();
            node::Node::ListItem {
                children,
                spread: val.spread,
                checked: val.checked,
            }
        }
        Node::Break(_) => node::Node::Break { html: false },
        Node::Code(raw) => {
            let lang = raw.lang.clone();
            let is_mermaid = lang
                .as_deref()
                .map(|lang| lang.eq_ignore_ascii_case("mermaid"))
                .unwrap_or(false);

            if is_mermaid {
                if let Some(svg) = render_mermaid_svg(&raw.value) {
                    let mut paragraph = Paragraph::default();
                    paragraph.push_image(ImageNode {
                        url: raw.value.clone().into(),
                        alt: Some(raw.value.clone().into()),
                        title: Some(raw.value.clone().into()),
                        mermaid_code: Some(raw.value.clone().into()),
                        mermaid_svg: Some(svg),
                        ..Default::default()
                    });
                    node::Node::Paragraph(paragraph)
                } else {
                    node::Node::CodeBlock(CodeBlock::new(
                        raw.value.into(),
                        lang.map(|s| s.into()),
                        style,
                        highlight_theme,
                    ))
                }
            } else {
                node::Node::CodeBlock(CodeBlock::new(
                    raw.value.into(),
                    lang.map(|s| s.into()),
                    style,
                    highlight_theme,
                ))
            }
        }
        Node::Heading(val) => {
            let mut paragraph = Paragraph::default();
            val.children.iter().for_each(|c| {
                parse_paragraph(&mut paragraph, c, cx);
            });

            node::Node::Heading {
                level: val.depth,
                children: paragraph,
            }
        }
        Node::Math(val) => {
            if let Some(svg) = render_math_svg(&val.value, true) {
                let mut paragraph = Paragraph::default();
                paragraph.push_image(ImageNode {
                    url: val.value.clone().into(),
                    alt: Some(val.value.clone().into()),
                    title: Some(val.value.clone().into()),
                    math_tex: Some(val.value.clone().into()),
                    math_svg: Some(svg),
                    math_display_mode: true,
                    ..Default::default()
                });
                node::Node::Paragraph(paragraph)
            } else {
                node::Node::CodeBlock(CodeBlock::new(
                    val.value.into(),
                    None,
                    style,
                    highlight_theme,
                ))
            }
        }
        Node::Html(val) => match super::html::parse(&val.value, cx) {
            Ok(el) => el,
            Err(err) => {
                if cfg!(debug_assertions) {
                    tracing::warn!("error parsing html: {:#?}", err);
                }

                node::Node::Paragraph(Paragraph::new(val.value))
            }
        },
        Node::MdxFlowExpression(val) => node::Node::CodeBlock(CodeBlock::new(
            val.value.into(),
            Some("mdx".into()),
            style,
            highlight_theme,
        )),
        Node::Yaml(val) => node::Node::CodeBlock(CodeBlock::new(
            val.value.into(),
            Some("yml".into()),
            style,
            highlight_theme,
        )),
        Node::Toml(val) => node::Node::CodeBlock(CodeBlock::new(
            val.value.into(),
            Some("toml".into()),
            style,
            highlight_theme,
        )),
        Node::MdxJsxTextElement(val) => {
            let mut paragraph = Paragraph::default();
            val.children.iter().for_each(|c| {
                parse_paragraph(&mut paragraph, c, cx);
            });
            node::Node::Paragraph(paragraph)
        }
        Node::MdxJsxFlowElement(val) => {
            let mut paragraph = Paragraph::default();
            val.children.iter().for_each(|c| {
                parse_paragraph(&mut paragraph, c, cx);
            });
            node::Node::Paragraph(paragraph)
        }
        Node::ThematicBreak(_) => node::Node::Divider,
        Node::Table(val) => {
            let mut table = Table::default();
            table.column_aligns = val
                .align
                .clone()
                .into_iter()
                .map(|align| align.into())
                .collect();
            val.children.iter().for_each(|c| {
                if let Node::TableRow(row) = c {
                    parse_table_row(&mut table, row, cx);
                }
            });

            node::Node::Table(table)
        }
        Node::FootnoteDefinition(def) => {
            let mut paragraph = Paragraph::default();
            let prefix = format!("[{}]: ", def.identifier);
            paragraph.push(InlineNode::new(&prefix).marks(vec![(
                0..prefix.len(),
                TextMark {
                    italic: true,
                    ..Default::default()
                },
            )]));

            def.children.iter().for_each(|c| {
                parse_paragraph(&mut paragraph, c, cx);
            });
            node::Node::Paragraph(paragraph)
        }
        Node::Definition(def) => {
            cx.add_ref(
                def.identifier.clone().into(),
                LinkMark {
                    url: def.url.clone().into(),
                    identifier: Some(def.identifier.clone().into()),
                    title: def.title.clone().map(Into::into),
                },
            );

            node::Node::Definition {
                identifier: def.identifier.clone().into(),
                url: def.url.clone().into(),
                title: def.title.clone().map(|s| s.into()),
            }
        }
        _ => {
            if cfg!(debug_assertions) {
                tracing::warn!("unsupported node: {:#?}", value);
            }
            node::Node::Unknown
        }
    }
}
