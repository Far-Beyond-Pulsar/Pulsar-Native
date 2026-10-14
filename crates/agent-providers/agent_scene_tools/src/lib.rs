//! Engine-native scene tools for agents.
//!
//! The tools read and edit the live SceneDB scene directly, through the same
//! typed world operations the editor uses, instead of going through a file
//! or a JSON copy:
//!
//! - **Reads** (`scene_query`, `scene_get_object`, `scene_list_component_classes`,
//!   `scene_describe_component`) are classified read-only.
//! - **Writes** (`scene_write_*`) are classified write, so the confirmation
//!   policy, taint rule and read-only harnesses apply. Each call is one
//!   transaction: validated against the component classes (reflection decode
//!   with the failing field path in the error), journaled with the state it
//!   replaced under the author `agent:<run_id>`, and rolled back as a whole
//!   when it fails. The run's revert button (and `revert_checkpoint_tree`)
//!   undoes a run's transactions, children's included.
//! - **Sandboxes** ([`sandbox`]): a run marked `sandbox` edits a private fork
//!   of the scene and returns a diff in its report; applying it replays the
//!   diff into the live scene as one transaction.
//!
//! Scene knowledge lives here and in the scene crates; `agent_runtime` only
//! knows the [`agent_runtime::RunExtension`] trait this crate implements.
//!
//! # Wiring
//!
//! ```ignore
//! let toolbox = agent_scene_tools::install_global(Arc::new(MyEditorSceneSource));
//! agent_scene_tools::register_tools(&mut registry, &toolbox);
//! ```

mod ext;
pub mod journal;
pub mod ops;
pub mod sandbox;
pub mod snapshot;
pub mod source;
mod toolbox;
mod tools;

use std::sync::{Arc, OnceLock};

pub use ext::SceneExtension;
/// The world types a [`SceneSource`] implementation sees.
pub use pulsar_scenedb::{Entity, World};
pub use journal::{author_of, Recorder, SceneTransaction, TransactionLog};
pub use sandbox::{ApplyReport, Fork, SceneDiff, SceneImage};
pub use source::{FixedScene, SceneSource, SharedScene};
pub use toolbox::{SceneToolbox, WriteNote};
pub use tools::{register_tools, READ_TOOLS, WRITE_TOOLS};

static GLOBAL: OnceLock<Arc<SceneToolbox>> = OnceLock::new();

/// Create the process-wide toolbox over `source` and register its run
/// extension with the agent runtime. The first call wins; later calls return
/// the existing toolbox.
pub fn install_global(source: Arc<dyn SceneSource>) -> Arc<SceneToolbox> {
    GLOBAL
        .get_or_init(|| {
            let toolbox = SceneToolbox::new(source);
            agent_runtime::register_run_extension(Arc::new(SceneExtension::new(toolbox.clone())));
            toolbox
        })
        .clone()
}

/// The toolbox from [`install_global`], if installed.
pub fn global_toolbox() -> Option<Arc<SceneToolbox>> {
    GLOBAL.get().cloned()
}

/// A toolbox that is not process-wide, with its run extension registered
/// under the `scene` name (replacing any earlier one). For tests and hosts
/// that manage the toolbox themselves.
pub fn install_local(source: Arc<dyn SceneSource>) -> Arc<SceneToolbox> {
    let toolbox = SceneToolbox::new(source);
    agent_runtime::register_run_extension(Arc::new(SceneExtension::new(toolbox.clone())));
    toolbox
}
