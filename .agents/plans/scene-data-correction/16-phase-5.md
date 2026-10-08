# Phase 5: independent observers, no repair paths

Status: **complete** (Pulsar-Native#1035), pending review. Builds on Phase 4 ([15-phase-4.md](15-phase-4.md)).

Phase 5 exit (from the plan): multiple panels and scripts observe the same commits regardless of polling order; all execution modes render the same authored scene through the same data contract; scene replacement and late attachment need no repair path.

## Decisions (approved)

These answer the `REVIEW:` questions in [04-state-notifications.md](04-state-notifications.md).

1. **Who needs every mutation.** Panels and script state watches invalidate and re-read current state; several commits between polls coalesce into one. Consumers that need every transition use gameplay or component events, which are unchanged.
2. **Retention and overflow.** SceneDB's per-type journal keeps its 64k-entry ring. A reader that falls behind gets "resync required" and rebuilds from current state.
3. **Notification content.** Entity, component and revision. No changed-field masks.
4. **Script state subscriptions.** Kept, on independent cursors, with re-read semantics.
5. **Subscribe-with-snapshot.** A caller protocol: open the cursor (watch) first, then read. SceneDB gives each `World` an identity so a cursor notices a replaced world.

## Stages

1. SceneDB: cursors bound to their `World`. Repin.
2. Panels and scripts on independent cursors; the destructive drain off every Pulsar path.
3. The forced resync deleted; lifecycle tests across undo/redo, level replacement and viewports.
4. Ledger, this record, PRs.

## Stage 1: change cursors belong to their World

SceneDB `2ac389e` (branch `claude/cool-hypatia-ict23g-phase-5`, on Phase 2's `999373e`):
- Each `World`'s journal set has a process-unique id, and every `ChangeCursor` carries the id of the world that opened it.
- `World::read_changes` given a cursor from another world rebinds the cursor to this world's newest entry and returns `ChangeRead::Overflowed` once. The reader rescans and then follows the new world. Before this, a cursor kept across a world replacement silently read the new world's journal at the old position.
- A cursor for a component type that has no journal yet starts one, instead of reporting nothing forever.
- Tests:
  - `change_journal.rs`: `a_cursor_from_another_world_rescans_once_then_follows_this_one` and `a_cursor_for_a_type_without_a_journal_starts_one`.
  - `tests/change_cursor_world_replacement.rs`: `a_cursor_survives_its_world_being_replaced`.

Pulsar pins SceneDB at `2ac389e` (root `Cargo.toml`, dependency and patch). It is not merged upstream yet, as with Phases 1 and 2.

## Stage 2: every observer reads through its own cursors

`pulsar_world_registry::ComponentWatch<K>` (`change_watch.rs`) is the one primitive.
- It watches `(entity, component)` pairs under caller keys, with one change cursor per component type.
- `poll(&World)` returns the keys whose component was inserted, written or removed since the last poll. Each key comes back once, however many commits touched it.
- An overflowed journal or a replaced world returns every key on that component type. That is the resync signal, and re-reading is the recovery.
- Reading needs only `&World`, so a panel polls under the scene's read lock.

What moved onto it:

| Observer | Before | Now |
|---|---|---|
| Properties panel (`object_type_fields`) | armed `World::subscribe_id` per card; drained the process-wide queue under the **write** lock every render, under a "single-drainer contract" | one `ComponentWatch<(class, index)>` per section, polled under a read lock; watch before reading a card's values. The single-drainer contract and the `Drop` disarm are gone. |
| Script state watches (`pulsar_script_object_model::subscribe`) | `subscribe_component` + `take_change_events_for`, which drained everything and kept one subscription's events | `ComponentRefWatch`: `watch`, `unwatch`, `changed`. A despawned target reports changed once; re-reading it is the typed `ReferenceDespawned` error. |
| Editor helpers (`scene_edit::components`) | `subscribe_component`, `unsubscribe_component`, `take_world_component_events` | `watch_component` |

Pulsar no longer calls `World::take_component_change_events` anywhere: the five tests that used it now read change cursors. SceneDB still has the API; it is Phase 6 residue.

The two existing cursor consumers had no test for the cases a cursor can hit. Both now do:
- **Script driver** (`ClassInstance` cursor): `an_overflowed_class_instance_journal_rescans` evicts an instance's placement before the driver reads it. The driver rescans and starts it. In `a_replaced_world_is_rescanned`, the shared world is replaced mid-play by a loaded level, and the next tick starts that level's instance.
- **Helio `StaticMoveWatch`**: after an overflow it attributes nothing, then carries on; after a world replacement it follows the new world from the next poll.

Observer fan-out (the acceptance matrix group):

| Case | Test |
|---|---|
| Two watchers, any read order; coalescing | `change_watch.rs` `two_watchers_see_the_same_changes_in_any_read_order`; `subscribe.rs` `two_scripts_each_see_the_change` |
| Two panels and a script over one editor edit, then a history restore | `scene_edit/tests/components.rs` `every_watcher_sees_an_edit_whichever_polls_first` |
| Initial subscription (watch, then read) | `a_change_between_watch_and_first_read_is_reported` |
| Unsubscribe; re-watch | `unwatched_keys_stop_reporting`, `rewatching_a_key_moves_it` |
| Removal / despawn | `two_watchers_…` (remove); `despawn_reports_changed_and_later_writes_are_typed_errors` |
| Overflow | `an_overflowed_journal_resyncs_its_keys`; the driver and static-move tests above |
| World replacement | `a_replaced_world_resyncs_every_key_once`; SceneDB `a_cursor_survives_its_world_being_replaced` |
| Rendering with a panel watching, and with none | `render_acceptance.rs`: `panel polling first` and every other case, which has no observer |

## Stage 3: no repair path

**The forced resync is deleted.** Every scene change already reaches the renderer the ordinary way:
- edits, undo/redo (`restore_history_delta`) and opening a level (`level_io::load_from_file`) all write through the `World`;
- each write advances the world's revision;
- the renderer steps SceneDB whenever the revision moved, and that step flushes the GPU mirror.

`force_full_resync` only reset that bookkeeping. It existed for the CPU projection, which Phase 2 deleted. Removed with it:
- the mailbox flag and `queue_force_full_resync`;
- `GpuRenderer::force_full_resync` and `HelioRenderer::force_full_resync`;
- the undo/redo handler calls.

The AI tools' undo path keeps the one thing it still needs: `pending_selection_sync` points the gizmo at the restored selection.

`render_acceptance.rs` `history_level_replacement_and_viewports_need_no_resync` renders each case with no resync:
- remove, undo, redo;
- opening a level into a running editor, then replacing it with an empty level;
- two viewports on one scene, the second opened after the mesh exists. Both draw it, and both drop it after a removal, whichever renders first.

The earlier cases in that file cover:
- first frame (insertion before the renderer exists, which is late GPU attach: mirror replay);
- idle wake-up (camera at rest);
- asset completion.

**A history bug the test found, fixed.** `undo` built the redo entry from two snapshots that both lacked a removed object (`{before: current, after: delta.after}`). The redo scope never named the object, so redo did not remove it again. Likewise, undoing a redone add left the object in place. Each new entry now keeps the original snapshot on the side it does not restore, so its ids stay in scope. `commands/tests.rs` `redo_and_undo_keep_working_after_a_round_trip` covers both directions.

**Runtime parity.** The three runtimes share one seam: write through the `World`, then `SceneDb::step()` at the frame boundary, which flushes the GPU mirror Helio's joins read.

| Runtime | Steps the scene | Ends the change window |
|---|---|---|
| Editor viewport | `HelioRenderer::render_frame` | `HelioRenderer::render_frame` |
| Standalone game | `windowed_app` on redraw | `TickLoop` |
| Embedded / PIE guest | `embed.rs` after the guest tick | `TickLoop` |

None of them builds a CPU projection or keeps a renderer object cache. Script parity between standalone and PIE was already tested:
- `spawn_five_destroy_two_same_in_standalone_and_pie`;
- `a_level_without_class_instances_runs_no_scripts`;
- `pie_play_stop_play_leaves_nothing_behind`.

## Phase 5 exit

- Several panels and scripts observe the same commits, whatever order and frequency they poll in. Nothing in Pulsar drains a shared queue.
- Every runtime renders through the same SceneDB step and GPU mirror.
- Undo/redo, level replacement, a late viewport and late GPU attach need no resync, and the resync API no longer exists.
- Ledger:
  - the `destructive-drain`, `panel-drain`, `subscribe` and `force-resync` rows are removed (their sites are gone);
  - `ComponentWatch` has a verified `change-cursor` row;
  - the script driver and static-move cursors are verified.

## Left open (recorded, not done here)

- **SceneDB's destructive subscription API** (`subscribe`, `subscribe_id`, `take_component_change_events`) still exists, with no Pulsar caller. Removing it is Phase 6 compatibility residue, together with an architecture check against reintroducing it.
- **Rendered-frame parity for the standalone and embedded runtimes** is structural (the table above), not a rendered-frame test: those runtimes need a window. The editor's headless harness covers the shared seam.
- **Helio's own SceneDB pin** (`crates/renderer/helio/Cargo.toml`) is still older than the workspace's. Inside Pulsar, the patch resolves it to `2ac389e`. `StaticMoveWatch`'s replaced-world test needs `2ac389e`. This is the same as in earlier phases.
- **`wgpui-component` tests** fail to compile on main (`gpui::headless` is missing). This is unrelated to this work.
- **Pre-existing test failures**, as in Phases 3 and 4. See the sweep below.
