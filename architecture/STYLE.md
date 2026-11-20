# Style ratchet

`just style` holds first-party Rust to the readability conventions in
`CONTRIBUTING.md` ("Target conventions"; the reasoning is in
`.agents/READABILITY_REPORT.md`). It is a shrink-only ratchet like
`just arch`: the existing violations are recorded in
`architecture/style-baseline.json`, and the check fails when any file's count
rises or a file that was clean starts violating. Old code is not rewritten to
get the gate in; it just cannot get worse.

```
just style                          check against the baseline (CI-ready)
just style --report                 totals per rule and the worst files
just style --update                 lower the baseline after fixing violations
just style --update --allow-increase   only for a pure file move or split
```

It is one stdlib-only Python file, `scripts/style-check.py` (Python 3.8+; the
same interpreter `just arch` needs, and Helio's
`scripts/architecture/run.ps1` finds one on Windows). `--root DIR` points it
at another tree (a plain directory walk when it is not a git checkout), which
is how the gate is tested.

## Scope

Tracked `.rs` files, minus `plugins/vendor/`, `crates/renderer/helio/`,
`crates/third-party/`, `crates/ui/wgpui*`, `crates/external/` and `target/`.
Those are separate repositories or vendored; each audits itself (a plugin
repository can carry its own copy of the script and baseline).

**Test code** is a file under `tests/` or `benches/`, named `tests.rs`,
`*_tests.rs`, `*_test.rs` or `test_*.rs`, declared `#[cfg(test)] mod x;`, or
starting with `#![cfg(test)]`; inside other files it is the body of a
`#[cfg(test)]` or `#[test]` item. Rules `unwrap`, `println` and `pub_result_string` skip it.

## Rules

| Rule (baseline key) | Limit | Why |
|---|---|---|
| `fn_length` | A function longer than 100 lines counts one | A 600-line function cannot be held in a reader's head or reviewed. |
| `file_length` | A file over 600 lines (tests: 1500); the baseline stores the line count | One file, one job. Storing the size means an over-long file cannot keep growing. |
| `process_comments` | A comment line with `Phase N`, `P<digit>` (in `//` comments) or a bare issue number (`#1234`, `Pulsar-Native#1234`) | Process belongs in commits and `.agents/` plans; source explains the code. Allowed: `TODO(#1234): what and why`. |
| `println` | `println!`/`eprintln!` in non-test library code | The engine logs through `tracing`, which has levels, spans and the Tasks window. |
| `unwrap` | `.unwrap()` in non-test code | A panic with no message. Use `?`, or `expect("why this cannot fail")`. |
| `pub_result_string` | `Result<_, String>` in a `pub fn` signature (non-test) | Stringly errors cannot be matched on or chained. Use a `pulsar_error` type. |

`println` also exempts what is not library code: `main.rs`, `build.rs`,
`src/bin/`, `examples/`, binary-only crates (a `main.rs` and no `lib.rs`) and
`tools/`. They are programs whose output is the interface.

Where printing is the feature of library code (a debug node whose job is to print,
a standalone runner's report on stdout), put `// style: allow println - <reason>`
on the line or the line above. The reason is required and is read in review; the
marker is for deliberate output, not for logging.

Functions are measured on all code, tests included. File length uses raw
lines.

## How it measures

No parser. Comments and the inside of string and char literals are blanked
(nested block comments, raw strings and lifetimes are handled), then function
bodies are found by brace matching. A function's length is the raw lines from
its `fn` keyword to its closing brace, comments and blanks included. Functions
written inside a `macro_rules!` body are measured as written there. Treat the
numbers as stable orderings: the same code always measures the same, which is
all a ratchet needs. A false positive is fixed by changing the code, or, if the
measurement is wrong, by fixing the script.

## The ratchet

- A count above the baseline, or a violating file not in it, fails with the
  rule and file.
- A count below the baseline passes and prints "baseline can shrink: run
  `just style --update`". Run it in the same change that fixed the code, so
  the gain is locked in.
- `--update` refuses to raise any count or add a file. `--allow-increase` is
  for a file moved or split, where the violations travel with the code: the
  totals per rule (printed on update) must not rise, and a reviewer should
  check that. Never use it to get a change through.
- Do not edit `style-baseline.json` by hand.

## Fixing a violation

- `fn_length`: extract named steps (each does one thing and has a name that
  says it), pass related parameters as a struct, move repeated
  `map_err(|e| format!(...))` into one helper.
- `file_length`: split by responsibility into modules; make the old file a
  table of contents (`mod` lines, docs, curated `pub use`).
- `process_comments`: delete the reference, keeping any explanation that
  helps a reader of the code. The context lives in the commit message or the
  `.agents/*_PLAN.md`. A real TODO becomes `TODO(#NNNN): what and why`.
- `println`: use `tracing::{info, warn, error, debug}`.
- `unwrap`: propagate with `?`; where the case really cannot happen, say why
  with `expect("...")`.
- `pub_result_string`: return a typed error (`pulsar_error`, or a small enum
  with `thiserror`), convert at the edge.

## Not gated yet

`lib.rs`/`mod.rs` staying a table of contents (at most 150 lines of
non-`mod`/`pub use` code), `Box<dyn Error>`, `.expect()`, `//!` module docs.
They are in the report and can be added as further rules.

## Wiring

`just style` is separate from `just arch` on purpose. CI does not run it yet:
it belongs in `.github/workflows/ci.yml` right after the "Layer gate" step, as
a step running `python3 scripts/style-check.py`. It needs the git checkout, not
the submodules.
