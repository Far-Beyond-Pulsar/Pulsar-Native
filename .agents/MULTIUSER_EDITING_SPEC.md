# Multiuser Editing: Production Specification

Status: architecture and implementation plan, based on the current Pulsar-Native
codebase as of 2026-10-15. This replaces the earlier research note. It describes
the target behavior and code work; it does not claim that the features below
are implemented.

## 1. Product contract

Multiuser editing must feel like one editor session while preserving one
authoritative result for every shared document. The same editing behavior must
work in both product modes:

- **Studio:** a dedicated service owns the remote workspace files and is the
  collaboration authority.
- **Peer to peer:** editors discover and connect to one another for the free
  session path. A designated host peer is the authority for the lifetime of a
  session term. Relay is used for rendezvous and, when needed, data fallback.

These modes share the document, operation, presence, and replication semantics.
Only their authentication, authority location, file provider, and transport
adapter differ. The repository models `CloudProject` and `PeerToPeer`, and has
remote and P2P filesystem providers, but this does not mean the current P2P data
path is production-ready: its networking implementation still has explicit
transport/NAT/proxy TODOs. “Free” is product policy, not something the source
code establishes.

### Required user-visible behavior

1. A user can opt a data-bound UI field into collaboration with
   `.replicates(true)` and turn it off with `.replicates(false)`. The API has a
   richer policy form for locks, host-only changes, and presence-only state.
2. The framework identifies the field's shared model target. It sends edit
   intents and transient cursor/focus presence; it does not make a widget's
   local GPUI state the source of truth.
3. An accepted scene edit updates the local SceneDB, renderer, subscriptions,
   editor panels, dirty state, and undo metadata exactly once. Other editors
   apply that accepted change to their own SceneDB without generating another
   outbound edit.
4. Text documents support simultaneous typing, remote selections/carets,
   reconnect, and undo through a text-specific CRDT/OT adapter. Replacing the
   complete string on every keystroke is not an acceptable implementation.
5. One workspace session can have several level documents open at the same
   time. Every edit, presence event, snapshot, revision, and resync request is
   routed to the correct document.
6. Local layout, tool configuration, scroll, camera, and window state remain
   local by default. The UI may show another user's cursor, active field,
   selected object, or viewport pointer as presence without changing the
   receiver's personal UI state.

## 2. Verified architecture and consequences

### Multiple open levels mean multiple SceneDB worlds

The editor creates a new `LevelEditorPanel` and `LevelEditorState` for each
opened level file. Each `SceneDomain` owns a distinct `SharedScene` and
`SceneDb`; its renderer shares that exact scene handle. The file-opening path
activates an already open matching path in the current dock before creating a
new panel. There is an explicit test for two open levels whose class instances
are rebuilt independently.

Sources: `crates/editor/ui_core/src/builtin_editors.rs` (level editor provider),
`crates/editor/ui_core/src/app/tab_management.rs` (open-path reuse),
`crates/editor/ui_level_editor/src/ui/panel/lifecycle.rs` (panel/scene
construction), `crates/editor/ui_level_editor/src/state/scene.rs`
(`SceneDomain`), `crates/core/engine_backend/src/scene/mod.rs` (`SharedScene`),
and `crates/editor/ui_level_editor/src/core/scene_edit/tests/classes.rs`
(two-open-level coverage).

The network model is therefore **one workspace session containing many
document sessions**, not one shared SceneDB for the workspace:

```text
Workspace collaboration session
  ├─ Level document A → SceneDB A → renderer/views for A
  ├─ Level document B → SceneDB B → renderer/views for B
  ├─ Text document C  → CRDT buffer C → editor views for C
  └─ ephemeral workspace/session presence
```

Never route by active tab, window, `Arc` address, SceneDB `Entity`, or the
currently selected object. Every document operation carries a document ID;
every scene target is scoped by that document ID.

### In-place level replacement is a document transition

The Level Editor's **Open Scene** and **New Scene** actions clear/reload the
current `World` in place to keep the renderer's `SharedScene` `Arc` alive. This
means the Arc can remain identical while its logical document changes. The
collaboration layer must detach the previous document's outbox/subscription
before the clear, classify replacement writes as `DocumentSwitch`, and join or
create the new document only after a successful load. A failed load must leave
the prior scene and its network binding intact; this calls for loading and
validating a candidate state before committing the in-place replacement.

Save As is also a document transition: it creates a new document identity from
the current accepted state. Other collaborators remain attached to the source
document. A path rename is an explicit rekey operation. An unsaved new level
gets a host/session-issued temporary document ID and becomes path-backed on
save.

Sources: `crates/editor/ui_level_editor/src/ui/panel/handlers/scene.rs`,
`crates/editor/ui_level_editor/src/ui/panel/lifecycle.rs`, and
`crates/editor/ui_level_editor/src/state/scene.rs::set_current_scene`.

### Current identities

- `pulsar_scene_model::StableId` survives save/load and is unique within one
  world. SceneDB `Entity` bits are local to one live `World`.
- `ComponentInstanceId` is a persistent random 128-bit ID for an attached
  component instance. Presentation list order is not identity.
- The level file currently has version, created/modified/editor-version
  metadata, but no persistent level UUID.
- `DocumentId::for_path` canonicalizes filesystem paths for undo history; UI
  tab activation currently compares paths directly. Collaboration must use a
  VFS/project-relative canonical path so Studio URIs and P2P project roots
  converge on one identity.

Required wire identity:

```text
WorkspaceId     = Studio workspace UUID or P2P project/session project ID
DocumentId      = normalized project-relative VFS path + document kind
                = session-issued UUID for a never-saved document
SceneObjectRef  = { DocumentId, StableId }
ComponentRef    = { DocumentId, StableId, ComponentInstanceId }
FieldRef        = { ComponentRef or core-object ref, stable FieldSchemaId }
```

Use a versioned canonical path key for existing levels, and add an optional
persistent document UUID to the level metadata when file-format migration is
ready. The path remains the locator; the UUID preserves identity across moves.
For legacy levels with no UUID, the authority deterministically derives the
initial ID from workspace ID, document kind, and normalized VFS path, then
assigns/persists a UUID as an explicit document-rekey transaction. Never let
each client independently invent an ID. The same canonicalization rules must
handle case sensitivity, separators, symlinks/aliases, remote paths, and
files outside the workspace. Files outside the workspace cannot join a
workspace session until explicitly imported or shared.

### Existing collaboration code is a prototype, not the target architecture

The repository currently has separate layers: WGPUI `ui::replication`,
`pulsar-multiplayer-core`, engine Studio WebSocket state patches, and P2P
signaling/file transfer. WGPUI has replication modes, presence, a `Replicator`
trait, and an `Entity::with_replication` extension. Its current state path
serializes whole UI state as JSON with timestamps; OT/CRDT labels do not
implement those merge algorithms. Existing `InputState` integration can replace
whole remote strings. Studio and P2P do not yet share one typed collaboration
data protocol.

Use these systems as reusable pieces and migration points. Do not expose raw
JSON widget state or raw SceneDB Delta bytes as the production edit API.

## 3. State ownership and replication policy

Every value must be classified before it is synchronized:

| State class | Examples | Wire behavior | Authoritative storage |
|---|---|---|---|
| Shared authored scene data | object name/transform/hierarchy, component fields, world settings, foliage authoring data | typed edit intent; accepted ordered scene transaction | existing SceneDB and level document |
| Shared authored document data | code, shader source, blueprint graph, table/query document | domain-specific operations/CRDT or OT | shared document model / file-backed document |
| Ephemeral presence | pointer, field focus, object selection, remote caret/selection, viewport pointer, active document | lossy/latest-value presence lane; expires on timeout | session memory only |
| Personal editor state | dock layout, panel size, local tool mode, scroll, camera, local undo UI, theme | no collaboration traffic by default | existing local editor/layout state |
| Derived runtime state | GPU mirror rows, renderer caches, physics caches, compiled scripts | recompute/update from accepted source data | engine subsystems; never sent as authored memory |

`Selected` currently exists as a SceneDB marker. It is personal selection, so
exclude it from authored replication and level snapshots; share a stable object
reference through presence instead. Review any other editor-only components
the same way. Per-level `world_settings` and foliage authoring values are
shared authored data. Editor camera currently serializes with level editor
data, but stays participant-local in a session; its save ownership must be
made explicit so one collaborator cannot silently overwrite another's camera.

### Framework API

The framework API is intentionally small:

```rust
TextInput::new(&bound_field).replicates(true)
number_input(bound_field).replicates(true)
```

`replicates(true)` is a builder extension on a **semantic field binding**, not
on an arbitrary visual `div`. The binding already knows its
`WorkspaceId/DocumentId/FieldRef`, type codec, validation, local setter, and
change subscription. Reflection-generated inspector rows receive their field
policy from the reflected schema and need no per-control network code. The
builder call is for hand-built bound controls; an unbound element using
`.replicates(true)` must fail fast in development and show a diagnosable error
in production rather than silently synchronizing an unstable element ID.

The bool is shorthand. The actual policy type must support:

```rust
.replication(ReplicationPolicy::Shared { conflict: OrderedLastWriterWins })
.replication(ReplicationPolicy::Exclusive { lease: ... })
.replication(ReplicationPolicy::HostOnly)
.replication(ReplicationPolicy::PresenceOnly)
.replicates(false)
```

An element may override an inherited field policy for a specific view, but it
cannot grant permission. Server/host policy is checked for every operation.
The local widget state remains local; accepted model changes arrive through
the binding's ordinary subscription. Presence decorations are rendered by the
framework using participant colors and are keyed by stable field/document
identity, not by GPUI entity IDs.

### Binding and schema requirements

Add a stable `FieldSchemaId` to reflection metadata. It must be derived from a
stable schema name/version or explicitly assigned, never from Rust field
position, `TypeId`, `ComponentId`, or build-specific numeric registry order.
Schema registration declares:

- replicated or local-only default;
- wire codec and schema version;
- value size/range/enum validation;
- authority policy and conflict strategy;
- whether a value is a scalar field, structural operation, or specialized
  document requiring an adapter.

Core scene identity fields and authoring fields opt in by schema. Runtime-only,
GPU-only, transient, and editor-local fields default to local. Unknown plugin
fields are rejected or explicitly treated as opaque, bounded, versioned data;
they are never reinterpreted as arbitrary memory.

## 4. One collaboration session, many document streams

Create one `CollaborationSession` per workspace/project in engine state. It
owns the authenticated session identity, transport, local participant identity,
role, current connection term, schema negotiation, presence registry, and a
map of `DocumentId → DocumentSession`.

Each `DocumentSession` owns:

- document identity and kind;
- independent ordered revision/sequence and last applied sequence;
- per-document schema fingerprint and authorization context;
- scene stable-ID resolver when it is a level;
- exactly one outbox/change-cursor owner for that World;
- bounded pending operation queue and per-document snapshot/replay state;
- view subscriptions/reference count, not UI layout state.

Opening a level subscribes to its document stream and requests a snapshot plus
replay from a known sequence. Opening another level creates/attaches a second
`DocumentSession`; it cannot replace a singleton `active_scene` field in
`MultiuserContext`. The active level is presence only. Closing a tab removes
that view's reference; it does not tear down a document still used by another
view. Closing the last local view flushes accepted local work, unsubscribes,
and releases the replica. If the same document is opened from another window,
the workspace-level document registry must share one `SharedScene`/replica or
refuse duplicate independent replicas. Path aliases must resolve to the same
registry key.

The runtime `RuntimeLevel::load_into` path can merge multiple level files into
one runtime World. That runtime World is a distinct scope from authored editor
documents; it must not collapse the identity of the source level documents.
Where a runtime collaboration feature is later added, it needs its own
document/session scope and source-level namespace.

## 5. Wire protocol and deployment modes

### Common typed envelope

Every data message has a bounded, versioned envelope:

```text
protocol_version, session_id, workspace_id, document_id?,
connection_term, authenticated_sender, operation_id?,
document_sequence?, base_revision?, schema_fingerprint,
message_kind, bounded_payload
```

The sender identity is assigned by the authenticated service/host. Do not
trust a client-provided `peer_id`. Sequence numbers and operation IDs make
duplicates, gaps, and retries detectable. Each document has an independent
sequence stream so edits in one open level do not block another.

Use four logical lanes over one transport abstraction:

1. **Control:** reliable ordered join/leave, handshake, schema negotiation,
   roles, locks, snapshot requests, acknowledgements, rekey, errors.
2. **Scene transactions:** reliable and ordered per document; bounded binary
   typed operations or accepted state deltas.
3. **Text operations:** reliable causal CRDT/OT updates per text document.
4. **Presence/previews:** unreliable latest-value state with rate limits and
   expiration; loss cannot corrupt document data.

Large snapshots are chunked, size-limited, checksummed, and applied only after
all chunks validate. Compression is for snapshots/batches, not a substitute for
limits. Define limits for frame bytes, operations/frame, touched entities,
field bytes, queue depth, snapshot bytes, decompression ratio, and per-peer
rates.

### Studio mode

Studio authenticates the user/workspace, owns remote workspace files, assigns
the session/participant identity, validates permissions, sequences accepted
edits, and stores/checkpoints documents according to service policy. The client
uses the Studio session transport; `RemoteFsProvider` remains file I/O and is
not the collaboration protocol. When reconnecting, the service returns the
document's checkpoint and operations after the last acknowledged sequence, or
requires a fresh snapshot.

### P2P mode

Rendezvous authenticates the invite/session and discovers peers. The creator
is the authority for a term and validates/sequences edits. Data uses direct
peer transport when available and relay fallback otherwise. The relay is not
the scene authority unless product architecture later explicitly moves that
role there. If the host disappears, the safe v1 behavior is to freeze writes
and end/rejoin from a checkpoint; host migration requires a new authority term
and a checkpoint acknowledged by the participants. Never let peers silently
become independent authorities.

Both adapters implement the same `CollaborationTransport` and protocol. A
P2P filesystem channel transfers files; it does not by itself provide durable
document ordering, edit authority, or reconnect semantics.

## 6. Scene-state edit and replication pipeline

### Local edit to accepted state

1. A bound widget, gizmo, hierarchy operation, command, script, AI tool,
   importer, or plugin creates a typed `EditIntent` with document ID, stable
   target, stable field/operation ID, base revision, operation ID, and payload.
2. The document session authenticates the sender and submits it to Studio or
   the P2P authority. It enforces per-field policy, structural permissions,
   schema compatibility, expected revision/preconditions, value validation,
   asset/reference validity, and quotas.
3. The authority sequences accepted operations. It applies the operation once
   to its document World and records author, label, affected stable IDs,
   before/after or inverse information, sequence, and resulting canonical
   state. Invalid operations are rejected with a typed reason and leave the
   World untouched.
4. The authority broadcasts an `AcceptedSceneTransaction` to every replica,
   including the author. The author does not independently commit the same
   accepted transaction a second time. A short-lived local preview can provide
   immediate drag feedback while awaiting authority.
5. Each replica validates the accepted envelope and applies it atomically to
   its own existing World with `MutationOrigin::Remote { peer, sequence }`.
   Normal SceneDB write behavior must still update GPU mirrors, object feeds,
   journals, lifecycle hooks, renderer pacing, dirty state, and UI observers.
   Remote origin suppresses only collaboration outbox emission, not local
   engine observers.
6. The replica acknowledges the sequence. On duplicate it acknowledges
   without reapplying; on a gap, schema mismatch, or base-revision mismatch it
   stops applying that document stream and requests replay/snapshot.

For frequent gizmo drags, use a short lease and a transient preview stream
(coalesced to a bounded frame rate). The preview is not durable scene state.
On release, submit one canonical transform transaction; the accepted operation
updates SceneDB and is one undo entry. If product requires continuously
committed transforms, use sequenced accepted updates under the same lease and
coalesce them into one undo group. Never broadcast unvalidated raw World
memory.

### Change capture and direct mutation paths

`SceneCommand` is useful for labels, user intent, validation, and undo, but it
does not cover all mutations. Gizmos, voxel tools, scripts, import/reimport,
AI, plugins, and other operations write directly to SceneDB. The production
capture point must therefore be a SceneDB/engine transaction boundary that
captures every authored World mutation, plus explicit transaction scopes for
multi-write operations. Do not require every editor control to hand-build
network messages.

Each `World` currently has a `SharedChangeTracker` attached, while
`end_change_window` drains/discards its pending data once per frame. Add a
non-destructive independent cursor/fan-out or a single explicit owner that
publishes captured changes to all consumers. Do not add a second destructive
drain. Capture semantic stable IDs and field values, not only component event
kinds. Coalesce high-frequency field changes at the transaction/frame boundary
and keep scene state out of whole-world scans.

### SceneDB Delta use and remote apply safety

SceneDB Delta is a useful compact field codec/transport primitive, not a
trusted client-intent API. The pinned `Delta::apply` accepts no sender/authority
context; uses wire entity bits; can mutate before all values validate; has no
complete frame/replay/schema checks; and remote structural changes can miss
normal GPU/lifecycle behavior or echo through tracking. Its parser has useful
truncation and typed-value defenses, but it is not a secure network ingress.

The engine adapter should initially translate accepted transactions through
stable IDs. Reuse SceneDB's registered field codecs where they pass the schema
and limits checks. Only use native `Delta` on the wire when the SceneDB layer
adds a validated document-local stable-ID mapping, strict framing/limits,
preflight decode, atomic apply, remote origin, and normal GPU/observer/lifecycle
parity. Never resolve a client's raw entity index as an authorization check.

Snapshots are persisted typed level state plus document metadata and version,
not process memory or GPU data. They include scene objects, hierarchy,
registered component instances, unresolved payloads under bounded opaque
handling, and authored per-level settings. They exclude local selection,
transient presence, renderer/GPU rows, and personal editor state. Snapshot
installation validates completely before replacing the replica World.

## 7. UI-state and text pipeline

### Data-bound controls

The UI framework's `ReplicationBinding` carries a stable `FieldRef`, read/write
subscription, policy, validation metadata, and a presence anchor. On local
input it emits a typed intent. On accepted remote scene state it updates the
control through the ordinary model observer with a remote origin. Focus,
caret, and selection updates travel separately on the lossy presence lane and
are decorated as remote UI affordances. They do not write SceneDB.

For scalar text fields such as object names, use the field's declared policy
(ordered scalar writes or an exclusive lease) and send debounced/transactional
value edits. For a text document, use a shared buffer with CRDT/OT operations;
keep IME composition local until committed, map remote carets through document
operations, and never overwrite another user's concurrent insertions with a
whole-string assignment.

### Editor-only state

Keep `.pulsar/layout.json`, docking, open-window geometry, local zoom, local
scroll, local camera, and personal tool settings per user. Use presence to show
the active document, selected object, focused shared field, cursor, and optional
viewport pointer. Presence is namespaced by `DocumentId`, expires after a
heartbeat timeout, and is not persisted or used for authorization.

Blueprint/shader/graph/table editors have authored models beyond simple
fields. Their adapter submits semantic graph/table transactions or a shared
text-document CRDT. It does not serialize their entire GPUI entity tree.

## 8. Conflict, locking, undo, save, and reconnect

- Scalar fields default to authority-ordered last accepted write, with the
  accepted sequence visible to clients. Fields that cannot tolerate this
  declare an exclusive lease or custom merge adapter.
- Structural operations (create/delete/reparent/attach/detach) are atomic and
  validate references and parent cycles at authority. Multi-object commands
  either commit fully or reject fully.
- A gizmo drag can acquire an expiring object/field lease. Leases are
  authority-owned, renewed, released on completion, and cleaned up on peer
  timeout. UI lock display is informative; authority enforcement is mandatory.
- Undo is a new compensating transaction against current document state, with
  author attribution and conflict checks. Do not synchronize local snapshot
  history stacks or restore an old scene snapshot over collaborators' newer
  edits. Group previews/drag updates into one shared undo unit.
- The authoritative document stream is the source for dirty/save state. Studio
  saves accepted checkpoints; P2P host saves the shared file and distributes
  save success/version. Prevent clients from independently writing stale
  copies over accepted shared state. Local layout/camera metadata uses per-user
  storage during collaboration.
- Reconnect resumes from `(DocumentId, last_applied_sequence)`. The authority
  replays a bounded retained log or sends a validated snapshot plus later
  operations. A peer with unacknowledged local intents retries with the same
  operation IDs. Offline divergent scene editing/merge is out of v1 scope;
  offline edits are held as pending and must be accepted/rejected on reconnect,
  never silently merged as if authoritative.

## 9. Security and resource limits

The wire carries serialized typed values, not RAM dumps. Even SceneDB's `Pod`
encoding is a bounded field value with a registered schema; never deserialize
arbitrary bytes into a Rust object or `World` allocation.

Before mutation, validate authenticated session/term/sender; document
membership; authority/role and field policy; protocol and schema fingerprints;
sequence/base revision; stable target resolution; enum/tag ranges; exact frame
consumption; field and collection lengths; and per-session rate/work quotas.
Predecode all transaction values before touching the World, then commit
atomically. Bound decompressed snapshots and queues. Treat custom/plugin
codecs as trusted executable code, but catch failures at the network boundary
and prevent panic from unwinding through session tasks. Log operation IDs and
rejection reasons without logging sensitive field payloads by default.

Transport must authenticate and encrypt Studio and P2P sessions. Encryption
does not replace operation authorization. P2P host identity, connection term,
host transfer, and replay protection must be explicit. Unknown plugin schema
versions fail closed for required state and preserve only explicitly bounded
opaque records.

## 10. Code plan

Implementation proceeds in vertical slices. Keep each slice buildable and
behind a collaboration feature flag until its acceptance gate passes.

### Phase 0 — document identity and multi-scene lifecycle

**Change:** Add canonical `WorkspaceId`/`DocumentId` types and a workspace-level
`SceneDocumentRegistry`. Bind each Level Editor panel/view to a document
session, not to an implicit singleton/current tab. Make `OpenScene`, `NewScene`,
Save As, rename, duplicate open, close, and reload explicit detach/attach
transitions. Parse/load into a candidate state and commit only on success.

**Likely files:**

- `crates/core/pulsar-multiplayer-core/src/session.rs` (IDs or shared ID
  module), `crates/core/engine_state/src/multiuser.rs` (session ownership),
  and a new `crates/core/engine_collaboration/` crate if keeping the document
  registry out of transport types.
- `crates/editor/ui_level_editor/src/state/scene.rs` and
  `ui/panel/lifecycle.rs` / `ui/panel/handlers/scene.rs` for document binding
  transitions and load replacement.
- `crates/editor/ui_core/src/app/tab_management.rs` and the project VFS path
  layer for canonical open-document identity and duplicate views.
- `crates/editor/ui_level_editor/src/core/scene_edit/mod.rs` and
  `level_io.rs` for optional persisted document UUID migration.

**Gate:** Two different levels open simultaneously map to two independently
addressable `DocumentSession`s and two SceneDBs. Opening another level in-place
routes to its ID; a failed load leaves the old one attached. Same-file aliases
cannot create two independent writable replicas. A Save As creates a new
document while other peers remain on the original.

### Phase 1 — typed protocol and authority

**Change:** Extend `pulsar-multiplayer-core` with the versioned envelope,
document channels, typed scene intents/accepted transactions, text op framing,
presence, acknowledgements, operation IDs, per-document sequence, reconnect
and snapshot messages. Add validation/limits before protocol decode and a
transport-neutral `CollaborationTransport` API. Keep file transfer messages
separate.

**Likely files:**

- `crates/core/pulsar-multiplayer-core/src/protocol.rs`, `transport.rs`,
  `session.rs`, plus new `collaboration/{ids,envelope,scene,text,presence,
  limits}.rs`.
- `crates/core/engine_state/src/multiuser.rs` or a dedicated session resource
  for connection term, role, workspace membership, and document map handle.

**Gate:** Deterministic serialization/version rejection; oversized, malformed,
unknown-tag, duplicate, stale, reordered, and cross-document packets are
rejected before any editor or World state changes.

### Phase 2 — authoritative scene transaction boundary

**Change:** Add `MutationOrigin` and a semantic scene transaction capture/apply
API that covers field writes and structural changes. Use stable IDs for wire
targets and host-assigned object/component IDs for creation. Preflight then
commit atomically. Preserve normal SceneDB GPU mirrors, journals, object feeds,
asset/lifecycle notifications, and renderer frame pacing. Remote accepted
transactions do not enter the local network outbox. Fan out changes without
stealing/draining the existing renderer tracker.

**Likely files:**

- SceneDB pinned dependency: `replication.rs` and `world.rs` for validated
  transaction apply, origin tracking, checked codec bounds, stable reference
  mapping, and no-echo semantics. If an upstream dependency change is required,
  make it in the SceneDB repository, merge dependency-first, then update the
  Cargo pin per `.agents/CRATES.md`.
- `crates/core/engine_backend/src/scene/mod.rs` and new
  `scene/collaboration/{capture,apply,identity,snapshot}.rs` for per-World
  adapter and resolver.
- `crates/editor/ui_level_editor/src/core/commands/` and direct mutation
  surfaces for explicit transaction labels/grouping; SceneDB capture remains
  the coverage backstop for scripts/plugins/tools.
- Reflection/registration in `crates/core/pulsar_reflection`,
  `pulsar_world_registry`, and `engine_class_derive` for stable field schema
  IDs, opt-in policy, codec/version/validation metadata.

**Gate:** Randomized local Entity allocation yields identical stable-target
results; malformed batch has zero partial effects; remote field/object changes
refresh subscriptions and GPU mirrors; despawn invokes lifecycle cleanup;
remote apply emits zero outbound operations; journal overflow triggers only
that document's resnapshot.

### Phase 3 — UI framework binding and presence

**Change:** Replace whole-JSON widget replication with semantic
`ReplicationBinding`s and `.replicates(bool)` / `.replication(policy)` builder
methods for supported input controls. Reflection-generated fields inherit
schema policies. Add remote focus/cursor/selection decorations, identity
validation, and origin-aware observer updates. Add per-document UI presence
registration; retain local layout state.

**Likely files:**

- `crates/ui/wgpui-component/crates/ui/src/replication.rs` (replace/reshape
  modes/registry/trait), input state and input elements (`TextInput`, number,
  checkbox, enum/select, picker controls), and new `collaboration/binding.rs`.
- `crates/editor/ui_multiplayer/src/sync_protocol.rs` and `peer_state.rs` for
  the session bridge, replacing UI JSON state transport with typed protocol
  messages.
- `crates/editor/ui_multiuser_status/` for session participants and
  per-document presence presentation.
- `crates/editor/ui_level_editor/src/ui/properties/bindings/` for the shared
  bound-field adapter; ordinary reflection-generated controls should require
  no per-control protocol code.

**Gate:** `TextInput::new(&bound_field).replicates(true)` produces an edit
intent and transient presence; accepted remote changes update the control once
without echo. Unbound visual elements cannot accidentally enter the shared
stream. A collaboration field's permission cannot be elevated by client UI.

### Phase 4 — Studio and P2P adapters

**Change:** Implement the same protocol over the Studio session WebSocket and
P2P `SessionChannel` data path. Studio service is authority and checkpoint
owner. P2P creator is host authority, with rendezvous, direct transport, relay
fallback, role/term validation, and a safe host-loss behavior. Do not use
`RemoteFsProvider` or P2P file chunks as a substitute for live edits.

**Likely files:**

- `crates/core/engine_backend/src/subsystems/networking/multiuser.rs` and new
  `networking/collaboration/{studio,p2p,dispatch}.rs`.
- `crates/core/engine_backend/src/subsystems/networking/p2p.rs` and
  `crates/core/pulsar-relay/` for data relay/transport completion and
  rendezvous/auth integration.
- Studio service counterpart for server-side authorization, ordered accepted
  operations, snapshots/log retention, and workspace file checkpointing.
- `crates/core/engine_fs/src/providers/{remote,p2p}.rs` only for consistent
  document path/identity and save-provider coordination.

**Gate:** The same collaboration conformance suite passes for both transports;
direct P2P and relay fallback produce identical ordered document state; host
loss cannot create split-brain; Studio reconnect recovers the authoritative
checkpoint and accepted log.

### Phase 5 — text documents and specialized adapters

**Change:** Integrate a text CRDT/OT buffer behind WGPUI text/code editors and
semantic adapters for authored graph/table editors. Keep IME composition,
scroll, and editor chrome local. Map carets and selections through operations.

**Likely files:** WGPUI `InputState`/`CodeEditor` and
`crates/editor/plugin_typescript/`; relevant vendored `shader_editor`,
`blueprint_editor`, `table_editor`, and other authored-data editors. Add an
optional plugin editor collaboration adapter API in
`crates/core/plugin_editor_api/`.

**Gate:** Concurrent insert/delete tests converge under delay/reorder/reconnect;
remote carets remain attached to intended text; local undo only compensates
the initiating user's operations; no whole-buffer replacement erases peer
work.

### Phase 6 — operations, recovery, and market readiness

**Change:** Add observability, bounded backpressure, reconnect replay/snapshot,
schema compatibility/migration, collaborative undo, crash recovery, graceful
session shutdown, and support diagnostics. Define offline edit behavior and
save ownership. Add compatibility testing across supported engine/plugin
schema versions.

**Gate:** Load/reconnect/recovery and adversarial scenario suite passes for
multiple open documents, Studio and P2P, late join, stale/replayed/gapped
messages, peer timeout, conflicting deletes/reparents, schema mismatch,
unknown plugins, asset references, undo races, and document switch during
pending operations.

## 11. Test and acceptance matrix

Required automated coverage before general availability:

| Area | Cases that must pass |
|---|---|
| Multi-document routing | Two or more levels open; interleaved edits route only to the matching SceneDB; presence scoped to the right level; switching active tabs changes only local focus |
| Lifecycle | Open/New/Reload/Save As/rename/close; failed load preserves old binding; same path alias resolves to one document; pending ops are drained or retained safely |
| Identity | Different local SceneDB entity allocation; StableId and ComponentInstanceId resolution; spawned IDs supplied by authority; collisions and missing refs rejected |
| UI binding | `.replicates(true/false)`; schema-inherited field policy; focus/cursor presence; remote observer update without control echo; invalid/unbound API use is diagnosable |
| Scene apply | Scalar, transform stream/commit, object create/delete, hierarchy/reparent, component attach/detach/enable, unresolved data, world settings; atomic failure behavior |
| SceneDB invariants | GPU mirror updates; object subscriptions update; lifecycle cleanup; renderer revisions; tracker fanout; no remote echo |
| Protocol/security | Authentication, role/field authorization, schema/protocol mismatch, replay, gaps, duplicates, malformed tags/lengths, oversized/decompression bomb, custom codec failure, rate limits |
| Text | Simultaneous insert/delete, IME, remote selection, reconnect, compaction/checkpoint, undo/redo attribution |
| Session modes | Equivalent behavior on Studio service, direct P2P, and relay fallback; host departure/term change; late join/snapshot recovery |
| Save/undo | Stale-save prevention; shared checkpoint; compensating undo with conflict; Save As forks rather than moves other collaborators |

Performance targets should be set with representative large levels, but the
architecture must avoid serializing/scanning the whole World per keystroke or
frame. Coalesce scene writes; send only changed fields/operations; keep presence
small/lossy; bound all queues; and isolate each document's work so a large
snapshot for one level does not stall presence or edits in another.

## 12. Explicit v1 boundaries

V1 includes shared authored scene data, common bound fields, structural scene
operations, remote presence, collaborative text for first-party code/shader
surfaces, both transport modes, safe join/reconnect, and shared undo for
supported operations.

V1 excludes offline multi-master scene merge, arbitrary plugin code execution
on remote peers, replication of personal UI layout/camera, runtime gameplay
World synchronization, and automatic conflict-free merge of every custom
component. Plugins participate through declared schemas/adapters; undeclared
fields stay local and are visibly marked unsupported for collaboration.

## 13. Planning size

With the framework owning bindings and generic presence, existing reflection
generated fields should have near-zero editor-specific networking code. A
framework-first full implementation is estimated at **18–38k production LOC
plus 10–20k tests/fixtures**, with roughly **0–2k ordinary editor rollout LOC**
and **1–4k specialized editor/plugin adapters**. The bulk is shared session,
safe SceneDB transactions, framework bindings, text collaboration, transport
completion, recovery, and tests. This is an order-of-magnitude planning band;
the Phase 0–2 spikes should narrow it before committing to a schedule.
