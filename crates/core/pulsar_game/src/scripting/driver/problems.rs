//! Converting runtime errors and dropped calls into editor problems.
//! Entry point: [`ScriptDriver::problems`].


use pulsar_script_runtime::RuntimeError;

use super::{DriverReport, ScriptDriver};


impl ScriptDriver {
    /// The frame's errors and dropped calls as editor problems (#854,
    /// #868): class, instance, function, graph node and the class's source
    /// file, where known.
    pub fn problems(&self, report: &DriverReport) -> Vec<pulsar_events::ScriptProblem> {
        let mut problems: Vec<_> = report
            .script_errors
            .iter()
            .chain(&report.load_errors)
            .map(|error| self.problem_for(error))
            .collect();
        for (class, call) in &report.dropped_calls {
            let mut problem = pulsar_events::ScriptProblem {
                severity: pulsar_events::ProblemSeverity::Warning,
                class: Some(class.clone()),
                instance: Some(call.object_id.clone()),
                function: Some(call.function.clone()),
                message: format!("class reload dropped a waiting call: {}", call.reason),
                ..Default::default()
            };
            self.fill_class(&mut problem, None);
            problems.push(problem);
        }
        problems
    }

    /// One error as an editor problem.
    pub fn problem_for(&self, error: &RuntimeError) -> pulsar_events::ScriptProblem {
        let details = error.details();
        let mut problem = pulsar_events::ScriptProblem {
            severity: pulsar_events::ProblemSeverity::Error,
            class: details.class,
            instance: details.object_id,
            function: details.function,
            node: details
                .location
                .as_ref()
                .map(|l| l.node.clone())
                .filter(|n| !n.is_empty()),
            line: details.location.as_ref().and_then(|l| l.line),
            message: details.message,
            ..Default::default()
        };
        self.fill_class(
            &mut problem,
            details.location.as_ref().map(|l| l.file.as_str()),
        );
        problem
    }

    /// Fill the class GUID and source path from the registry.
    pub(super) fn fill_class(&self, problem: &mut pulsar_events::ScriptProblem, file: Option<&str>) {
        let Some(entry) = problem.class.as_deref().and_then(|name| {
            let guid = self
                .loaded
                .iter()
                .find(|(_, n)| n.as_str() == name)
                .map(|(guid, _)| guid.clone());
            guid.and_then(|g| self.registry.by_id(&g))
                .or_else(|| self.registry.by_name(name))
        }) else {
            return;
        };
        problem.class_id = Some(entry.id.as_str().to_owned());
        problem.path = Some(match file.filter(|f| !f.is_empty()) {
            Some(file) => entry.dir.join(file),
            None => entry.dir.clone(),
        });
    }
}
