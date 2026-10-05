# Contract proposal: implementation order and delegation

Status: **proposed — review required**

## Dependency order

The implementation plan is gated by reviewed contracts. Work can be explored in parallel, but changes that assume different data ownership or identity cannot merge independently.

1. **Close the inventory and verify feasibility.** Enumerate registered component classes, all mutation paths, notification consumers, GPU schemas/passes, crate dependency edges, workspace boundaries and reproducible failures. Validate upstream SceneDB capabilities for component entities, typed erased writes, generic mirror attachment and independent cursors.
2. **Approve authority and identity.** Decide object/component entities, IDs, owner links, multiplicity, nesting, references and transaction semantics.
3. **Approve typed reflection and component structure.** Specify factory, value ownership, typed properties, clone/snapshot, module template and error behavior.
4. **Approve renderer graph boundary.** Prove by dependency graph that only the render-graph crate names/concretely depends on pass crates; engine composition registers component bundles and the graph API only.
5. **Prove one vertical slice.** Implement one mesh and one light from editor command through typed SceneDB insertion, GPU reflection, graph-owned pass mapping, GPU join and observable frame output. No renderer subscription or CPU projection. Include add/remove/edit and panel-open/closed cases.
6. **Approve/finish GPU, notifications, archive/history and plugin contracts.** Some work can proceed alongside the vertical slice, but do not freeze public APIs until findings are incorporated.
7. **Migrate remaining producers and consumers by feature.** Every component uses the same storage/reflection lifecycle; graph-owned mappings stay in the render-graph crate; each feature follows the approved APIs and owns contract tests.
8. **Remove legacy bridges/queues and refresh architecture docs.** Delete compatibility paths in the same changes that complete their replacement; verify all ledger rows and dependency boundaries.

The user review packet is the architecture gate. No subagent should make a conflicting architecture decision implicitly through its implementation.

## Work package boundary for a subagent

Every delegated package includes:

- One named crate/module or feature and exact in-scope symbols.
- No new concrete pass dependencies outside the render-graph crate; any required graph mapping change is assigned to that crate's owner.
- Approved contract section/version and APIs it may call.
- Files it may change; explicitly named out-of-scope bridge/API files.
- Required behavior and failure semantics.
- A caller/consumer list and an integration owner.
- Tests or runtime evidence required; how to run them in that crate's workspace.
- A definition of done including diff review and any discovered scope additions.

Good parallel tasks after API approval include: inventorying one component family through producer/schema/pass; migrating one leaf feature to a settled typed transaction; adding one independent subscriber test; adding layout validation for a named GPU schema; updating one serialization migration. Poor parallel tasks include separate agents inventing the typed API, component identity, renderer join scheme or event semantics at the same time.

An agent must report assumptions and interface changes before broadening scope. Integration owner reconciles shared-file changes and runs end-to-end checks. Multiple successful unit tests do not close the vertical-slice acceptance gate.

## Work-package completion evidence

For each feature, capture:

1. Typed SceneDB value before and after operations.
2. Reflected GPU row fields, owner/component IDs, generation and dirty range.
3. Consuming pass/bindings and draw/simulation eligibility.
4. Rendered result or domain effect for supported capability.
5. Removal/disable/reload behavior.
6. Independent observer behavior, if the feature exposes state subscriptions.
7. Valid crate/workspace test command, toolchain/backend and result.

## Gate to full migration

Do not retire a bridge until its complete set of callers has moved and its replacement has direct acceptance evidence. Do not keep a bridge because a consumer had no documented owner; resolve the owner. Do not reintroduce `PendingWorldWrites` as a generic fix for missed component families. Do not batch all components into one monolithic renderer subsystem; feature modules register their own schema/capability and the shared runtime consumes those declarations.

## REVIEW: decisions to confirm or edit

- Is the typed mesh+light vertical slice the first implementation gate?
- Should the crate graph enforce the pass boundary with dependency checks in CI?
- Who owns integration and the SceneDB upstream API changes?
- Which crates are acceptable package boundaries for delegated work?
- What target GPU backends/hardware are required for end-to-end acceptance?
- What baseline performance/correctness budgets should be recorded before migration?
