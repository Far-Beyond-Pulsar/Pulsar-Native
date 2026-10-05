# Contract proposal: state notifications

Status: **proposed — review required**

## Proposed decision

The current committed SceneDB state is authoritative. A state notification tells a subscriber that relevant state may have changed and where to look. It is not the state itself and is not a renderer synchronization mechanism.

Use independent per-subscriber cursors over a bounded SceneDB change journal (or equivalent broadcast primitive). Reading one cursor never advances another. A notification identifies the commit/revision, stable entity/component ID, schema, and change kind or changed-field mask when available. Subscribers then read current typed state. Repeated writes may be coalesced for UI refresh if the subscriber contract promises state invalidation rather than every intermediate value.

Rendering does not subscribe to this mechanism. GPU dirty tracking is emitted by the transaction/write path directly. A consumer that needs every intermediate transition must use a separately named event stream with explicit retention and ordering guarantees.

## Subscribe/read protocol

1. Resolve the current world identity and open a cursor at a defined commit boundary.
2. Read a snapshot/current value and its revision in a way that closes the subscribe/read race (atomic subscribe-with-snapshot or read revision, subscribe, reread if revision advanced).
3. Poll/read changes for that cursor. Filter by entity/component/property interests without affecting other cursors.
4. On journal overflow, world replacement or stale cursor, receive an explicit `ResyncRequired`, rebuild the relevant view from current state, and resume at a fresh revision.
5. Unsubscribe deterministically when a panel, script or world is destroyed. Despawn invalidation is delivered while the referenced stable identity can still be interpreted.

The cursor owns its position; it does not own an unbounded private queue. The journal is bounded and reports missed history. UI subscribers may coalesce several commits into one redraw while retaining the latest revision. Mutation ordering remains monotonic per world.

## Subscriber ownership

- Property panels subscribe by stable object/component/property interest and independently refresh from SceneDB.
- Scripts may subscribe to state changes where appropriate. Script code requiring every transition should register for a gameplay/domain event instead.
- Asset/resource completions use their own typed lifecycle notifications and update SceneDB through a write transaction.
- Renderer/GPU upload is driven by intrinsic dirty ranges, never by cursor polling.
- Debug/logging/telemetry subscribers can attach without changing delivery to panels or scripts.

Remove `take_component_change_events()` as a production global destructive API once all callers migrate. During transition, any compatibility dispatcher must fan out to every registered consumer before acknowledging a journal range; it cannot drain once and filter for one ID. Prefer direct independent cursors to a central dispatcher.

## Keep actual events distinct

Preserve ordered gameplay events such as input, collision/contact, script lifecycle, network messages, and user actions where consumers need occurrence, order, or payload history. Give those APIs event names and explicit queue/broadcast semantics. Do not convert them to “read current state” invalidations or remove them under the general label of events.

## REVIEW: decisions to confirm or edit

- Which subscribers need every mutation, and which need only invalidate/re-read current state?
- What journal retention/overflow response is suitable for long-paused editor panels and scripts?
- Should notifications report changed fields, or only schema/entity plus revision?
- Should script state subscriptions remain supported, or should scripts use explicit gameplay events only?
- Is subscribe-with-snapshot a SceneDB primitive or a caller protocol built on revision checks?
