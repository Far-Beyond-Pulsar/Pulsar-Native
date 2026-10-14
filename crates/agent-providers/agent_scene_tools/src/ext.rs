//! The scene toolbox as an `agent_runtime` run extension: tool
//! classification, one-click revert, sandbox diffs in child reports and the
//! panel's Apply / Discard reviews.

use std::sync::Arc;

use agent_runtime::{Review, ReviewDecision, RunExtension, ToolClass};

use crate::toolbox::SceneToolbox;
use crate::tools::{READ_TOOLS, WRITE_TOOLS};

pub struct SceneExtension {
    toolbox: Arc<SceneToolbox>,
}

impl SceneExtension {
    pub fn new(toolbox: Arc<SceneToolbox>) -> Self {
        Self { toolbox }
    }
}

impl RunExtension for SceneExtension {
    fn name(&self) -> &'static str {
        "scene"
    }

    fn classify(&self, tool: &str) -> Option<ToolClass> {
        if READ_TOOLS.contains(&tool) {
            Some(ToolClass::Read)
        } else if WRITE_TOOLS.contains(&tool) {
            Some(ToolClass::Write)
        } else {
            None
        }
    }

    fn has_undo(&self, run_id: &str) -> bool {
        self.toolbox.log().has_undo(run_id) || self.toolbox.has_pending_in(run_id)
    }

    fn revert(&self, run_id: &str) -> Result<Vec<String>, String> {
        self.toolbox.revert_tree(run_id)
    }

    fn revert_with(&self, run_id: &str, force: bool) -> Result<Vec<String>, String> {
        self.toolbox.revert_tree_with(run_id, force)
    }

    fn report_sections(&self, run_id: &str) -> Vec<(String, String)> {
        self.toolbox.finish_sandbox(run_id)
    }

    fn pending_reviews(&self) -> Vec<Review> {
        self.toolbox
            .pending()
            .into_iter()
            .map(|(id, title, summary)| Review {
                run_id: id.clone(),
                id,
                title,
                summary,
            })
            .collect()
    }

    fn resolve_review(&self, id: &str, decision: ReviewDecision) -> Result<String, String> {
        match decision {
            ReviewDecision::Apply => self.toolbox.apply_sandbox(id, false),
            ReviewDecision::Discard => self.toolbox.discard_sandbox(id),
        }
    }
}
