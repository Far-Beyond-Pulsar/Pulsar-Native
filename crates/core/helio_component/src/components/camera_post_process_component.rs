//! A render view's post-process baseline. It reaches the post-process
//! resolve as its derived row ([`super::environment_rows`]) through the
//! graph's environment join.
use super::PostProcessSettingsProps;
use engine_class_derive::{engine_class, register_world_component};

pub const CAMERA_POST_PROCESS_CLASS_NAME: &str = "CameraPostProcessComponent";

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Camera", category_color = "#3AA0FF")]
#[serde(default)]
pub struct CameraPostProcessComponent {
    #[property(category = "Camera")]
    pub enabled: bool,
    /// Generic render view identifier, independent of any editor camera type.
    #[property(category = "Camera")]
    pub view_id: u32,
    #[sub_props]
    #[serde(flatten)]
    pub settings: PostProcessSettingsProps,
}

impl Default for CameraPostProcessComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            view_id: 0,
            settings: PostProcessSettingsProps::default(),
        }
    }
}

#[register_world_component]
impl CameraPostProcessComponent {}
