use super::*;

pub(crate) fn color_palettes() -> Vec<Vec<Hsla>> {
    use crate::theme::DEFAULT_COLORS;
    use itertools::Itertools as _;

    macro_rules! c {
        ($color:tt) => {
            DEFAULT_COLORS
                .$color
                .keys()
                .sorted()
                .map(|k| DEFAULT_COLORS.$color.get(k).map(|c| c.hsla).unwrap())
                .collect::<Vec<_>>()
        };
    }

    vec![
        c!(stone),
        c!(red),
        c!(orange),
        c!(yellow),
        c!(green),
        c!(cyan),
        c!(blue),
        c!(purple),
        c!(pink),
    ]
}

pub(crate) fn named_color_palettes() -> Vec<(&'static str, Vec<Hsla>)> {
    let palettes = color_palettes();
    let names = [
        "Stone",
        "Red",
        "Orange",
        "Yellow",
        "Green",
        "Cyan",
        "Blue",
        "Purple",
        "Pink",
    ];

    names
        .into_iter()
        .zip(palettes)
        .collect::<Vec<(&'static str, Vec<Hsla>)>>()
}
