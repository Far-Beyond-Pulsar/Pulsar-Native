use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UiValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Array(Vec<UiValue>),
    Object(HashMap<String, UiValue>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiComponent {
    #[serde(rename = "type")]
    pub component_type: String,

    #[serde(default)]
    pub props: HashMap<String, UiValue>,

    #[serde(default)]
    pub children: Vec<UiChild>,

    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UiChild {
    Component(UiComponent),
    Text(String),
    Reference {
        #[serde(rename = "$ref")]
        reference: String,
        #[serde(default)]
        props: HashMap<String, UiValue>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiDocument {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    #[serde(flatten)]
    pub root: UiComponent,
}

impl UiValue {
    pub fn as_string(&self) -> Option<&str> {
        match self {
            UiValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            UiValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            UiValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<UiValue>> {
        match self {
            UiValue::Array(arr) => Some(arr),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&HashMap<String, UiValue>> {
        match self {
            UiValue::Object(obj) => Some(obj),
            _ => None,
        }
    }
}
