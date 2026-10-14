//! The harness `verify:` hook: checks run on a child's report before it is
//! returned. They are [`Verifier`]s, the same trait best-of-N selects with.
//!
//! Names: `refs_exist` (every cited file exists), `schema` (the report has
//! the harness's sections), `nonempty` (the report says something), `default`
//! (`schema` + `refs_exist`).

use std::{path::Path, sync::Arc};

use crate::best_of::{AllOf, Candidate, RefsExistVerifier, Verdict, Verifier};
use crate::delegation::{heading_name, parse_report};

pub const KNOWN: [&str; 4] = ["refs_exist", "schema", "nonempty", "default"];

pub fn is_known(name: &str) -> bool {
    KNOWN.contains(&name.trim())
}

/// The report must contain a heading for every section of its schema.
pub struct ReportSchemaVerifier {
    schema: Vec<String>,
}

impl ReportSchemaVerifier {
    pub fn new(schema: &[String]) -> Self {
        Self {
            schema: schema.to_vec(),
        }
    }
}

impl Verifier for ReportSchemaVerifier {
    fn name(&self) -> &str {
        "report-schema"
    }

    fn verify(&self, candidate: &Candidate) -> Verdict {
        let present: Vec<String> = candidate
            .text
            .lines()
            .filter_map(|line| heading_name(line, &self.schema))
            .collect();
        let missing: Vec<&str> = self
            .schema
            .iter()
            .filter(|field| !present.contains(field))
            .map(String::as_str)
            .collect();
        if missing.is_empty() {
            Verdict::Accept(1.0)
        } else {
            Verdict::Reject(format!(
                "the report is missing the section(s): {}",
                missing.join(", ")
            ))
        }
    }
}

/// The report must have a non-empty summary.
pub struct NonEmptyVerifier {
    schema: Vec<String>,
}

impl Verifier for NonEmptyVerifier {
    fn name(&self) -> &str {
        "report-nonempty"
    }

    fn verify(&self, candidate: &Candidate) -> Verdict {
        if parse_report(&candidate.text, &self.schema).summary.trim().is_empty() {
            Verdict::Reject("the report has an empty summary".to_string())
        } else {
            Verdict::Accept(1.0)
        }
    }
}

/// The verifier for a harness's `verify` names, or `None` for no checks.
pub fn build(names: &[String], schema: &[String], workspace_root: &Path) -> Option<Arc<dyn Verifier>> {
    let mut checks: Vec<Arc<dyn Verifier>> = Vec::new();
    for name in names {
        match name.trim() {
            "refs_exist" => checks.push(Arc::new(RefsExistVerifier::new(workspace_root))),
            "schema" => checks.push(Arc::new(ReportSchemaVerifier::new(schema))),
            "nonempty" => checks.push(Arc::new(NonEmptyVerifier {
                schema: schema.to_vec(),
            })),
            "default" => {
                checks.push(Arc::new(ReportSchemaVerifier::new(schema)));
                checks.push(Arc::new(RefsExistVerifier::new(workspace_root)));
            }
            _ => {}
        }
    }
    (!checks.is_empty()).then(|| Arc::new(AllOf(checks)) as Arc<dyn Verifier>)
}

/// A report as a candidate. Entries of the `refs` section are cited in
/// backticks too, which is the form `refs_exist` reads.
pub fn candidate_for(text: &str, schema: &[String]) -> Candidate {
    let report = parse_report(text, schema);
    let mut full = text.to_string();
    for reference in &report.refs {
        let reference = reference.trim().trim_matches('`');
        if !reference.is_empty() && !reference.contains(char::is_whitespace) {
            full.push_str(&format!("\n`{reference}`"));
        }
    }
    Candidate {
        text: full,
        tool_calls: Vec::new(),
        parse_errors: Vec::new(),
    }
}

/// `Ok` when every check passes; `Err(reason)` otherwise.
pub fn check(verifier: &dyn Verifier, text: &str, schema: &[String]) -> Result<(), String> {
    match verifier.verify(&candidate_for(text, schema)) {
        Verdict::Accept(_) => Ok(()),
        Verdict::Reject(reason) => Err(reason),
    }
}

/// What the child is told when its report failed a check.
pub fn retry_message(reason: &str) -> String {
    format!(
        "Your report did not pass verification: {reason}. Fix this and reply with the complete corrected report."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::DEFAULT_RESULT_FIELDS;

    fn schema() -> Vec<String> {
        DEFAULT_RESULT_FIELDS.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn schema_check_names_the_missing_sections() {
        let v = build(&["schema".to_string()], &schema(), Path::new(".")).unwrap();
        let err = check(v.as_ref(), "## Summary\nok\n## Findings\n- a", &schema()).unwrap_err();
        assert!(err.contains("refs") && err.contains("open_questions"), "{err}");
        let full = "## Summary\nok\n## Findings\n- a\n## Refs\n- x\n## Open questions\nnone";
        assert!(check(v.as_ref(), full, &schema()).is_ok());
    }

    #[test]
    fn refs_in_the_refs_section_are_checked_even_without_backticks() {
        let dir = std::env::temp_dir().join(format!("report_verify_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/real.rs"), "fn main() {}").unwrap();
        let v = build(&["refs_exist".to_string()], &schema(), &dir).unwrap();
        let good = "## Summary\ns\n## Refs\n- src/real.rs:1";
        assert!(check(v.as_ref(), good, &schema()).is_ok());
        let bad = "## Summary\ns\n## Refs\n- src/missing.rs:9";
        assert!(check(v.as_ref(), bad, &schema()).is_err());
    }

    #[test]
    fn unknown_names_are_rejected_by_the_parser() {
        let err = crate::harness::Harness::parse(
            "---\nname: x\nverify: [bogus]\n---\nbody",
            "x",
            crate::harness::HarnessOrigin::Project,
        )
        .unwrap_err();
        assert!(err.contains("bogus"));
    }
}
