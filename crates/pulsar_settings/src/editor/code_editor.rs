use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "code_editor";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Code Editor", "Source code editing behavior and display")
        // ── Font & Display ─────────────────────────────────────────────────
        .setting("font_family",
            SchemaEntry::new("Font family for source code", "JetBrains Mono")
                .label("Code Font Family").page("Code Editor")
                .field_type(FieldType::TextInput { placeholder: Some("JetBrains Mono".into()), multiline: false }))
        .setting("font_size",
            SchemaEntry::new("Font size for source code (pt)", 14_i64)
                .label("Code Font Size").page("Code Editor")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(8, 32)))
        .setting("line_height",
            SchemaEntry::new("Line height multiplier for code lines", 1.5_f64)
                .label("Line Height").page("Code Editor")
                .field_type(FieldType::Slider { min: 1.0, max: 3.0, step: 0.05 })
                .validator(Validator::float_range(1.0, 3.0)))
        .setting("letter_spacing",
            SchemaEntry::new("Extra spacing between characters (em)", 0.0_f64)
                .label("Letter Spacing").page("Code Editor")
                .field_type(FieldType::Slider { min: -0.1, max: 0.5, step: 0.01 })
                .validator(Validator::float_range(-0.1, 0.5)))
        // ── Behavior ───────────────────────────────────────────────────────
        .setting("word_wrap",
            SchemaEntry::new("Wrap long lines at the editor boundary", false)
                .label("Word Wrap").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("word_wrap_column",
            SchemaEntry::new("Column at which to wrap lines when word wrap is 'bounded'", 120_i64)
                .label("Wrap Column").page("Code Editor")
                .field_type(FieldType::NumberInput { min: Some(40.0), max: Some(300.0), step: Some(1.0) })
                .validator(Validator::int_range(40, 300)))
        .setting("tab_size",
            SchemaEntry::new("Number of spaces per tab stop", 4_i64)
                .label("Tab Size").page("Code Editor")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(16.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 16)))
        .setting("insert_spaces",
            SchemaEntry::new("Insert spaces when pressing Tab (soft tabs)", true)
                .label("Insert Spaces").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("detect_indentation",
            SchemaEntry::new("Auto-detect indentation style from file content", true)
                .label("Detect Indentation").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("auto_save",
            SchemaEntry::new("Automatically save files when focus is lost or editor is idle", true)
                .label("Auto Save").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("auto_save_delay_ms",
            SchemaEntry::new("Delay in ms before an auto-save triggers", 1000_i64)
                .label("Auto Save Delay (ms)").page("Code Editor")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(10000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 10000)))
        .setting("format_on_save",
            SchemaEntry::new("Run the default formatter when saving a file", false)
                .label("Format on Save").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("format_on_paste",
            SchemaEntry::new("Auto-format code pasted from clipboard", false)
                .label("Format on Paste").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("trim_trailing_whitespace",
            SchemaEntry::new("Remove trailing whitespace from lines on save", true)
                .label("Trim Trailing Whitespace").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("insert_final_newline",
            SchemaEntry::new("Ensure files end with a single newline on save", true)
                .label("Insert Final Newline").page("Code Editor")
                .field_type(FieldType::Checkbox))
        // ── Gutters & Indicators ───────────────────────────────────────────
        .setting("show_line_numbers",
            SchemaEntry::new("Show line numbers in the gutter", true)
                .label("Show Line Numbers").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("line_numbers_style",
            SchemaEntry::new("Style of line numbers shown in gutter", "absolute")
                .label("Line Number Style").page("Code Editor")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Absolute", "absolute"),
                    DropdownOption::new("Relative", "relative"),
                    DropdownOption::new("Relative + Current", "relative_current"),
                ]})
                .validator(Validator::string_one_of(["absolute", "relative", "relative_current"])))
        .setting("show_indent_guides",
            SchemaEntry::new("Render vertical indent guide lines", true)
                .label("Indent Guides").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("show_folding_controls",
            SchemaEntry::new("Show code folding controls in the gutter", true)
                .label("Folding Controls").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("show_minimap",
            SchemaEntry::new("Display a minimap overview of the file", false)
                .label("Show Minimap").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("minimap_side",
            SchemaEntry::new("Which side the minimap appears on", "right")
                .label("Minimap Side").page("Code Editor")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Left", "left"),
                    DropdownOption::new("Right", "right"),
                ]})
                .validator(Validator::string_one_of(["left", "right"])))
        .setting("highlight_active_line",
            SchemaEntry::new("Highlight the line the cursor is on", true)
                .label("Highlight Active Line").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("highlight_matching_brackets",
            SchemaEntry::new("Highlight the matching bracket under the cursor", true)
                .label("Highlight Brackets").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("render_whitespace",
            SchemaEntry::new("Render whitespace characters visually", "selection")
                .label("Render Whitespace").page("Code Editor")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Boundary", "boundary"),
                    DropdownOption::new("Selection", "selection"),
                    DropdownOption::new("Trailing", "trailing"),
                    DropdownOption::new("All", "all"),
                ]})
                .validator(Validator::string_one_of(["none", "boundary", "selection", "trailing", "all"])))
        .setting("rulers",
            SchemaEntry::new("Comma-separated column positions to draw vertical rulers", "")
                .label("Column Rulers").page("Code Editor")
                .field_type(FieldType::TextInput { placeholder: Some("80, 120".into()), multiline: false }))
        // ── IntelliSense & Completions ─────────────────────────────────────
        .setting("suggest_on_trigger_characters",
            SchemaEntry::new("Show completion suggestions when trigger characters are typed", true)
                .label("Suggest on Trigger").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("accept_suggestion_on_enter",
            SchemaEntry::new("Accept the highlighted completion on Enter", true)
                .label("Accept on Enter").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("accept_suggestion_on_tab",
            SchemaEntry::new("Accept the highlighted completion on Tab", true)
                .label("Accept on Tab").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("snippet_suggestions",
            SchemaEntry::new("How to handle snippet completions in the suggestion list", "inline")
                .label("Snippet Suggestions").page("Code Editor")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Top", "top"),
                    DropdownOption::new("Bottom", "bottom"),
                    DropdownOption::new("Inline", "inline"),
                ]})
                .validator(Validator::string_one_of(["none", "top", "bottom", "inline"])))
        .setting("inline_hints",
            SchemaEntry::new("Show inline type and parameter hints from the language server", true)
                .label("Inline Hints").page("Code Editor")
                .field_type(FieldType::Checkbox))
        // ── Search ─────────────────────────────────────────────────────────
        .setting("search_case_sensitive",
            SchemaEntry::new("Default to case-sensitive search", false)
                .label("Case-Sensitive Search").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("search_whole_word",
            SchemaEntry::new("Default to whole-word matching in search", false)
                .label("Whole Word Search").page("Code Editor")
                .field_type(FieldType::Checkbox))
        .setting("search_regex",
            SchemaEntry::new("Default to regex mode in search", false)
                .label("Regex Search").page("Code Editor")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
