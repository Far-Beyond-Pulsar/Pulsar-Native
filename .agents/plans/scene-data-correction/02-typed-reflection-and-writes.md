# Contract proposal: typed reflection and writes

Status: **proposed — review required**

## Proposed decision

Reflection describes and constructs actual component values. The live edit path never encodes a component to JSON and decodes it again. SceneDB owns the value; an object-safe reflected value wrapper exists only to cross generic editor/script/plugin APIs.

Every registered class exposes a descriptor keyed by stable schema ID with:

- Stable schema/version ID, display metadata and Rust runtime type identity.
- A default/factory constructor and a fallible typed clone operation (or an explicit non-cloneable policy).
- Checked downcast accessors and property/method metadata with typed get/set/validate operations.
- Optional generated GPU layout/mirror metadata.
- Optional persistence codec and version migrations, invoked only at an archive boundary.
- Component lifecycle/resource hooks when genuinely required, kept separate from renderer discovery.

`TypeId` is a process-local type check, never a saved identifier or plugin protocol. Stable schema ID handles files, references and versioning. The reflected erased value owns a valid concrete Rust allocation and is dropped by the registered compatible runtime. Plugin unload remains unsupported while values or function pointers from that plugin exist, consistent with the permanent-DLL pattern.

## Write transaction contract

All mutation routes converge on one SceneDB transaction API:

1. Resolve stable object/component IDs and registered schema.
2. Validate type, property path, permission/constraints, and owner/lifecycle conditions before mutation.
3. Apply an owned typed value or typed patch under a mutable SceneDB transaction.
4. Commit once. SceneDB advances revision, updates relevant GPU mirrors/dirty ranges, and publishes a committed change record atomically.
5. Return structured errors without partial mutation on failure.

Direct typed `World::insert/get_mut/remove` remain usable, but must have exactly the same lifecycle hooks. The API must prevent mutable references from escaping transaction scope. Reflection's property setter should operate on the live allocation using typed `Any` values and return an error when the reflected type does not match; it should not construct a replacement via JSON.

Compound object/component creation, class instantiation and history restore validate completely before commit or provide rollback. GPU mirror failure must be reported as a failed commit or retried by a documented recoverable upload mechanism; it must never leave an editor-visible success with stale render data.

## Typed commands and values

- `AddComponent` carries a schema ID plus an owned reflected value produced by a registered factory, or a typed default-construction request with typed initialization patches.
- `SetComponentProperty` carries a typed property value or a reflection-owned value envelope with schema-checked metadata. Do not use an unvalidated `Box<dyn Any>` detached from its expected type.
- `SetComponentData` is a typed full replacement. External agents/AI may send JSON to a boundary adapter; that adapter constructs a typed command before calling the core executor.
- Component duplication uses reflected clone on the value and creates a new instance ID. Shared resource handles remain shared where their semantics are immutable.
- Unknown plugin schemas can remain opaque serialized attachments at persistence boundaries; they are not claimed as live components and cannot be edited as known values.
- Non-cloneable values declare whether duplicate/history snapshots are unsupported, use immutable `Arc`-backed state, or provide a custom snapshot strategy. JSON is not a clone workaround.

## GPU reflection hook

Generated GPU reflection is attached to the registered schema and SceneDB component storage. Insert, replacement, nested mutation, removal, and despawn all update the reflected row through the same commit path. There is no `refresh_gpu_mirror_for_class`, render-specific mutation marker, component subscription used as a dirty signal, or list of caller reminders.

Prefer generated field-level mutation tracking or transaction-level serialization to the GPU representation. A component's custom business logic can validate and normalize data before commit, but does not discover renderer state or call into Helio. SceneDB remains independent of renderer crates.

## Registration and module ownership

Registration is idempotent and keyed by stable schema ID. Registration includes Rust `TypeId`, descriptor, SceneDB erased operations and optional `SceneStore`/GPU layout. Duplicate schema IDs or incompatible layouts fail with a diagnostic that names both registrations. Registry lifetime is explicit at engine/world startup; late plugin registration either safely extends all relevant worlds/mirrors or is rejected before instances can be created.

Generated code should provide the routine implementation. Handwritten callbacks are reserved for custom codecs, validators, resource acquisition, and domain-specific behavior. Core editor code must not use a match on component class names for insertion, cloning, property conversion or GPU refresh.

## Error behavior

Use typed errors for unregistered schema, stale IDs, ambiguous lookup, invalid owner, type mismatch, invalid property, failed validation, unsupported clone/snapshot, unsupported GPU schema, and transaction conflict. Include object/component/schema/property context. Logging and UI formatting happen above the storage API.

## REVIEW: decisions to confirm or edit

- What object-safe erased-value abstraction best fits the existing reflection crate and plugin ABI?
- Should property setters receive `Box<dyn Any + Send>` plus explicit expected type metadata, or a typed `PropertyValue` enum for supported reflected primitives/containers?
- Which components require custom validators or resource hooks, and which can use generated metadata only?
- Are non-cloneable component values allowed in scenes? What duplication/history semantics should they have?
- Should GPU mirror writes be part of transaction commit synchronously, or committed to a durable dirty journal whose failed uploads are retried before draw?
