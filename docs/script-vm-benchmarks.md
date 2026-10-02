# Script VM benchmarks (#853)

Measure first, then optimize. Two harnesses, both dependency-free
(`harness = false`), both take `--quick` for a fast pass:

```
cargo bench -p pulsar_script_vm --bench vm              # microbenchmarks
cargo bench -p pulsar_world_registry --bench script_tick # 10k-instance macro
```

## Microbenchmarks (`pulsar_script_vm/benches/vm.rs`)

Release build, best of 15 runs of a 100 000-iteration loop (depth: 500 × 200
frames), the interpreter only. "Before" is the tree prior to this change.

| case | before | after | per instr (after) |
|---|---:|---:|---:|
| empty loop (test, branch, add, jump) | 21.5 ns | 15.8 ns | 3.9 ns |
| arithmetic, 3 adds | 47.1 ns | 32.6 ns | 4.7 ns |
| variable load/store | 42.4 ns | 32.0 ns | 4.6 ns |
| call (leaf, 1 arg) | 85.8 ns | 50.1 ns | 6.3 ns |
| call depth 200 (per chain of 201 frames) | 19.2 µs | 12.2 µs | 8.6 ns |
| native call (no args, `entity::none`) | 42.3 ns | 35.3 ns | 7.1 ns |
| list push (copy-on-write, in place) | 62.2 ns | 60.8 ns | 12.2 ns |

## Macrobenchmark (`pulsar_world_registry/benches/script_tick.rs`)

N instances, each running `tick(me)`: `Transform::of(me)`, read `position`,
write it back — three native calls against the world — vs. the same work
written directly in Rust against `World`.

| instances | direct | script | ratio |
|---:|---:|---:|---:|
| 1 000 | 22 ns/instance | 366 ns/instance | 16.6× |
| 10 000 | 22 ns/instance | 358 ns/instance | 16.4× |

10 000 scripted instances cost about 3.6 ms of a frame, linear in the count.
About 120 ns of that is per reflected native (lookup, argument marshalling,
the world access), not per bytecode instruction: the next win is in the
natives, not the dispatch loop.

## Candidate optimizations: what the measurements said

- **Source-location lookup on every instruction** — not on the issue's list, but
  the largest: `function.location(pc).cloned()` ran for every instruction even
  with no debugger attached. Now looked up only when a debugger is attached.
- **Allocate `Vec<Value>` per call** — `Call` allocated one per call. Now copies
  registers frame-to-frame (leaf call 85.8 → 50.1 ns together with the above).
  `CallNative`/`Collection` already reused one scratch buffer, so the native-
  argument allocation the issue lists was already gone.
- **Specialize `Binary`** — done where it matters without a second opcode set:
  `binary_scalar` inlines the unchecked int/float arithmetic and the numeric
  comparisons into the dispatch loop and falls back to `binary` for everything
  else (checked arithmetic, division, strings). Same results by construction;
  the conformance harness still runs every fixture on both backends.
- **`catch_unwind` per native** — measured by removing it: ~3.5 ns of a 35 ns
  native call (10%). Grouping it per tick would let a panicking native leave the
  VM mid-instruction with its register stack in an arbitrary state, so a panic
  would have to abandon the whole tick for every instance. Not worth 3.5 ns;
  kept per native.
- **Register reset per call** — `push_frame` clones the function's typed default
  registers (a handful of small enum values; no heap). Not measurable against
  the frame push itself in the call-depth case; left alone.
- **Pre-resolved imports** — `Program::natives` already holds the resolved
  `NativeFn`s; `CallNative` is an index, no lookup.

Not done: the headless benchmark table lives in the Helio submodule next to the
renderer's numbers; these tables are the content to add there.
