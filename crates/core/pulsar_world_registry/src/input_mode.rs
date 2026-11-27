//! [`InputModeName`]: the reflected type of the `game::set_input_mode` pin.
//!
//! Scripts pass a plain string; this type exists so a node editor offers a
//! `Game` / `UI` choice for the pin's literal (the native names it in its
//! `pin_type.mode` attribute, `ui_common` registers the editor).

/// `Game` or `UI`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct InputModeName(pub String);

impl InputModeName {
    /// The choices, as written into a pin.
    pub const CHOICES: [&'static str; 2] = ["Game", "UI"];
}

impl Default for InputModeName {
    fn default() -> Self {
        Self(Self::CHOICES[0].to_owned())
    }
}

impl std::ops::Deref for InputModeName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for InputModeName {
    fn from(mode: String) -> Self {
        Self(mode)
    }
}

const _: () = {
    #[pulsar_reflection::pulsar_type(
        serialize_json_with = crate::asset_path::to_json::<InputModeName>,
        deserialize_json_with = crate::asset_path::from_json::<InputModeName>
    )]
    #[allow(dead_code)]
    type Registered = InputModeName;
};
