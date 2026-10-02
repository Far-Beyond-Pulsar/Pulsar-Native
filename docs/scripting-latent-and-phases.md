# Latent actions and the script phase

Two things about how scripts run: what a call can *wait for* (#863), and how
much of the script phase needs the world's exclusive lock (#860). Equality and
printing for value types (#859) is at the end.

## Latent actions

The VM has one way to suspend a call: `Wait { seconds }`. Everything else a
script may wait for is a native that **asks** the runtime to park the call; the
VM stays small and the runtime owns the clock, the frame counter, the instance
and its events.

A native writes a `Wake` into the instance's `Latent` state (`Host::latent`);
the VM suspends the call right after the native returns, in the interpreter and
in generated code alike. A host that attaches no `Latent` (the exported-Rust
actors, plain tests) makes these natives fail the call with a clear error.

| Native | Resumes |
|---|---|
| `wait::frames(n)` | after `n` ticks (at least 1; never the tick it was called in) |
| `wait::next_tick()` | the next tick |
| `wait::until(predicate)` | when the exported, parameterless `bool` function `predicate` returns true; tested once per tick, before `tick` |
| `wait::event(name)` | right after the instance handles an event named `name` (a subscribed hub event, or a custom event sent with `send_event`) |
| `schedule::call(function, seconds)` → handle | runs the exported `function` once after `seconds` |
| `schedule::repeat(function, interval)` → handle | every `interval` seconds (a long hitch fires at most 8 catch-up calls) |
| `schedule::restart(key, function, seconds)` → handle | **retriggerable delay**: calling again with the same key restarts the countdown instead of stacking |
| `schedule::clear(handle)`, `schedule::clear_key(key)` | cancels |
| `schedule::pending(handle)`, `schedule::remaining(handle)` | queries |

Order within a tick: scheduled calls, then calls whose wait is over, then `tick`.
A call that waits again resumes on a later tick.

Rules worth knowing:

- Waits and schedules survive a hot reload when the code's shape still fits
  (the existing continuation rebase, #862); a schedule whose function is no
  longer exported is dropped with a warning.
- A bad `wait::until` predicate (missing, wrong signature, failing) drops the
  call with a reported error rather than being retried every tick.
- At most 1024 pending scheduled calls per instance.
- `timer::set` / `timer::clear` (the event hub's timers) are unchanged: they
  publish `TimerFired`. `schedule::*` names the function to run and needs no
  subscription.
- "Do once / reset" per instance is the existing stateful Blueprint flow nodes
  (#876): their state is per instance.

## The script phase in two stages

Natives declare what they do to the world (`NativeFn::access()`): the `access`
attribute (`read` / `write`), else side-effect-free natives read and everything
else writes. A class whose imports all read (`Program::access()`) cannot change
the world, and each instance only changes its own variables.

`TickLoop` runs the script phase through `ScriptDriver::run_frame_shared`:

1. **write lock**: reconcile instances, `begin_play`, queued event handlers;
2. **read lock**: the *read stage* — read-only instances run concurrently
   (`Host::read_only`, `&World`), threads scaled to the instance count; the
   renderer's snapshot readers share this lock;
3. **write lock**: every other instance, then the spawns/destroys scripts queued.

Measured (`script_phase_cost_by_instance_count`, release, 200 loop iterations
per instance, 32-thread machine):

| instances | 1 thread | threaded |
|---:|---:|---:|
| 1 000 | 9.1 ms | 2.7 ms |
| 5 000 | 52 ms | 9.1 ms |
| 20 000 | 194 ms | 37.6 ms |

and none of the read stage is under the exclusive lock any more. The loop
records it: `ScriptStats::write_lock_hold_last_us`, `…_max_us`,
`write_lock_wait_max_us`, `read_stage_*`, and how many instances ran in each
stage.

What the split changes, on purpose:

- Read-only instances tick before the others in a frame instead of interleaved
  in spawn order; they cannot observe one another, so only their order relative
  to writers moves.
- An instance with breakpoints (or stopped at one) runs in the write stage,
  where the debugger is wired.
- Read-only classes cannot publish events (those natives write); their event
  handlers still run, in the first write stage.
- A write native reached in a read-only host (a mis-declared native) fails the
  call: *"this native changes the world, but the script is running in the
  read-only phase"*.

Writers still hold the exclusive lock for the whole write stage. Narrowing that
further means making component writes deferred commands with read-your-writes,
which changes what a script observes mid-call; it is not done here.

## Equality and printing for value types (#859)

`script_value_ops!(Vec3, "Vec3", eq = |a, b| a == b, display = |v| v.to_string())`
registers hooks next to `script_value_type!`. `==`, `!=` and string conversion
are then allowed on that type and on lists, maps and tuples of it; a module that
uses them on a type without hooks is refused at link time, naming the function
and instruction. Component references compare by identity (entity and component)
and print as `Health@<entity>`. The math types have hooks (exact float equality).
