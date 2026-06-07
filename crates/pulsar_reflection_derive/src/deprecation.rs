/// Emits a rustc-style colourised deprecation warning to stderr.
///
/// Cargo pipes proc-macro stderr through its own buffer before writing to the terminal,
/// so `is_terminal()` always returns false from inside a proc macro — meaning any
/// TTY-gated approach silently strips all colours. Instead we emit ANSI codes
/// unconditionally and honour only `NO_COLOR` / `TERM=dumb` to let users opt out.
pub fn emit_deprecation_warning(
    prim_span: Option<proc_macro2::Span>,
    struct_span: Option<proc_macro2::Span>,
) {
    let no_color = std::env::var("NO_COLOR").is_ok()
        || std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);

    macro_rules! c {
        ($code:literal, $text:expr) => {
            if no_color {
                $text.to_string()
            } else {
                format!("\x1b[{}m{}\x1b[0m", $code, $text)
            }
        };
    }

    let fmt_loc = |span: proc_macro2::Span| -> String {
        let start = span.start();
        let file = span.file();
        format!("{}:{}:{}", file, start.line, start.column + 1)
    };

    let warn = c!("1;33", "warning");
    let arrow = c!("1;34", "-->");
    let bar = c!("1;34", "|");
    let note_tag = c!("1;36", "note");
    let help_tag = c!("1;32", "help");
    let attr = c!("1;37", "#[pulsar_type]");
    let dim_dash = c!("2", "─────────────────────────────");
    let ignored = c!("33", "silently ignored");
    let removed = c!("31", "has been removed");
    let unified = c!("32", "single unified registration path");

    let header_span = prim_span.or(struct_span).unwrap();
    let loc = fmt_loc(header_span);
    let loc_colored = c!("2;37", loc);

    eprintln!();
    eprintln!("{}: deprecated argument(s) passed to {}", warn, attr);
    eprintln!(" {} {}", arrow, loc_colored);
    eprintln!(" {} {}", bar, dim_dash);

    if let Some(span) = prim_span {
        let arg = c!("1;37", "primitive");
        let in_ctx = c!("2", "#[pulsar_type(");
        let suffix = c!("2", ", ...)]");
        let sloc = c!("2;37", fmt_loc(span));
        eprintln!(" {} {}:{}", bar, c!("2;37", "at"), sloc);
        eprintln!(
            " {} in {}{}{}: argument is {}",
            bar, in_ctx, arg, suffix, ignored
        );
    }
    if let Some(span) = struct_span {
        let arg = c!("1;37", "structure");
        let in_ctx = c!("2", "#[pulsar_type(");
        let suffix = c!("2", " = ..., ...)]");
        let sloc = c!("2;37", fmt_loc(span));
        eprintln!(" {} {}:{}", bar, c!("2;37", "at"), sloc);
        eprintln!(
            " {} in {}{}{}: argument is {}",
            bar, in_ctx, arg, suffix, ignored
        );
    }

    eprintln!(" {}", bar);
    let prim_token = c!("1;37", "primitive");
    let struct_token = c!("1;37", "structure");
    eprintln!(
        " {} {}: the {} / {} type distinction {}",
        bar, note_tag, prim_token, struct_token, removed
    );
    eprintln!(" {}       all types now follow a {}", bar, unified);

    eprintln!(" {}", bar);
    if prim_span.is_some() {
        let before = c!("31", "#[pulsar_type(primitive, ...)]");
        let after = c!("1;32", "#[pulsar_type(...)]");
        let arr = c!("33", "→");
        eprintln!(" {} {}:  {}  {}  {}", bar, help_tag, before, arr, after);
    }
    if struct_span.is_some() {
        let before = c!("31", "#[pulsar_type(structure = ..., ...)]");
        let after = c!("1;32", "#[pulsar_type(...)]");
        let arr = c!("33", "→");
        eprintln!(" {} {}:  {}  {}  {}", bar, help_tag, before, arr, after);
    }
    eprintln!(" {} {}", bar, dim_dash);
    eprintln!();
}
