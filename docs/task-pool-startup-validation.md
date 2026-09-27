# Task-pool readiness and packaging compatibility

The voxel integration CI exposed two independent engine validation failures.

## Packaging smoke compilation

The synthetic merge `3b08e93ea` includes the upstream `LaunchOptions.profile` field from `11735b06c`. The packaging smoke test still initialized only the older fields, so all six cargo-check/packaging jobs in run `36280254828` failed with E0063. The initializer now uses `..LaunchOptions::default()`, preserving default profiling behavior on either API version.

Local packaging smoke validation passed two tests; the fixture-regeneration utility remained ignored. The local branch API does not yet contain the new field, so cross-platform synthetic-merge confirmation remains a CI responsibility.

## Worker startup allocations

The earlier Windows run `36274650731` failed its unchanged heap test twice. Attempt 1 measured a one-time +960 bytes / +3 blocks in the idle scenario. Attempt 2 measured +114,640 bytes / +8 blocks, then a flat plateau. Other scenarios reported no growth. These results alone do not establish a leak or its cause.

An isolated process containing only `TaskPool::new(0)`, a counting allocator and a 30 ms observation window reproduced +960 bytes / +3 blocks in all 12 runs. Executing a task before sampling made all 12 windows flat. Construction previously returned before the worker entered its executor; thread-local and queue initialization happened later.

The constructor now waits for each worker to enter `smol::block_on` and poll its executor once. With that change, the original un-warmed probe reported zero additional bytes/blocks in all 12 runs. An isolated regression covering requested thread counts 0, 1 and 4 also passed. It allows deallocations and rejects positive growth; no heap-test tolerance changed.

This is startup synchronization only. It does not change scheduling policy, remove the idle yield loop, join workers on shutdown or claim overall engine memory qualification. A local copy of the frozen CI heap test is also being run against the modified engine; its final outcome must be recorded separately.

The paired Helio pointer `66100ff8` passed all nine native voxel integration tests. The [evidence archive](validation/2026-09-26-task-pool/evidence.zip) and [entry hashes](validation/2026-09-26-task-pool/manifest.json) contain the completed probes, startup regression, packaging smoke and voxel integration logs, plus the frozen heap-test source. They do not include an outcome for the still-running full heap test.
