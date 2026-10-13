//! What a node can hold: the kinds of node, the properties each has, and the
//! layout properties its children get from it.
//!
//! Node properties are stored as sparse JSON (only values that differ from
//! the default), and this module is the single place that says what the keys
//! are, what type each one has and what it defaults to. The inspector, the
//! parameter system, the layout engine and the file parser are all driven by
//! these tables, so adding a property is one line here.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value as Json};

use super::geom::{json_array2, json_array4, json_f32, num, Color};

/// A `(value, label)` pair of a choice property.
pub type Opt = (&'static str, &'static str);

/// What a property's value looks like once stored. Parameters can drive a
/// property of the same class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueClass {
    Bool,
    Float,
    Int,
    Text,
    Color,
    Vec2,
    /// Four numbers: insets, corner radii, canvas offsets.
    Quad,
    Choice,
    Image,
}

impl ValueClass {
    pub fn label(self) -> &'static str {
        match self {
            ValueClass::Bool => "Boolean",
            ValueClass::Float => "Number",
            ValueClass::Int => "Integer",
            ValueClass::Text => "Text",
            ValueClass::Color => "Color",
            ValueClass::Vec2 => "Vector 2",
            ValueClass::Quad => "Margins",
            ValueClass::Choice => "Choice",
            ValueClass::Image => "Image",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ValueClass::Bool => "bool",
            ValueClass::Float => "number",
            ValueClass::Int => "integer",
            ValueClass::Text => "text",
            ValueClass::Color => "color",
            ValueClass::Vec2 => "vec2",
            ValueClass::Quad => "margins",
            ValueClass::Choice => "choice",
            ValueClass::Image => "image",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [
            ValueClass::Bool,
            ValueClass::Float,
            ValueClass::Int,
            ValueClass::Text,
            ValueClass::Color,
            ValueClass::Vec2,
            ValueClass::Quad,
            ValueClass::Choice,
            ValueClass::Image,
        ]
        .into_iter()
        .find(|c| c.key() == key)
    }

    /// The value a new parameter of this class starts with.
    pub fn default_value(self) -> Json {
        match self {
            ValueClass::Bool => json!(false),
            ValueClass::Float => json!(0.0),
            ValueClass::Int => json!(0),
            ValueClass::Text => json!(""),
            ValueClass::Color => json!("#ffffff"),
            ValueClass::Vec2 => json!([0.0, 0.0]),
            ValueClass::Quad => json!([0.0, 0.0, 0.0, 0.0]),
            ValueClass::Choice => json!(""),
            ValueClass::Image => json!(""),
        }
    }

    /// `value` as this class stores it, or `None` when it isn't one.
    pub fn coerce(self, value: &Json) -> Option<Json> {
        match self {
            ValueClass::Bool => value.as_bool().map(Json::Bool),
            ValueClass::Float => json_f32(value).map(num),
            ValueClass::Int => json_f32(value).map(|n| json!(n.round() as i64)),
            ValueClass::Text | ValueClass::Image | ValueClass::Choice => value.as_str().map(|s| json!(s)),
            ValueClass::Color => value.as_str().and_then(Color::from_hex).map(|c| json!(c.to_hex())),
            ValueClass::Vec2 => json_array2(value).map(|a| json!([num(a[0]), num(a[1])])),
            ValueClass::Quad => json_array4(value).map(|a| json!([num(a[0]), num(a[1]), num(a[2]), num(a[3])])),
        }
    }
}

/// How a property is edited, with the limits that go with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropType {
    Bool,
    Float { min: f32, max: f32, step: f32 },
    Int { min: i32, max: i32 },
    Text { multiline: bool },
    Color,
    Vec2,
    /// Left, top, right, bottom.
    Insets,
    /// Top-left, top-right, bottom-right, bottom-left radii.
    Corners,
    /// A canvas slot's offsets; meaning depends on the anchors.
    Offsets,
    Choice(&'static [Opt]),
    Image,
}

impl PropType {
    pub fn class(&self) -> ValueClass {
        match self {
            PropType::Bool => ValueClass::Bool,
            PropType::Float { .. } => ValueClass::Float,
            PropType::Int { .. } => ValueClass::Int,
            PropType::Text { .. } => ValueClass::Text,
            PropType::Color => ValueClass::Color,
            PropType::Vec2 => ValueClass::Vec2,
            PropType::Insets | PropType::Corners | PropType::Offsets => ValueClass::Quad,
            PropType::Choice(_) => ValueClass::Choice,
            PropType::Image => ValueClass::Image,
        }
    }

    /// `value` as this property stores it, or `None` when it is not valid
    /// for it (wrong shape, or a choice that isn't offered).
    pub fn coerce(&self, value: &Json) -> Option<Json> {
        match self {
            PropType::Choice(options) => value.as_str().filter(|s| options.iter().any(|(v, _)| v == s)).map(|s| json!(s)),
            other => other.class().coerce(value),
        }
    }
}

/// A property's default value, in a form a `static` table can hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Def {
    Bool(bool),
    F(f32),
    I(i32),
    T(&'static str),
    /// `#rrggbb[aa]`, or a choice's value.
    S(&'static str),
    V2(f32, f32),
    V4(f32, f32, f32, f32),
    /// Optional property that is off.
    Unset,
}

impl Def {
    pub fn json(&self) -> Json {
        match *self {
            Def::Bool(b) => json!(b),
            Def::F(f) => num(f),
            Def::I(i) => json!(i),
            Def::T(s) | Def::S(s) => json!(s),
            Def::V2(x, y) => json!([num(x), num(y)]),
            Def::V4(a, b, c, d) => json!([num(a), num(b), num(c), num(d)]),
            Def::Unset => Json::Null,
        }
    }
}

/// One property of a node or slot.
#[derive(Clone, Copy, Debug)]
pub struct PropDef {
    pub key: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub ty: PropType,
    pub default: Def,
    /// Has an on/off state: absent means "not set" rather than a value.
    pub optional: bool,
    pub tip: &'static str,
}

const fn p(key: &'static str, label: &'static str, category: &'static str, ty: PropType, default: Def) -> PropDef {
    PropDef { key, label, category, ty, default, optional: false, tip: "" }
}

impl PropDef {
    const fn optional(mut self) -> Self {
        self.optional = true;
        self.default = Def::Unset;
        self
    }

    const fn tip(mut self, tip: &'static str) -> Self {
        self.tip = tip;
        self
    }
}

const fn float(min: f32, max: f32, step: f32) -> PropType {
    PropType::Float { min, max, step }
}

const ANY: PropType = float(-100_000.0, 100_000.0, 1.0);
const SIZE: PropType = float(0.0, 100_000.0, 1.0);
const UNIT: PropType = float(0.0, 1.0, 0.01);

// ── Choices ────────────────────────────────────────────────────────────────

pub const VISIBILITY: &[Opt] = &[
    ("visible", "Visible"),
    ("hidden", "Hidden (keeps its space)"),
    ("collapsed", "Collapsed (takes no space)"),
    ("hit_test_invisible", "Visible, not clickable"),
];
pub const H_ALIGN: &[Opt] = &[("fill", "Fill"), ("left", "Left"), ("center", "Center"), ("right", "Right")];
pub const V_ALIGN: &[Opt] = &[("fill", "Fill"), ("top", "Top"), ("center", "Center"), ("bottom", "Bottom")];
pub const ORIENTATION: &[Opt] = &[("horizontal", "Horizontal"), ("vertical", "Vertical")];
pub const WEIGHT: &[Opt] = &[("regular", "Regular"), ("medium", "Medium"), ("semibold", "Semibold"), ("bold", "Bold")];
pub const JUSTIFY: &[Opt] = &[("left", "Left"), ("center", "Center"), ("right", "Right")];
pub const IMAGE_STRETCH: &[Opt] =
    &[("fill", "Fill"), ("uniform", "Fit"), ("uniform_fill", "Cover"), ("none", "Natural size")];
pub const BOX_STRETCH: &[Opt] = &[
    ("contain", "Fit inside"),
    ("cover", "Cover"),
    ("fill", "Stretch"),
    ("scale_down", "Shrink only"),
    ("user", "Custom scale"),
];
pub const SIZE_RULE: &[Opt] = &[("auto", "Auto"), ("fill", "Fill")];

// ── Kinds ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Canvas,
    VerticalBox,
    HorizontalBox,
    Overlay,
    Grid,
    WrapBox,
    ScrollBox,
    SizeBox,
    ScaleBox,
    Border,
    Text,
    Image,
    Button,
    CheckBox,
    Slider,
    ProgressBar,
    TextInput,
    ComboBox,
    Spacer,
    /// A hole in a widget that whoever instances it can fill.
    NamedSlot,
    /// Another widget, by id.
    Instance,
    /// A kind this version doesn't know (the file came from a newer one).
    /// Kept so saving doesn't lose it.
    Unknown,
}

impl Kind {
    pub const ALL: [Kind; 21] = [
        Kind::Canvas,
        Kind::VerticalBox,
        Kind::HorizontalBox,
        Kind::Overlay,
        Kind::Grid,
        Kind::WrapBox,
        Kind::ScrollBox,
        Kind::SizeBox,
        Kind::ScaleBox,
        Kind::Border,
        Kind::Text,
        Kind::Image,
        Kind::Button,
        Kind::CheckBox,
        Kind::Slider,
        Kind::ProgressBar,
        Kind::TextInput,
        Kind::ComboBox,
        Kind::Spacer,
        Kind::NamedSlot,
        Kind::Instance,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Canvas => "canvas",
            Kind::VerticalBox => "vertical_box",
            Kind::HorizontalBox => "horizontal_box",
            Kind::Overlay => "overlay",
            Kind::Grid => "grid",
            Kind::WrapBox => "wrap_box",
            Kind::ScrollBox => "scroll_box",
            Kind::SizeBox => "size_box",
            Kind::ScaleBox => "scale_box",
            Kind::Border => "border",
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Button => "button",
            Kind::CheckBox => "check_box",
            Kind::Slider => "slider",
            Kind::ProgressBar => "progress_bar",
            Kind::TextInput => "text_input",
            Kind::ComboBox => "combo_box",
            Kind::Spacer => "spacer",
            Kind::NamedSlot => "named_slot",
            Kind::Instance => "widget",
            Kind::Unknown => "unknown",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }

    pub fn info(self) -> &'static KindInfo {
        &tables().kinds[self as usize]
    }
}

/// How many children a node holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Children {
    None,
    One,
    Many,
    /// An instance: children each fill one of the instanced widget's named slots.
    Slots,
}

/// Which layout properties a parent gives its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SlotKind {
    None,
    Canvas,
    Box,
    Overlay,
    Grid,
    Wrap,
    Single,
}

impl SlotKind {
    pub fn key(self) -> &'static str {
        match self {
            SlotKind::None => "none",
            SlotKind::Canvas => "canvas",
            SlotKind::Box => "box",
            SlotKind::Overlay => "overlay",
            SlotKind::Grid => "grid",
            SlotKind::Wrap => "wrap",
            SlotKind::Single => "single",
        }
    }

    pub fn from_key(key: &str) -> Option<SlotKind> {
        [SlotKind::None, SlotKind::Canvas, SlotKind::Box, SlotKind::Overlay, SlotKind::Grid, SlotKind::Wrap, SlotKind::Single]
            .into_iter()
            .find(|s| s.key() == key)
    }

    pub fn props(self) -> &'static [PropDef] {
        match self {
            SlotKind::None => &[],
            SlotKind::Canvas => CANVAS_SLOT,
            SlotKind::Box => BOX_SLOT,
            SlotKind::Overlay => OVERLAY_SLOT,
            SlotKind::Grid => GRID_SLOT,
            SlotKind::Wrap => WRAP_SLOT,
            SlotKind::Single => SINGLE_SLOT,
        }
    }

    pub fn def(self, key: &str) -> Option<&'static PropDef> {
        self.props().iter().find(|d| d.key == key)
    }
}

/// Everything the editor and layout need to know about a kind of node.
pub struct KindInfo {
    pub kind: Kind,
    pub label: &'static str,
    /// Palette group.
    pub category: &'static str,
    pub description: &'static str,
    pub children: Children,
    /// What its children's slots look like.
    pub slot: SlotKind,
    /// Common properties followed by the kind's own.
    pub props: Vec<PropDef>,
    index: HashMap<&'static str, usize>,
}

impl KindInfo {
    pub fn def(&self, key: &str) -> Option<&PropDef> {
        self.index.get(key).map(|&i| &self.props[i])
    }

    /// The default value of `key`: `Null` for optional properties and unknown keys.
    pub fn default_of(&self, key: &str) -> Json {
        self.def(key).map_or(Json::Null, |d| d.default.json())
    }

    pub fn can_have_children(&self) -> bool {
        self.children != Children::None
    }
}

struct Tables {
    kinds: Vec<KindInfo>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut kinds = Vec::new();
        for kind in Kind::ALL.into_iter().chain([Kind::Unknown]) {
            let (label, category, description, children, slot, own) = describe(kind);
            let mut props: Vec<PropDef> = COMMON.to_vec();
            props.extend_from_slice(own);
            let index = props.iter().enumerate().map(|(i, d)| (d.key, i)).collect();
            kinds.push(KindInfo { kind, label, category, description, children, slot, props, index });
        }
        // `Kind as usize` indexes this table.
        debug_assert!(kinds.iter().enumerate().all(|(i, k)| k.kind as usize == i));
        Tables { kinds }
    })
}

type Description = (&'static str, &'static str, &'static str, Children, SlotKind, &'static [PropDef]);

fn describe(kind: Kind) -> Description {
    use Children::*;
    match kind {
        Kind::Canvas => ("Canvas Panel", "Layout", "Places children freely, pinned to the panel with anchors.", Many, SlotKind::Canvas, &[]),
        Kind::VerticalBox => ("Vertical Box", "Layout", "Stacks children top to bottom.", Many, SlotKind::Box, &[]),
        Kind::HorizontalBox => ("Horizontal Box", "Layout", "Lines children up left to right.", Many, SlotKind::Box, &[]),
        Kind::Overlay => ("Overlay", "Layout", "Stacks children on top of each other.", Many, SlotKind::Overlay, &[]),
        Kind::Grid => ("Grid Panel", "Layout", "Arranges children in rows and columns.", Many, SlotKind::Grid, GRID),
        Kind::WrapBox => ("Wrap Box", "Layout", "Flows children and wraps to a new line when full.", Many, SlotKind::Wrap, WRAP),
        Kind::ScrollBox => ("Scroll Box", "Layout", "A scrollable stack of children.", Many, SlotKind::Box, SCROLL),
        Kind::SizeBox => ("Size Box", "Layout", "Forces or limits the size of its child.", One, SlotKind::Single, SIZE_BOX),
        Kind::ScaleBox => ("Scale Box", "Layout", "Scales its child to fit the space it has.", One, SlotKind::Single, SCALE_BOX),
        Kind::Border => ("Border", "Layout", "A background and outline around one child.", One, SlotKind::Single, BORDER),
        Kind::Text => ("Text", "Common", "A run of text.", None, SlotKind::None, TEXT),
        Kind::Image => ("Image", "Common", "A picture or a flat color.", None, SlotKind::None, IMAGE),
        Kind::Button => ("Button", "Common", "Something to click; holds any content.", One, SlotKind::Single, BUTTON),
        Kind::CheckBox => ("Check Box", "Input", "A labelled on/off switch.", None, SlotKind::None, CHECK_BOX),
        Kind::Slider => ("Slider", "Input", "Picks a number by dragging.", None, SlotKind::None, SLIDER),
        Kind::ProgressBar => ("Progress Bar", "Display", "Shows how far along something is.", None, SlotKind::None, PROGRESS),
        Kind::TextInput => ("Text Input", "Input", "A box the player types into.", None, SlotKind::None, TEXT_INPUT),
        Kind::ComboBox => ("Combo Box", "Input", "Picks one of several options.", None, SlotKind::None, COMBO),
        Kind::Spacer => ("Spacer", "Common", "Empty space of a fixed size.", None, SlotKind::None, SPACER),
        Kind::NamedSlot => ("Named Slot", "Special", "A hole that whoever uses this widget can fill. Its name is the node's name.", None, SlotKind::None, &[]),
        Kind::Instance => ("Widget", "Special", "Another widget, referenced by id.", Slots, SlotKind::Single, &[]),
        Kind::Unknown => ("Unknown", "Special", "Made by a newer version; kept as it is.", None, SlotKind::None, &[]),
    }
}

// ── Property tables ────────────────────────────────────────────────────────

/// On every node.
pub const COMMON: &[PropDef] = &[
    p("visibility", "Visibility", "Behavior", PropType::Choice(VISIBILITY), Def::S("visible")),
    p("enabled", "Enabled", "Behavior", PropType::Bool, Def::Bool(true)).tip("Disabled widgets draw dimmed and ignore input"),
    p("tooltip", "Tooltip", "Behavior", PropType::Text { multiline: false }, Def::T("")),
    p("opacity", "Opacity", "Appearance", UNIT, Def::F(1.0)),
    p("clip", "Clip to bounds", "Appearance", PropType::Bool, Def::Bool(false)),
    p("translate", "Translation", "Render transform", PropType::Vec2, Def::V2(0.0, 0.0)).tip("Moves the widget when drawn without changing layout"),
    p("scale", "Scale", "Render transform", PropType::Vec2, Def::V2(1.0, 1.0)),
    p("pivot", "Pivot", "Render transform", PropType::Vec2, Def::V2(0.5, 0.5)).tip("Where scaling is centred, 0 to 1 across the widget"),
];

const GRID: &[PropDef] = &[
    p("column_sizes", "Column sizes", "Grid", PropType::Text { multiline: false }, Def::T("1, 1"))
        .tip("One entry per column: a number (share of the leftover space), `auto`, or a pixel size like `120px`"),
    p("row_sizes", "Row sizes", "Grid", PropType::Text { multiline: false }, Def::T("auto")).tip("Same format; the last entry repeats for more rows"),
    p("gap", "Gap", "Grid", PropType::Vec2, Def::V2(0.0, 0.0)),
];

const WRAP: &[PropDef] = &[
    p("orientation", "Orientation", "Wrap", PropType::Choice(ORIENTATION), Def::S("horizontal")),
    p("wrap_size", "Wrap at", "Wrap", SIZE, Def::F(0.0)).optional().tip("Wrap at this length instead of the available space"),
    p("gap", "Gap", "Wrap", PropType::Vec2, Def::V2(0.0, 0.0)),
];

const SCROLL: &[PropDef] = &[
    p("orientation", "Orientation", "Scroll", PropType::Choice(ORIENTATION), Def::S("vertical")),
    p("scroll_offset", "Scroll offset", "Scroll", SIZE, Def::F(0.0)).tip("How far the content starts scrolled"),
    p("scrollbar_visible", "Show scrollbar", "Scroll", PropType::Bool, Def::Bool(true)),
    p("scrollbar_color", "Scrollbar color", "Scroll", PropType::Color, Def::S("#ffffff55")),
];

const SIZE_BOX: &[PropDef] = &[
    p("width", "Width", "Size", SIZE, Def::F(0.0)).optional().tip("Forces the child to this width"),
    p("height", "Height", "Size", SIZE, Def::F(0.0)).optional().tip("Forces the child to this height"),
    p("min_width", "Min width", "Size", SIZE, Def::F(0.0)).optional(),
    p("min_height", "Min height", "Size", SIZE, Def::F(0.0)).optional(),
    p("max_width", "Max width", "Size", SIZE, Def::F(0.0)).optional(),
    p("max_height", "Max height", "Size", SIZE, Def::F(0.0)).optional(),
];

const SCALE_BOX: &[PropDef] = &[
    p("stretch", "Stretch", "Scale", PropType::Choice(BOX_STRETCH), Def::S("contain")),
    p("user_scale", "Scale", "Scale", float(0.01, 100.0, 0.05), Def::F(1.0)).tip("Used by the custom-scale stretch mode"),
];

const BORDER: &[PropDef] = &[
    p("background", "Background", "Brush", PropType::Color, Def::S("#00000000")),
    p("gradient_to", "Gradient to", "Brush", PropType::Color, Def::S("#00000000")).optional().tip("Fades the background into this color"),
    p("gradient_angle", "Gradient angle", "Brush", float(0.0, 360.0, 5.0), Def::F(180.0)),
    p("border_color", "Outline color", "Brush", PropType::Color, Def::S("#ffffff33")),
    p("border_width", "Outline width", "Brush", float(0.0, 64.0, 0.5), Def::F(0.0)),
    p("corner_radius", "Corner radius", "Brush", PropType::Corners, Def::V4(0.0, 0.0, 0.0, 0.0)),
    p("shadow_color", "Shadow color", "Shadow", PropType::Color, Def::S("#00000000")),
    p("shadow_blur", "Shadow blur", "Shadow", float(0.0, 128.0, 1.0), Def::F(0.0)),
    p("shadow_offset", "Shadow offset", "Shadow", PropType::Vec2, Def::V2(0.0, 4.0)),
];

const TEXT: &[PropDef] = &[
    p("text", "Text", "Content", PropType::Text { multiline: true }, Def::T("Text")),
    p("font_size", "Size", "Font", float(4.0, 512.0, 1.0), Def::F(16.0)),
    p("weight", "Weight", "Font", PropType::Choice(WEIGHT), Def::S("regular")),
    p("italic", "Italic", "Font", PropType::Bool, Def::Bool(false)),
    p("color", "Color", "Font", PropType::Color, Def::S("#ffffff")),
    p("justify", "Justify", "Font", PropType::Choice(JUSTIFY), Def::S("left")),
    p("line_height", "Line height", "Font", float(0.5, 4.0, 0.05), Def::F(1.25)),
    p("wrap", "Wrap text", "Layout", PropType::Bool, Def::Bool(false)),
    p("shadow_color", "Shadow color", "Shadow", PropType::Color, Def::S("#00000000")),
    p("shadow_offset", "Shadow offset", "Shadow", PropType::Vec2, Def::V2(1.0, 1.0)),
];

const IMAGE: &[PropDef] = &[
    p("source", "Image", "Content", PropType::Image, Def::T("")),
    p("tint", "Tint", "Content", PropType::Color, Def::S("#ffffff")),
    p("stretch", "Stretch", "Content", PropType::Choice(IMAGE_STRETCH), Def::S("fill")),
    p("image_size", "Natural size", "Content", PropType::Vec2, Def::V2(64.0, 64.0)).tip("Used for layout until the image is available"),
    p("corner_radius", "Corner radius", "Content", PropType::Corners, Def::V4(0.0, 0.0, 0.0, 0.0)),
];

const BUTTON: &[PropDef] = &[
    p("action", "Action", "Behavior", PropType::Text { multiline: false }, Def::T("")).tip("The name of the event this button raises when clicked"),
    p("normal_color", "Normal", "Style", PropType::Color, Def::S("#3a3f4b")),
    p("hover_color", "Hovered", "Style", PropType::Color, Def::S("#4a5060")),
    p("pressed_color", "Pressed", "Style", PropType::Color, Def::S("#2c303a")),
    p("disabled_color", "Disabled", "Style", PropType::Color, Def::S("#2a2d34")),
    p("border_color", "Outline color", "Style", PropType::Color, Def::S("#ffffff1f")),
    p("border_width", "Outline width", "Style", float(0.0, 64.0, 0.5), Def::F(1.0)),
    p("corner_radius", "Corner radius", "Style", PropType::Corners, Def::V4(6.0, 6.0, 6.0, 6.0)),
    p("padding", "Content padding", "Style", PropType::Insets, Def::V4(14.0, 8.0, 14.0, 8.0)),
];

const CHECK_BOX: &[PropDef] = &[
    p("label", "Label", "Content", PropType::Text { multiline: false }, Def::T("Check box")),
    p("checked", "Checked", "Content", PropType::Bool, Def::Bool(false)),
    p("action", "Action", "Behavior", PropType::Text { multiline: false }, Def::T("")),
    p("font_size", "Font size", "Style", float(4.0, 512.0, 1.0), Def::F(15.0)),
    p("text_color", "Text color", "Style", PropType::Color, Def::S("#e6e8ee")),
    p("box_color", "Box color", "Style", PropType::Color, Def::S("#2a2e38")),
    p("check_color", "Check color", "Style", PropType::Color, Def::S("#4a9eff")),
    p("box_size", "Box size", "Style", float(8.0, 128.0, 1.0), Def::F(18.0)),
    p("spacing", "Spacing", "Style", float(0.0, 64.0, 1.0), Def::F(8.0)),
];

const SLIDER: &[PropDef] = &[
    p("value", "Value", "Content", float(-100_000.0, 100_000.0, 0.01), Def::F(0.5)),
    p("min", "Minimum", "Content", ANY, Def::F(0.0)),
    p("max", "Maximum", "Content", ANY, Def::F(1.0)),
    p("orientation", "Orientation", "Content", PropType::Choice(ORIENTATION), Def::S("horizontal")),
    p("action", "Action", "Behavior", PropType::Text { multiline: false }, Def::T("")),
    p("track_color", "Track", "Style", PropType::Color, Def::S("#2a2e38")),
    p("fill_color", "Fill", "Style", PropType::Color, Def::S("#4a9eff")),
    p("handle_color", "Handle", "Style", PropType::Color, Def::S("#f2f4f8")),
    p("thickness", "Track thickness", "Style", float(1.0, 64.0, 1.0), Def::F(6.0)),
    p("handle_size", "Handle size", "Style", float(4.0, 128.0, 1.0), Def::F(16.0)),
];

const PROGRESS: &[PropDef] = &[
    p("percent", "Percent", "Content", UNIT, Def::F(0.5)),
    p("indeterminate", "Indeterminate", "Content", PropType::Bool, Def::Bool(false)).tip("Shows a moving bar instead of a fraction"),
    p("orientation", "Orientation", "Content", PropType::Choice(ORIENTATION), Def::S("horizontal")),
    p("fill_color", "Fill", "Style", PropType::Color, Def::S("#4a9eff")),
    p("back_color", "Background", "Style", PropType::Color, Def::S("#2a2e38")),
    p("corner_radius", "Corner radius", "Style", float(0.0, 128.0, 1.0), Def::F(4.0)),
    p("thickness", "Thickness", "Style", float(1.0, 256.0, 1.0), Def::F(12.0)),
];

const TEXT_INPUT: &[PropDef] = &[
    p("text", "Text", "Content", PropType::Text { multiline: false }, Def::T("")),
    p("hint", "Hint", "Content", PropType::Text { multiline: false }, Def::T("Type here")),
    p("password", "Password", "Content", PropType::Bool, Def::Bool(false)),
    p("multiline", "Multiline", "Content", PropType::Bool, Def::Bool(false)),
    p("action", "Action", "Behavior", PropType::Text { multiline: false }, Def::T("")),
    p("font_size", "Font size", "Style", float(4.0, 512.0, 1.0), Def::F(15.0)),
    p("color", "Text color", "Style", PropType::Color, Def::S("#f2f4f8")),
    p("hint_color", "Hint color", "Style", PropType::Color, Def::S("#8a90a0")),
    p("background", "Background", "Style", PropType::Color, Def::S("#20232b")),
    p("border_color", "Outline color", "Style", PropType::Color, Def::S("#ffffff2a")),
    p("border_width", "Outline width", "Style", float(0.0, 64.0, 0.5), Def::F(1.0)),
    p("corner_radius", "Corner radius", "Style", PropType::Corners, Def::V4(5.0, 5.0, 5.0, 5.0)),
    p("padding", "Padding", "Style", PropType::Insets, Def::V4(10.0, 7.0, 10.0, 7.0)),
];

const COMBO: &[PropDef] = &[
    p("options", "Options", "Content", PropType::Text { multiline: true }, Def::T("Option 1\nOption 2\nOption 3")).tip("One option per line"),
    p("selected", "Selected", "Content", PropType::Int { min: -1, max: 10_000 }, Def::I(0)),
    p("action", "Action", "Behavior", PropType::Text { multiline: false }, Def::T("")),
    p("font_size", "Font size", "Style", float(4.0, 512.0, 1.0), Def::F(15.0)),
    p("color", "Text color", "Style", PropType::Color, Def::S("#f2f4f8")),
    p("background", "Background", "Style", PropType::Color, Def::S("#20232b")),
    p("border_color", "Outline color", "Style", PropType::Color, Def::S("#ffffff2a")),
    p("border_width", "Outline width", "Style", float(0.0, 64.0, 0.5), Def::F(1.0)),
    p("corner_radius", "Corner radius", "Style", PropType::Corners, Def::V4(5.0, 5.0, 5.0, 5.0)),
    p("padding", "Padding", "Style", PropType::Insets, Def::V4(10.0, 7.0, 10.0, 7.0)),
];

const SPACER: &[PropDef] = &[p("size", "Size", "Layout", PropType::Vec2, Def::V2(16.0, 16.0))];

// ── Slot tables ────────────────────────────────────────────────────────────

const CANVAS_SLOT: &[PropDef] = &[
    p("anchor_min", "Anchor min", "Slot", PropType::Vec2, Def::V2(0.0, 0.0)),
    p("anchor_max", "Anchor max", "Slot", PropType::Vec2, Def::V2(0.0, 0.0)),
    p("offsets", "Offsets", "Slot", PropType::Offsets, Def::V4(0.0, 0.0, 100.0, 30.0)),
    p("alignment", "Alignment", "Slot", PropType::Vec2, Def::V2(0.0, 0.0)).tip("The point of the widget that sits on the position, 0 to 1"),
    p("auto_size", "Size to content", "Slot", PropType::Bool, Def::Bool(false)),
    p("z_order", "Z order", "Slot", PropType::Int { min: -1000, max: 1000 }, Def::I(0)),
];

const BOX_SLOT: &[PropDef] = &[
    p("size_rule", "Size", "Slot", PropType::Choice(SIZE_RULE), Def::S("auto")),
    p("fill_weight", "Fill weight", "Slot", float(0.0, 1000.0, 0.1), Def::F(1.0)),
    p("padding", "Padding", "Slot", PropType::Insets, Def::V4(0.0, 0.0, 0.0, 0.0)),
    p("h_align", "Horizontal align", "Slot", PropType::Choice(H_ALIGN), Def::S("fill")),
    p("v_align", "Vertical align", "Slot", PropType::Choice(V_ALIGN), Def::S("fill")),
];

const OVERLAY_SLOT: &[PropDef] = &[
    p("padding", "Padding", "Slot", PropType::Insets, Def::V4(0.0, 0.0, 0.0, 0.0)),
    p("h_align", "Horizontal align", "Slot", PropType::Choice(H_ALIGN), Def::S("fill")),
    p("v_align", "Vertical align", "Slot", PropType::Choice(V_ALIGN), Def::S("fill")),
];

const SINGLE_SLOT: &[PropDef] = OVERLAY_SLOT;

const GRID_SLOT: &[PropDef] = &[
    p("column", "Column", "Slot", PropType::Int { min: 0, max: 255 }, Def::I(0)),
    p("row", "Row", "Slot", PropType::Int { min: 0, max: 255 }, Def::I(0)),
    p("column_span", "Column span", "Slot", PropType::Int { min: 1, max: 255 }, Def::I(1)),
    p("row_span", "Row span", "Slot", PropType::Int { min: 1, max: 255 }, Def::I(1)),
    p("padding", "Padding", "Slot", PropType::Insets, Def::V4(0.0, 0.0, 0.0, 0.0)),
    p("h_align", "Horizontal align", "Slot", PropType::Choice(H_ALIGN), Def::S("fill")),
    p("v_align", "Vertical align", "Slot", PropType::Choice(V_ALIGN), Def::S("fill")),
];

const WRAP_SLOT: &[PropDef] = &[
    p("padding", "Padding", "Slot", PropType::Insets, Def::V4(0.0, 0.0, 0.0, 0.0)),
    p("h_align", "Horizontal align", "Slot", PropType::Choice(H_ALIGN), Def::S("fill")),
    p("v_align", "Vertical align", "Slot", PropType::Choice(V_ALIGN), Def::S("fill")),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_is_valid_for_its_own_type() {
        for kind in Kind::ALL.into_iter().chain([Kind::Unknown]) {
            for def in &kind.info().props {
                if def.optional {
                    assert_eq!(def.default, Def::Unset, "{kind:?}.{}", def.key);
                    continue;
                }
                assert!(def.ty.coerce(&def.default.json()).is_some(), "{kind:?}.{} default is not a valid {:?}", def.key, def.ty);
            }
        }
        for slot in [SlotKind::Canvas, SlotKind::Box, SlotKind::Overlay, SlotKind::Grid, SlotKind::Wrap, SlotKind::Single] {
            for def in slot.props() {
                assert!(def.ty.coerce(&def.default.json()).is_some(), "{slot:?}.{} default", def.key);
            }
        }
    }

    #[test]
    fn keys_are_unique_within_a_kind_and_the_table_matches_the_enum() {
        for kind in Kind::ALL.into_iter().chain([Kind::Unknown]) {
            let info = kind.info();
            assert_eq!(info.kind, kind);
            let mut keys: Vec<_> = info.props.iter().map(|d| d.key).collect();
            keys.sort_unstable();
            let n = keys.len();
            keys.dedup();
            assert_eq!(keys.len(), n, "{kind:?} repeats a property key");
        }
        for kind in Kind::ALL {
            assert_eq!(Kind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(Kind::from_key("flux_capacitor"), None);
    }

    #[test]
    fn choices_reject_values_that_are_not_offered() {
        let ty = PropType::Choice(H_ALIGN);
        assert!(ty.coerce(&json!("left")).is_some());
        assert!(ty.coerce(&json!("diagonal")).is_none());
        assert!(PropType::Bool.coerce(&json!("yes")).is_none());
        assert_eq!(ValueClass::Int.coerce(&json!(2.6)), Some(json!(3)));
        assert_eq!(ValueClass::Color.coerce(&json!("#FFF")), Some(json!("#ffffff")));
        assert!(ValueClass::Vec2.coerce(&json!([1.0])).is_none());
    }

    #[test]
    fn containers_declare_the_slots_their_children_get() {
        assert_eq!(Kind::Canvas.info().slot, SlotKind::Canvas);
        assert_eq!(Kind::Text.info().children, Children::None);
        assert_eq!(Kind::Button.info().children, Children::One);
        assert_eq!(Kind::Instance.info().children, Children::Slots);
        assert!(Kind::VerticalBox.info().def("visibility").is_some(), "common properties are on every kind");
    }
}
