//! Best-of-N sampling for hard steps on cheap (usually local) models.
//!
//! Enabled per harness with `best_of: N`. At a designated hard step the run
//! asks the model for N-1 more candidates (in parallel when the profile
//! allows) and a [`Verifier`] picks the winner:
//!
//! - the **final report** of a run: always sampled N times,
//! - a **tool-call turn that fails validation** (unparsable text-protocol
//!   block, unknown tool, arguments that break the tool's schema, or anything
//!   the caller's verifier rejects): resampled, valid turns cost nothing.
//!
//! If every candidate is rejected the first one is used, exactly as if
//! best-of were off, so a bad verifier can slow a run but never break it.
//! With `best_of` unset the loop never builds a [`BestOf`], clones no request
//! and starts no thread.

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::Arc,
};

use agent_chat_core::{ChatProvider, ChatRequest, ChatResponse, ToolCall, ToolDefinition};
use serde_json::Value;

use crate::run::CancelToken;
use crate::text_tools;

/// Temperature of the extra samples; the first keeps the run's own.
const SAMPLE_TEMPERATURE: f32 = 0.7;

// ---------------------------------------------------------------------------
// Verifier

/// One sampled reply, with its tool calls already parsed.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Text-protocol blocks that looked like calls but were unusable.
    pub parse_errors: Vec<String>,
}

impl Candidate {
    /// Read `response`; in text-protocol mode the calls are parsed out of the
    /// text, otherwise they are the provider's.
    pub fn from_response(response: &ChatResponse, text_mode: bool, known: &[&str]) -> Self {
        let raw = response
            .assistant_message
            .clone()
            .unwrap_or_else(|| response.streamed_text_chunks.concat());
        if text_mode {
            let parsed = text_tools::parse(&raw, known);
            Candidate {
                text: parsed.text,
                tool_calls: parsed.calls,
                parse_errors: parsed.errors,
            }
        } else {
            Candidate {
                text: raw,
                tool_calls: response.tool_calls.clone(),
                parse_errors: Vec::new(),
            }
        }
    }

    pub fn is_final_report(&self) -> bool {
        self.tool_calls.is_empty() && self.parse_errors.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Usable; the highest score wins, ties go to the earliest candidate.
    Accept(f32),
    Reject(String),
}

pub trait Verifier: Send + Sync {
    fn name(&self) -> &str;
    fn verify(&self, candidate: &Candidate) -> Verdict;
}

/// A verifier from a closure.
pub struct FnVerifier<F> {
    name: String,
    check: F,
}

impl<F: Fn(&Candidate) -> Verdict + Send + Sync> FnVerifier<F> {
    pub fn new(name: impl Into<String>, check: F) -> Self {
        Self {
            name: name.into(),
            check,
        }
    }
}

impl<F: Fn(&Candidate) -> Verdict + Send + Sync> Verifier for FnVerifier<F> {
    fn name(&self) -> &str {
        &self.name
    }
    fn verify(&self, candidate: &Candidate) -> Verdict {
        (self.check)(candidate)
    }
}

/// Every call names a known tool and its arguments satisfy the tool's schema
/// (required fields, primitive types, enums).
pub struct SchemaVerifier {
    tools: Vec<ToolDefinition>,
}

impl SchemaVerifier {
    pub fn new(tools: &[ToolDefinition]) -> Self {
        Self {
            tools: tools.to_vec(),
        }
    }
}

impl Verifier for SchemaVerifier {
    fn name(&self) -> &str {
        "schema-valid"
    }

    fn verify(&self, candidate: &Candidate) -> Verdict {
        for call in &candidate.tool_calls {
            let Some(tool) = self.tools.iter().find(|t| t.name == call.name) else {
                return Verdict::Reject(format!("unknown tool `{}`", call.name));
            };
            if let Err(why) = check_schema(&tool.parameters_json_schema, &call.arguments_json, "arguments") {
                return Verdict::Reject(format!("`{}`: {why}", call.name));
            }
        }
        Verdict::Accept(1.0)
    }
}

/// The subset of JSON Schema weak tool callers get wrong.
fn check_schema(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!("{at} is not one of the allowed values"));
        }
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let ok = match kind {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            "null" => value.is_null(),
            _ => true,
        };
        if !ok {
            return Err(format!("{at} should be {kind}"));
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("missing required `{name}`"));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, sub) in properties {
                if let Some(field) = object.get(key) {
                    check_schema(sub, field, key)?;
                }
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            check_schema(items, item, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

/// Files the reply cites (`` `src/a.rs:12` ``) exist under the workspace.
pub struct RefsExistVerifier {
    root: PathBuf,
}

impl RefsExistVerifier {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

/// Backticked tokens that look like file references.
fn cited_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        // Odd segments sit between a pair of backticks.
        if i % 2 == 0 || part.is_empty() || part.contains(char::is_whitespace) || part.contains("://") {
            continue;
        }
        let mut path = part;
        // Drop `:line` and `:line:col`.
        while let Some((head, tail)) = path.rsplit_once(':') {
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                path = head;
            } else {
                break;
            }
        }
        let has_sep = path.contains('/') || path.contains('\\');
        let has_ext = path
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=6).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
        if has_sep && has_ext && !path.starts_with('-') {
            out.push(path.to_string());
        }
    }
    out
}

impl Verifier for RefsExistVerifier {
    fn name(&self) -> &str {
        "refs-exist"
    }

    fn verify(&self, candidate: &Candidate) -> Verdict {
        for path in cited_paths(&candidate.text) {
            let full = std::path::Path::new(&path);
            let full = if full.is_absolute() { full.to_path_buf() } else { self.root.join(full) };
            if !full.exists() {
                return Verdict::Reject(format!("cites `{path}`, which does not exist"));
            }
        }
        Verdict::Accept(1.0)
    }
}

/// Accepts only what every verifier accepts; the score is their mean.
pub struct AllOf(pub Vec<Arc<dyn Verifier>>);

impl Verifier for AllOf {
    fn name(&self) -> &str {
        "all-of"
    }

    fn verify(&self, candidate: &Candidate) -> Verdict {
        let mut total = 0.0;
        for verifier in &self.0 {
            match guarded(verifier.as_ref(), candidate) {
                Verdict::Accept(score) => total += score,
                reject => return reject,
            }
        }
        Verdict::Accept(if self.0.is_empty() { 1.0 } else { total / self.0.len() as f32 })
    }
}

/// Schema plus refs: what a harness's `best_of: N` uses unless the caller
/// supplies its own.
pub fn default_verifier(tools: &[ToolDefinition], workspace_root: impl Into<PathBuf>) -> Arc<dyn Verifier> {
    Arc::new(AllOf(vec![
        Arc::new(SchemaVerifier::new(tools)),
        Arc::new(RefsExistVerifier::new(workspace_root)),
    ]))
}

/// A verifier that panics rejects instead of taking the run down.
fn guarded(verifier: &dyn Verifier, candidate: &Candidate) -> Verdict {
    catch_unwind(AssertUnwindSafe(|| verifier.verify(candidate)))
        .unwrap_or_else(|_| Verdict::Reject(format!("verifier `{}` panicked", verifier.name())))
}

// ---------------------------------------------------------------------------
// Configuration and selection

/// Run-level best-of options. `RunConfig::best_of` is `None` when off.
#[derive(Clone)]
pub struct BestOf {
    /// Total candidates per hard step, including the first. Below 2 is off.
    pub n: u32,
    pub verifier: Arc<dyn Verifier>,
    /// Sample the run's final reply (a child's report).
    pub on_final_report: bool,
    /// Resample a tool-call turn the verifier rejects.
    pub on_invalid_calls: bool,
    /// Cap on concurrent samples (still one at a time for profiles that do
    /// not allow parallel work).
    pub max_parallel: Option<usize>,
}

impl BestOf {
    /// `None` for `n < 2`.
    pub fn new(n: u32, verifier: Arc<dyn Verifier>) -> Option<Self> {
        (n >= 2).then(|| Self {
            n,
            verifier,
            on_final_report: true,
            on_invalid_calls: true,
            max_parallel: None,
        })
    }
}

/// What the user is told about a best-of step.
#[derive(Clone, Debug, PartialEq)]
pub struct BestOfReport {
    pub requested: u32,
    /// Candidates that actually came back.
    pub sampled: u32,
    pub accepted: u32,
    /// Index of the candidate used (0 = the first, i.e. unchanged).
    pub chosen: usize,
    /// Every candidate was rejected; the first was kept.
    pub fell_back: bool,
    pub verifier: String,
    /// Why the first candidate was rejected, when it was.
    pub first_rejection: Option<String>,
    /// The chosen text when it differs from what was already streamed.
    pub replacement: Option<String>,
}

impl BestOfReport {
    pub fn notice(&self) -> String {
        if self.fell_back {
            format!(
                "Best-of-{}: no candidate passed `{}`; kept the first.",
                self.requested, self.verifier
            )
        } else {
            format!(
                "Best-of-{}: {} of {} candidates passed `{}`; chose #{}.",
                self.requested,
                self.accepted,
                self.sampled,
                self.verifier,
                self.chosen + 1
            )
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Selection {
    pub chosen: usize,
    pub accepted: u32,
    pub fell_back: bool,
    pub rejections: Vec<Option<String>>,
}

/// Pick the best verified candidate; the first when none passes.
pub fn select(candidates: &[Candidate], verifier: &dyn Verifier) -> Selection {
    let mut best: Option<(usize, f32)> = None;
    let mut accepted = 0;
    let mut rejections = Vec::with_capacity(candidates.len());
    for (i, candidate) in candidates.iter().enumerate() {
        let verdict = if let Some(error) = candidate.parse_errors.first() {
            Verdict::Reject(format!("malformed tool call: {error}"))
        } else {
            guarded(verifier, candidate)
        };
        match verdict {
            Verdict::Accept(score) => {
                accepted += 1;
                rejections.push(None);
                let score = if score.is_finite() { score } else { 0.0 };
                if best.is_none_or(|(_, top)| score > top) {
                    best = Some((i, score));
                }
            }
            Verdict::Reject(why) => rejections.push(Some(why)),
        }
    }
    Selection {
        chosen: best.map_or(0, |(i, _)| i),
        accepted,
        fell_back: best.is_none(),
        rejections,
    }
}

// ---------------------------------------------------------------------------
// Sampling

pub struct StepContext<'a> {
    pub provider: &'a Arc<dyn ChatProvider>,
    pub request: &'a ChatRequest,
    /// The profile allows concurrent calls.
    pub parallel: bool,
    pub text_mode: bool,
    pub known_tools: &'a [&'a str],
    pub cancel: &'a CancelToken,
}

/// Apply best-of to `first`, the reply the run already has. Returns the reply
/// to continue with and, when sampling happened, a report. Costs nothing for
/// a step that is not a designated hard step.
pub fn refine(
    options: &BestOf,
    ctx: &StepContext<'_>,
    first: ChatResponse,
) -> (ChatResponse, Option<BestOfReport>) {
    let first_candidate = Candidate::from_response(&first, ctx.text_mode, ctx.known_tools);
    let trigger = if first_candidate.is_final_report() {
        options.on_final_report
    } else {
        options.on_invalid_calls
            && (!first_candidate.parse_errors.is_empty()
                || matches!(guarded(options.verifier.as_ref(), &first_candidate), Verdict::Reject(_)))
    };
    if !trigger || ctx.cancel.is_cancelled() {
        return (first, None);
    }

    let extras = sample_more(options, ctx, options.n.saturating_sub(1) as usize);
    let mut responses = vec![first];
    responses.extend(extras);
    let candidates: Vec<Candidate> = responses
        .iter()
        .map(|r| Candidate::from_response(r, ctx.text_mode, ctx.known_tools))
        .collect();
    let selection = select(&candidates, options.verifier.as_ref());

    let chosen_text = candidates[selection.chosen].text.clone();
    let replacement = (selection.chosen != 0 && candidates[selection.chosen].is_final_report())
        .then_some(chosen_text);
    let report = BestOfReport {
        requested: options.n,
        sampled: responses.len() as u32,
        accepted: selection.accepted,
        chosen: selection.chosen,
        fell_back: selection.fell_back,
        verifier: options.verifier.name().to_string(),
        first_rejection: selection.rejections.first().cloned().flatten(),
        replacement,
    };
    let chosen = responses.swap_remove(selection.chosen);
    (chosen, Some(report))
}

/// `count` extra replies; failures are dropped.
fn sample_more(options: &BestOf, ctx: &StepContext<'_>, count: usize) -> Vec<ChatResponse> {
    let width = if ctx.parallel {
        options.max_parallel.unwrap_or(count).clamp(1, count.max(1))
    } else {
        1
    };
    let mut request = ctx.request.clone();
    request.temperature = Some(request.temperature.unwrap_or(0.0).max(SAMPLE_TEMPERATURE));

    let mut out = Vec::new();
    let mut remaining = count;
    while remaining > 0 && !ctx.cancel.is_cancelled() {
        let batch = remaining.min(width);
        remaining -= batch;
        if batch == 1 {
            if let Ok(response) = ctx.provider.chat(request.clone()) {
                out.push(response);
            }
            continue;
        }
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..batch)
                .map(|_| {
                    let request = request.clone();
                    scope.spawn(move || ctx.provider.chat(request))
                })
                .collect();
            for handle in handles {
                if let Ok(Ok(response)) = handle.join() {
                    out.push(response);
                }
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cand(text: &str) -> Candidate {
        Candidate {
            text: text.into(),
            tool_calls: vec![],
            parse_errors: vec![],
        }
    }

    fn tools() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "read".into(),
            description: None,
            parameters_json_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "line": {"type": "integer"}},
                "required": ["path"]
            }),
        }]
    }

    fn with_call(args: Value) -> Candidate {
        Candidate {
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "read".into(),
                arguments_json: args,
            }],
            ..cand("")
        }
    }

    #[test]
    fn schema_verifier_checks_names_required_and_types() {
        let v = SchemaVerifier::new(&tools());
        assert!(matches!(v.verify(&with_call(json!({"path": "a"}))), Verdict::Accept(_)));
        assert!(matches!(v.verify(&with_call(json!({}))), Verdict::Reject(_)));
        assert!(matches!(v.verify(&with_call(json!({"path": 3}))), Verdict::Reject(_)));
        assert!(matches!(v.verify(&with_call(json!({"path": "a", "line": "x"}))), Verdict::Reject(_)));
        let mut unknown = with_call(json!({"path": "a"}));
        unknown.tool_calls[0].name = "nope".into();
        assert!(matches!(v.verify(&unknown), Verdict::Reject(_)));
    }

    #[test]
    fn cited_paths_finds_file_refs_only() {
        let paths = cited_paths("See `src/a.rs:12:3`, `cargo build`, `x.y` and `https://a/b.c`.");
        assert_eq!(paths, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn refs_verifier_rejects_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "x").unwrap();
        let v = RefsExistVerifier::new(dir.path());
        assert!(matches!(v.verify(&cand("see `src/a.rs:1`")), Verdict::Accept(_)));
        assert!(matches!(v.verify(&cand("see `src/zzz.rs:1`")), Verdict::Reject(_)));
    }

    #[test]
    fn select_prefers_highest_score_then_earliest() {
        let v = FnVerifier::new("len", |c: &Candidate| {
            if c.text == "bad" {
                Verdict::Reject("bad".into())
            } else {
                Verdict::Accept(c.text.len() as f32)
            }
        });
        let picked = select(&[cand("bad"), cand("aa"), cand("bb"), cand("c")], &v);
        assert_eq!(picked.chosen, 1);
        assert_eq!(picked.accepted, 3);
        assert!(!picked.fell_back);
        assert_eq!(picked.rejections[0].as_deref(), Some("bad"));
    }

    #[test]
    fn select_falls_back_to_first_when_all_rejected_or_verifier_panics() {
        let reject = FnVerifier::new("no", |_: &Candidate| Verdict::Reject("no".into()));
        let picked = select(&[cand("a"), cand("b")], &reject);
        assert_eq!((picked.chosen, picked.fell_back), (0, true));

        let panics = FnVerifier::new("boom", |_: &Candidate| -> Verdict { panic!("boom") });
        let picked = select(&[cand("a"), cand("b")], &panics);
        assert_eq!((picked.chosen, picked.fell_back), (0, true));
    }

    #[test]
    fn malformed_candidates_never_win() {
        let any = FnVerifier::new("any", |_: &Candidate| Verdict::Accept(1.0));
        let mut broken = cand("x");
        broken.parse_errors.push("bad json".into());
        let picked = select(&[broken, cand("y")], &any);
        assert_eq!(picked.chosen, 1);
    }

    #[test]
    fn new_requires_at_least_two() {
        let v: Arc<dyn Verifier> = Arc::new(SchemaVerifier::new(&[]));
        assert!(BestOf::new(0, v.clone()).is_none());
        assert!(BestOf::new(1, v.clone()).is_none());
        assert!(BestOf::new(2, v).is_some());
    }
}
