# Agent runtime evals

Regression evals for the properties the agent framework promises. They drive
the real projector and run loop with a mock provider, no network.

    cargo test -p agent_runtime --test evals

## What is covered

| Module | Property |
|---|---|
| `report_roles` | Sub-agent reports are never a system message; tool-capable modes deliver them as tool results; text-only mode wraps them in a `trust="data"` envelope that cannot be closed early |
| `pins` | Pinned requirements are in every request across 200 turns and many compactions at a 4k profile |
| `budget` | A 4k profile never sends more than `context - reserve_output` estimated tokens, even with huge tool results |
| `tainted_writes` | A write after untrusted content is gated; denial blocks it; a clean-context write is not gated |
| `steward_off` | A disabled steward makes zero extra requests and leaves the main request bytes unchanged |
| `determinism` | Same ledger, byte-identical messages (repeat, clone, persisted) |
| `time_travel` | `Ledger::fork_at`, snapshots, and replaying a recorded ledger against a mock provider |

## Adding an eval

1. Pick the module (or add `mod yours;` to `main.rs`).
2. Build a `Ledger` and drive it with `agent_runtime::testing::MockProvider`:
   - `MockProvider::scripted(vec![ScriptedStep::call(..), ScriptedStep::say(..)])`
     pops one step per request and errors when exhausted;
   - `MockProvider::from_fn(|request, index| ..)` for replies that depend on the request
     (for example, detect the compaction summarizer by its system prompt);
   - `provider.requests()` returns every `ChatRequest` the runtime sent.
3. Run `run_loop(common::config(provider, profile), ledger, CancelToken::new(), sink)`
   or call `project(&ledger, &profile)` directly.
4. Assert on the requests, the returned ledger, or the events.

## Golden transcripts

`common::assert_golden("name", &common::render(&messages))` compares against
`golden/name.txt`. After an intended projection change, regenerate with

    UPDATE_GOLDEN=1 cargo test -p agent_runtime --test evals

and review the diff of `golden/` like any other code change. A golden that
changes unexpectedly is the eval doing its job.

## Replaying a recorded run

`agent_runtime::replay(&recorded_ledger, &profile, |provider| config, sink)`
re-runs each user turn against a scripted provider built from the recorded
assistant turns, and reports the first divergence (`Replay::divergence`).
Tools run again through the registry your closure supplies, so stub them if
they have side effects. Tool exchanges are not persisted to disk yet, so only
in-memory ledgers carry enough to replay.
