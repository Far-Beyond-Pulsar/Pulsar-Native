//! [`asset_path_type!`](crate::asset_path_type): the one way a component
//! declares a property that names an asset file by project-relative path.

use std::ops::Deref;

use pulsar_reflection::{ReflectError, ReflectResult};

/// The saved form of an asset path: a plain string.
#[doc(hidden)]
pub fn to_json<T: Deref<Target = str>>(path: &T) -> ReflectResult<serde_json::Value> {
    Ok(serde_json::Value::String(path.to_string()))
}

/// Reads [`to_json`]'s form; `null` is no asset.
#[doc(hidden)]
pub fn from_json<T: From<String> + Default>(value: serde_json::Value) -> ReflectResult<T> {
    match value {
        serde_json::Value::String(path) => Ok(T::from(path)),
        serde_json::Value::Null => Ok(T::default()),
        other => Err(ReflectError::TypeMismatch {
            expected: "an asset path",
            found: format!("{other:?}"),
        }),
    }
}

/// Declare an asset-path property type: a transparent newtype over `String`
/// (saved data is the plain path), which
///
/// - gets the shared searchable asset picker in property panels, offering
///   files with the given extensions (feature `editor` of the declaring
///   crate, which then depends on `ui_common` and `gpui-ce`);
/// - reads and writes as a `string` in scripts, so a component field of this
///   type keeps its Blueprint get and set nodes.
///
/// Offer only extensions that something loads and that the packer ships
/// (`pulsar_content::AssetKind::from_path`, or one of its asset keys).
///
/// ```ignore
/// pulsar_world_registry::asset_path_type! {
///     /// A sound file.
///     pub SoundAssetPath, empty = "No sound", extensions = ["wav", "ogg"]
/// }
/// ```
///
/// The declaring crate needs `serde`, `serde_json` and `pulsar_reflection`,
/// and a feature named `editor`. A crate with no GPUI code writes
/// `editor = elsewhere,` before `extensions` instead: it needs no feature,
/// and `ui_common` registers the type's editor.
#[macro_export]
macro_rules! asset_path_type {
    // A type whose editor another crate registers (a kernel crate, which has
    // no GPUI code).
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident,
        editor = elsewhere,
        extensions = [$($extension:literal),+ $(,)?] $(,)?
    ) => {
        $crate::asset_path_type!(@type $(#[$meta])* $vis $name, [$($extension),+]);

        const _: () = {
            #[::pulsar_reflection::pulsar_type(
                serialize_json_with = $crate::asset_path::to_json::<$name>,
                deserialize_json_with = $crate::asset_path::from_json::<$name>
            )]
            #[allow(dead_code)]
            type Registered = $name;
        };
    };
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident,
        empty = $empty:literal,
        extensions = [$($extension:literal),+ $(,)?] $(,)?
    ) => {
        $crate::asset_path_type!(@type $(#[$meta])* $vis $name, [$($extension),+]);

        // A block of its own, so the registrations' names never collide
        // between two types of one module.
        const _: () = {
            #[cfg(feature = "editor")]
            impl ::ui_common::asset_path_editor::AssetPathValue for $name {
                fn from_path(path: ::std::string::String) -> Self {
                    Self(path)
                }

                fn path(&self) -> &str {
                    &self.0
                }
            }

            #[cfg(feature = "editor")]
            fn editor(
                args: &::pulsar_reflection::PropertyEditorArgs<'_>,
                window: &mut ::gpui::Window,
                cx: &mut ::gpui::App,
            ) -> ::pulsar_reflection::BoundPropertyEditor {
                ::ui_common::asset_path_editor::asset_path_editor::<$name>(
                    args,
                    $empty,
                    $name::EXTENSIONS,
                    window,
                    cx,
                )
            }

            #[cfg_attr(
                feature = "editor",
                ::pulsar_reflection::pulsar_type(
                    serialize_json_with = $crate::asset_path::to_json::<$name>,
                    deserialize_json_with = $crate::asset_path::from_json::<$name>,
                    editor = editor
                )
            )]
            #[cfg_attr(
                not(feature = "editor"),
                ::pulsar_reflection::pulsar_type(
                    serialize_json_with = $crate::asset_path::to_json::<$name>,
                    deserialize_json_with = $crate::asset_path::from_json::<$name>
                )
            )]
            #[allow(dead_code)]
            type Registered = $name;
        };
    };
    (@type $(#[$meta:meta])* $vis:vis $name:ident, [$($extension:literal),+]) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(transparent)]
        $vis struct $name(pub ::std::string::String);

        impl $name {
            /// The file extensions the picker offers.
            pub const EXTENSIONS: &'static [&'static str] = &[$($extension),+];
        }

        impl ::std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::convert::From<::std::string::String> for $name {
            fn from(path: ::std::string::String) -> Self {
                Self(path)
            }
        }

        impl ::std::convert::From<&str> for $name {
            fn from(path: &str) -> Self {
                Self(path.to_owned())
            }
        }

        $crate::pulsar_script_vm::script_text_type!($name);
    };
}

#[cfg(test)]
mod tests {
    use pulsar_script_vm::{Type, TypeRegistry};

    crate::asset_path_type! {
        pub ProbePath, editor = elsewhere, extensions = ["probe"]
    }

    #[test]
    fn scripts_see_an_asset_path_as_a_string_and_it_saves_as_one() {
        let registry = TypeRegistry::global();
        let binding = registry
            .binding(std::any::TypeId::of::<ProbePath>())
            .expect("bound");
        assert_eq!(binding.script_type(), Type::Str);
        let value = binding.to_value(&ProbePath::from("a/b.probe"));
        let back = binding.from_value(&value).expect("a string");
        assert_eq!(
            back.downcast_ref::<ProbePath>(),
            Some(&ProbePath::from("a/b.probe"))
        );

        assert_eq!(ProbePath::EXTENSIONS, ["probe"]);
        let saved = serde_json::to_value(ProbePath::from("a/b.probe")).unwrap();
        assert_eq!(saved, serde_json::json!("a/b.probe"));
        assert_eq!(
            super::from_json::<ProbePath>(serde_json::Value::Null).unwrap(),
            ProbePath::default()
        );
    }
}
