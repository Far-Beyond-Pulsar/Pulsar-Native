use serde::{Deserialize, Serialize};

/// Classifies what kind of asset a drag payload carries.
///
/// Used by drop targets to decide whether to accept or reject a drag without
/// inspecting the full payload. All built-in kinds map to well-known file
/// extensions. Plugins can define their own kinds via [`AssetKind::Custom`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetKind {
    /// 3D geometry: .fbx .gltf .glb .obj .usd .usdc .dae .abc
    Mesh,
    /// Image/texture: .png .jpg .jpeg .exr .hdr .tga .webp .bmp .dds .ktx
    Texture,
    /// Material definition: .pulsarmat
    Material,
    /// Audio: .wav .ogg .mp3 .flac .aiff
    Audio,
    /// Scene / level file: .pulsarscene
    Scene,
    /// Blueprint graph: .blueprint
    Blueprint,
    /// Script: .lua .wren .rhai .js
    Script,
    /// Font: .ttf .otf .woff .woff2
    Font,
    /// Shader source: .wgsl .hlsl .glsl .spv
    Shader,
    /// Raw data / config: .json .toml .yaml .csv
    Data,
    /// Plugin-defined custom kind. The string is an opaque identifier chosen
    /// by the plugin (e.g. `"com.myplugin.terrain_heightmap"`).
    Custom(String),
    /// Kind could not be determined from the extension alone.
    Unknown,
}

impl AssetKind {
    /// Derive the asset kind from a file extension (case-insensitive, without
    /// the leading dot).
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "fbx" | "gltf" | "glb" | "obj" | "usd" | "usdc" | "dae" | "abc" => Self::Mesh,
            "png" | "jpg" | "jpeg" | "exr" | "hdr" | "tga" | "webp" | "bmp" | "dds" | "ktx" => {
                Self::Texture
            }
            "pulsarmat" => Self::Material,
            "wav" | "ogg" | "mp3" | "flac" | "aiff" => Self::Audio,
            "pulsarscene" => Self::Scene,
            "blueprint" => Self::Blueprint,
            "lua" | "wren" | "rhai" | "js" => Self::Script,
            "ttf" | "otf" | "woff" | "woff2" => Self::Font,
            "wgsl" | "hlsl" | "glsl" | "spv" => Self::Shader,
            "json" | "toml" | "yaml" | "yml" | "csv" => Self::Data,
            _ => Self::Unknown,
        }
    }

    /// Returns `true` if this kind is a 3D mesh format.
    pub fn is_mesh(&self) -> bool {
        matches!(self, Self::Mesh)
    }

    /// Returns `true` if this kind is a texture/image format.
    pub fn is_texture(&self) -> bool {
        matches!(self, Self::Texture)
    }

    /// Returns `true` if this kind is an audio format.
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio)
    }

    /// Returns `true` if this kind is a scene/level format.
    pub fn is_scene(&self) -> bool {
        matches!(self, Self::Scene)
    }

    /// Returns `true` if this kind is a blueprint graph.
    pub fn is_blueprint(&self) -> bool {
        matches!(self, Self::Blueprint)
    }

    /// Returns `true` if this is a custom plugin-defined kind.
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Short human-readable label suitable for UI display and drag ghost tooltips.
    pub fn display_label(&self) -> &str {
        match self {
            Self::Mesh => "3D Mesh",
            Self::Texture => "Texture",
            Self::Material => "Material",
            Self::Audio => "Audio",
            Self::Scene => "Scene",
            Self::Blueprint => "Blueprint",
            Self::Script => "Script",
            Self::Font => "Font",
            Self::Shader => "Shader",
            Self::Data => "Data",
            Self::Custom(s) => s.as_str(),
            Self::Unknown => "File",
        }
    }
}
