use super::math::{clamp01, hsl_to_rgb};
use super::*;

pub(crate) fn parse_percent_or_unit(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if let Some(v) = trimmed.strip_suffix('%') {
        v.trim().parse::<f32>().ok().map(|n| clamp01(n / 100.0))
    } else {
        trimmed.parse::<f32>().ok().map(clamp01)
    }
}

pub(crate) fn parse_rgb_channel(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if let Some(v) = trimmed.strip_suffix('%') {
        v.trim().parse::<f32>().ok().map(|n| clamp01(n / 100.0))
    } else {
        trimmed
            .parse::<f32>()
            .ok()
            .map(|n| clamp01((n / 255.0).clamp(0.0, 1.0)))
    }
}

pub(crate) fn parse_hue(value: &str) -> Option<f32> {
    let trimmed = value.trim().trim_end_matches("deg").trim();
    trimmed
        .parse::<f32>()
        .ok()
        .map(|degrees| (degrees / 360.0).rem_euclid(1.0))
}

pub(crate) fn parse_color_function_args(input: &str, name: &str) -> Option<Vec<String>> {
    let trimmed = input.trim();
    if !trimmed.to_ascii_lowercase().starts_with(name) {
        return None;
    }
    let open = trimmed.find('(')?;
    let close = trimmed.rfind(')')?;
    if close <= open {
        return None;
    }

    let raw_args = trimmed[(open + 1)..close].trim();
    let mut args = raw_args
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if args.is_empty() {
        return None;
    }

    // Accept CSS forms without commas, including slash-alpha variants:
    // rgb(255 0 0), rgba(255 0 0 / 50%), hsl(210 80% 40%), hsla(... / 0.6)
    if args.len() == 1 {
        let single = args.remove(0);
        if single.contains('/') {
            let slash_parts = single
                .split('/')
                .map(|part| part.trim().to_string())
                .collect::<Vec<_>>();
            if slash_parts.len() == 2 {
                let mut space_parts = slash_parts[0]
                    .split_whitespace()
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                space_parts.push(slash_parts[1].clone());
                args = space_parts;
            }
        } else {
            args = single
                .split_whitespace()
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
        }
    }

    Some(args)
}

pub(crate) fn parse_color_code(input: &str) -> Option<Hsla> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(color) = Hsla::parse_hex(trimmed) {
        return Some(color);
    }

    if let Some(args) = parse_color_function_args(trimmed, "rgb") {
        if args.len() == 3 {
            let r = parse_rgb_channel(&args[0])?;
            let g = parse_rgb_channel(&args[1])?;
            let b = parse_rgb_channel(&args[2])?;
            return Some(gpui::Rgba { r, g, b, a: 1.0 }.into());
        }
    }

    if let Some(args) = parse_color_function_args(trimmed, "rgba") {
        if args.len() == 4 {
            let r = parse_rgb_channel(&args[0])?;
            let g = parse_rgb_channel(&args[1])?;
            let b = parse_rgb_channel(&args[2])?;
            let a = parse_percent_or_unit(&args[3])?;
            return Some(gpui::Rgba { r, g, b, a }.into());
        }
    }

    if let Some(args) = parse_color_function_args(trimmed, "hsl") {
        if args.len() == 3 {
            let h = parse_hue(&args[0])?;
            let s = parse_percent_or_unit(&args[1])?;
            let l = parse_percent_or_unit(&args[2])?;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            return Some(gpui::Rgba { r, g, b, a: 1.0 }.into());
        }
    }

    if let Some(args) = parse_color_function_args(trimmed, "hsla") {
        if args.len() == 4 {
            let h = parse_hue(&args[0])?;
            let s = parse_percent_or_unit(&args[1])?;
            let l = parse_percent_or_unit(&args[2])?;
            let a = parse_percent_or_unit(&args[3])?;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            return Some(gpui::Rgba { r, g, b, a }.into());
        }
    }

    None
}
