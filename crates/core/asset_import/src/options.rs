//! The import options a format's importer offers, in terms of the
//! reflection system so the editor's property editors render them
//! generically (the configurator).

use std::any::Any;
use std::fmt;

use pulsar_reflection::RuntimeTypeInfo;

/// UI constraints for an import field.
#[derive(Debug, Clone, Default)]
pub struct FieldConstraints {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

/// A single configurable import option — expressed in terms of the
/// reflection system so the gpui property editors can render it generically.
pub struct ImportField {
    pub key: String,
    pub label: String,
    pub doc: String,
    pub type_info: &'static RuntimeTypeInfo,
    pub default: Box<dyn Any + Send>,
    pub constraints: FieldConstraints,
}

impl fmt::Debug for ImportField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportField")
            .field("key", &self.key)
            .field("label", &self.label)
            .field("doc", &self.doc)
            .field("type_info", &self.type_info)
            .field("default", &"<dyn Any>")
            .field("constraints", &self.constraints)
            .finish()
    }
}

/// An ordered set of [`ImportField`]s describing a loader's import options.
#[derive(Debug)]
pub struct OptionsSchema {
    pub fields: Vec<ImportField>,
}
