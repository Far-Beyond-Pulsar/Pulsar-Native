use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// Primitive Rust types supported by the type system
pub const PRIMITIVES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize",
    "u8", "u16", "u32", "u64", "u128", "usize",
    "f32", "f64",
    "bool", "char", "str", "String",
    "()", // unit type
];

/// Type constructors for building complex types in aliases
pub const CONSTRUCTORS: &[&str] = &[
    "Box", "Arc", "Rc", "Option", "Result", "Vec", "HashMap", "HashSet",
    "Cow", "Pin", "PhantomData", "Mutex", "RwLock", "RefCell", "Cell",
];

/// Kind of type definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TypeKind {
    Struct,
    Enum,
    Trait,
    Alias,
}

impl TypeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TypeKind::Struct => "struct",
            TypeKind::Enum => "enum",
            TypeKind::Trait => "trait",
            TypeKind::Alias => "alias",
        }
    }

    pub fn plural(&self) -> &'static str {
        match self {
            TypeKind::Struct => "structs",
            TypeKind::Enum => "enums",
            TypeKind::Trait => "traits",
            TypeKind::Alias => "aliases",
        }
    }
}

/// Type reference that can be used in fields, parameters, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum TypeRef {
    /// Primitive type like i32, String, etc.
    Primitive { name: String },

    /// Path to a type, e.g., std::time::Duration
    Path { path: String },

    /// Reference to a type alias
    AliasRef { alias: String },
}

impl TypeRef {
    pub fn primitive(name: impl Into<String>) -> Self {
        TypeRef::Primitive { name: name.into() }
    }

    pub fn path(path: impl Into<String>) -> Self {
        TypeRef::Path { path: path.into() }
    }

    pub fn alias(alias: impl Into<String>) -> Self {
        TypeRef::AliasRef { alias: alias.into() }
    }
}

/// AST node for type alias definitions (supports complex nested types)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "nodeKind")]
pub enum TypeAstNode {
    /// Primitive type
    Primitive { name: String },

    /// Path type
    Path { path: String },

    /// Reference to another alias
    AliasRef { alias: String },

    /// Constructor like Box<T>, Arc<T>, Result<T, E>, etc.
    Constructor {
        name: String,
        params: Vec<TypeAstNode>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lifetimes: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        const_generics: Vec<String>,
    },

    /// Tuple type (T1, T2, ...)
    Tuple { elements: Vec<TypeAstNode> },

    /// Function pointer type
    FnPointer {
        params: Vec<TypeAstNode>,
        return_type: Box<TypeAstNode>,
    },
}

/// Visibility modifier for types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
    Crate,
    Super,
}

impl Default for Visibility {
    fn default() -> Self {
        Visibility::Public
    }
}

/// Field in a struct
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StructField {
    pub name: String,
    #[serde(rename = "type")]
    pub type_ref: TypeRef,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// Struct type definition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StructAsset {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "typeKind")]
    pub type_kind: TypeKind,
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub fields: Vec<StructField>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Enum variant payload
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum VariantPayload {
    /// No payload (unit variant)
    Unit,

    /// Single type payload (tuple variant with one field)
    Single(TypeRef),

    /// Multiple fields (struct variant)
    Struct(Vec<StructField>),
}

/// Enum variant
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnumVariant {
    pub name: String,
    #[serde(default)]
    pub payload: Option<TypeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// Enum type definition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnumAsset {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "typeKind")]
    pub type_kind: TypeKind,
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub variants: Vec<EnumVariant>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Method parameter
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MethodParam {
    pub name: String,
    #[serde(rename = "type")]
    pub type_ref: TypeRef,
}

/// Method signature
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MethodSignature {
    pub params: Vec<MethodParam>,
    #[serde(rename = "return")]
    pub return_type: TypeRef,
}

/// Trait method
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TraitMethod {
    pub name: String,
    pub signature: MethodSignature,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "defaultBody")]
    pub default_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// Trait type definition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TraitAsset {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "typeKind")]
    pub type_kind: TypeKind,
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub methods: Vec<TraitMethod>,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Type alias definition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AliasAsset {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "typeKind")]
    pub type_kind: TypeKind,
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub ast: TypeAstNode,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Generic asset wrapper
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "typeKind")]
pub enum TypeAsset {
    #[serde(rename = "struct")]
    Struct(StructAsset),
    #[serde(rename = "enum")]
    Enum(EnumAsset),
    #[serde(rename = "trait")]
    Trait(TraitAsset),
    #[serde(rename = "alias")]
    Alias(AliasAsset),
}

impl TypeAsset {
    pub fn name(&self) -> &str {
        match self {
            TypeAsset::Struct(s) => &s.name,
            TypeAsset::Enum(e) => &e.name,
            TypeAsset::Trait(t) => &t.name,
            TypeAsset::Alias(a) => &a.name,
        }
    }

    pub fn kind(&self) -> TypeKind {
        match self {
            TypeAsset::Struct(_) => TypeKind::Struct,
            TypeAsset::Enum(_) => TypeKind::Enum,
            TypeAsset::Trait(_) => TypeKind::Trait,
            TypeAsset::Alias(_) => TypeKind::Alias,
        }
    }
}
