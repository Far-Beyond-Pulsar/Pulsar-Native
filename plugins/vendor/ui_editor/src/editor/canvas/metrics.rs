//! Text measurement and painting with the window's real text system, so the
//! designer lays text out exactly as it is drawn.

use gpui::*;

use crate::layout::{TextMetrics, TextRequest};
use crate::model::Vec2;

pub fn font_weight(name: &str) -> FontWeight {
    match name {
        "medium" => FontWeight::MEDIUM,
        "semibold" => FontWeight::SEMIBOLD,
        "bold" => FontWeight::BOLD,
        _ => FontWeight::NORMAL,
    }
}

/// The font a widget's text is set in.
pub fn font_for(family: &SharedString, weight: &str, italic: bool) -> Font {
    Font {
        family: family.clone(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: font_weight(weight),
        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

fn run(len: usize, font: Font, color: Hsla) -> TextRun {
    TextRun { len, font, color: color.into(), background_color: None, underline: None, strikethrough: None, letter_spacing: None }
}

/// Measures with the window's text system.
pub struct WindowMetrics<'a> {
    pub window: &'a Window,
    pub family: SharedString,
}

impl TextMetrics for WindowMetrics<'_> {
    fn measure(&self, r: &TextRequest) -> Vec2 {
        if r.text.is_empty() || r.size <= 0.0 {
            return Vec2::new(0.0, r.size.max(0.0) * r.line_height);
        }
        let font = font_for(&self.family, &r.weight, r.italic);
        let runs = [run(r.text.len(), font, Hsla::default())];
        let line_height = r.size * r.line_height;
        let lines = self.window.text_system().shape_text(
            SharedString::from(r.text.clone()),
            px(r.size),
            &runs,
            r.wrap_width.map(px),
            None,
        );
        match lines {
            Ok(lines) => {
                let (mut width, mut height) = (0.0f32, 0.0f32);
                for line in &lines {
                    let size = line.size(px(line_height));
                    width = width.max(f32::from(size.width));
                    height += f32::from(size.height);
                }
                Vec2::new(width, height)
            }
            Err(_) => Vec2::new(r.text.chars().count() as f32 * r.size * 0.5, line_height),
        }
    }
}

/// Paint `text` into `bounds` (top-aligned), wrapping to its width when asked.
#[allow(clippy::too_many_arguments)]
pub fn paint_text(
    window: &mut Window,
    cx: &mut App,
    family: &SharedString,
    bounds: Bounds<Pixels>,
    text: &str,
    size: f32,
    weight: &str,
    italic: bool,
    color: Hsla,
    align: TextAlign,
    wrap: bool,
    line_height: f32,
) {
    if text.is_empty() || size < 1.0 || bounds.size.width <= px(0.0) {
        return;
    }
    let font = font_for(family, weight, italic);
    let runs = [run(text.len(), font, color)];
    let wrap_width = wrap.then_some(bounds.size.width);
    let Ok(lines) = window.text_system().shape_text(SharedString::from(text.to_string()), px(size), &runs, wrap_width, None) else {
        return;
    };
    let line_height = px(size * line_height);
    let mut origin = bounds.origin;
    for line in &lines {
        // Alignment is within the box even when the line is wider than it.
        let _ = line.paint(origin, line_height, align, Some(bounds), window, cx);
        origin.y += line.size(line_height).height;
    }
}
