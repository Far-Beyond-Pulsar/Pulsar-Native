//! Type definitions for documentation structures
//!
//! Defines all the data structures used throughout the documentation system.
//! These represent the parsed and extracted documentation information.

/// Complete documentation for a crate
#[derive(Debug, Clone)]
pub struct CrateDocumentation {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub structs: Vec<StructDoc>,
    pub enums: Vec<EnumDoc>,
    pub traits: Vec<TraitDoc>,
    pub functions: Vec<FunctionDoc>,
    pub macros: Vec<MacroDoc>,
    pub constants: Vec<ConstantDoc>,
    pub type_aliases: Vec<TypeAliasDoc>,
}

/// Documentation for a struct
#[derive(Debug, Clone)]
pub struct StructDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub generics: Vec<Generic>,
    pub fields: Vec<FieldDoc>,
    pub source_code: String,
}

/// Documentation for an enum
#[derive(Debug, Clone)]
pub struct EnumDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub generics: Vec<Generic>,
    pub variants: Vec<VariantDoc>,
    pub source_code: String,
}

/// Documentation for a trait
#[derive(Debug, Clone)]
pub struct TraitDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub methods: Vec<MethodDoc>,
    pub source_code: String,
}

/// Documentation for a function
#[derive(Debug, Clone)]
pub struct FunctionDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub generics: Vec<Generic>,
    pub parameters: Vec<ParameterDoc>,
    pub return_type: Option<String>,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub is_const: bool,
    pub source_code: String,
}

/// Documentation for a macro
#[derive(Debug, Clone)]
pub struct MacroDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub source_code: String,
}

/// Documentation for a constant
#[derive(Debug, Clone)]
pub struct ConstantDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub type_: String,
    pub value: Option<String>,
}

/// Documentation for a type alias
#[derive(Debug, Clone)]
pub struct TypeAliasDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub target_type: String,
}

/// Documentation for a struct/enum field
#[derive(Debug, Clone)]
pub struct FieldDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub type_: String,
}

/// Documentation for an enum variant
#[derive(Debug, Clone)]
pub struct VariantDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub fields: VariantFields,
}

/// Enum variant field types
#[derive(Debug, Clone)]
pub enum VariantFields {
    Unit,
    Tuple(Vec<String>),
    Struct,
}

/// Documentation for a method
#[derive(Debug, Clone)]
pub struct MethodDoc {
    pub name: String,
    pub doc_comment: Option<String>,
    pub self_param: Option<SelfParam>,
    pub parameters: Vec<ParameterDoc>,
    pub return_type: Option<String>,
    pub source_code: String,
}

/// Documentation for a parameter
#[derive(Debug, Clone)]
pub struct ParameterDoc {
    pub name: String,
    pub type_: String,
}

/// Self parameter type
#[derive(Debug, Clone)]
pub enum SelfParam {
    Value,
    Reference,
}

/// Generic parameter
#[derive(Debug, Clone)]
pub struct Generic {
    pub name: String,
    pub bounds: Vec<String>,
}

/// Visibility level
#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Public,
    PublicCrate,
    PublicSuper,
    PublicIn(String),
    Private,
}
