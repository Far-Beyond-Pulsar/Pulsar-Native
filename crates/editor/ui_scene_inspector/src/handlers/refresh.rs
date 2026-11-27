//! Reading the scene: one job at a time on a background thread, its result
//! applied on the UI thread. The UI thread never takes the scene's lock.
//!
//! The index and the selected value are re-read while Live, each no more
//! often than the cost of its last read allows, so inspecting a huge scene or
//! value cannot crowd out the renderer. Paused, nothing is re-read until the
//! selection moves or Refresh is pressed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_backend::scene::SharedScene;
use engine_backend::services::scene_directory;
use gpui::Context;
use pulsar_scene_model::SceneWorldExt as _;
use pulsar_scenedb::{ComponentId, Entity};

use crate::screen::SceneInspectorView;
use crate::utils::detail::{self, ComponentData, ComponentRef, EntityDetail};
use crate::utils::index::{self, SceneIndex};

/// How often the view checks whether a read is due.
pub const TICK: Duration = Duration::from_millis(100);
const MIN_VALUE_INTERVAL: Duration = Duration::from_millis(200);
const MIN_INDEX_INTERVAL: Duration = Duration::from_millis(750);
const SCENES_INTERVAL: Duration = Duration::from_secs(1);

/// The records of one page of a component's column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnRequest {
    pub id: ComponentId,
    pub stride: usize,
    pub entities: Arc<[Entity]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnPage {
    pub request: ColumnRequest,
    /// `stride` bytes per requested entity.
    pub bytes: Arc<[u8]>,
    /// `true` where the entity no longer has the component: its bytes are
    /// zeros that stand for nothing.
    pub missing: Vec<bool>,
}

struct Job {
    scene: SharedScene,
    scene_id: u64,
    /// `Some` when the index is due: the revision of the one on hand, to
    /// skip a scan of an unchanged scene.
    index: Option<Option<u64>>,
    /// Follow the editor's selection, if it moved from this.
    follow: Option<Option<Entity>>,
    selected: Option<Entity>,
    wanted: Option<ComponentRef>,
    column: Option<ColumnRequest>,
    /// Selection the job was built for; see [`SceneInspectorView::apply`].
    selection_epoch: u64,
}

struct Done {
    scene_id: u64,
    selection_epoch: u64,
    index: Option<(Arc<SceneIndex>, Duration)>,
    /// The editor's selection, when it differs from the one followed.
    editor_selection: Option<Option<Entity>>,
    selected: Option<Entity>,
    detail: Option<EntityDetail>,
    data: Option<ComponentData>,
    column: Option<ColumnPage>,
    read: Duration,
}

fn run(job: Job) -> Done {
    let editor_selection = job.follow.and_then(|followed| {
        let selected = job.scene.read().world.selected_entity();
        (selected != followed).then_some(selected)
    });
    // A selection the editor just made is read in this same job.
    let followed = editor_selection.flatten();
    let selected = followed.or(job.selected);

    let index = job.index.and_then(|have| {
        let revision = job.scene.read().world.revision();
        // A world without a change tracker reports 0: always scan it.
        if revision != 0 && have == Some(revision) {
            return None;
        }
        let start = Instant::now();
        let index = index::scan(&job.scene);
        Some((Arc::new(index), start.elapsed()))
    });

    let start = Instant::now();
    let (detail, data, column) = {
        let guard = job.scene.read();
        let world = &guard.world;
        let detail = selected.and_then(|entity| detail::read_detail(world, entity));
        let data = detail
            .as_ref()
            .and_then(|detail| detail.pick(job.wanted))
            .and_then(|target| detail::read_component(world, target));
        let column = job.column.map(|request| {
            let mut bytes = vec![0u8; request.stride * request.entities.len()];
            let mut missing = vec![true; request.entities.len()];
            for (record, &entity) in request.entities.iter().enumerate() {
                let Some(value) = world.get_dyn(entity, request.id) else {
                    continue;
                };
                let value = detail::value_bytes(value);
                if value.len() == request.stride {
                    bytes[record * request.stride..][..request.stride].copy_from_slice(&value);
                    missing[record] = false;
                }
            }
            ColumnPage {
                request,
                bytes: bytes.into(),
                missing,
            }
        });
        (detail, data, column)
    };

    Done {
        scene_id: job.scene_id,
        selection_epoch: job.selection_epoch,
        index,
        editor_selection,
        selected,
        detail,
        data,
        column,
        read: start.elapsed(),
    }
}

impl SceneInspectorView {
    /// Called every [`TICK`].
    pub(crate) fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if now >= self.refresh.scenes_due {
            self.refresh.scenes_due = now + SCENES_INTERVAL;
            self.refresh_scenes(cx);
        }
        if self.expire_flashes(now) {
            cx.notify();
        }
        let due = self.live && (now >= self.refresh.index_due || now >= self.refresh.value_due);
        if due || self.refresh.requested {
            self.start_job(cx);
        }
    }

    /// Read again as soon as the running job (if any) is done. `index` also
    /// rescans the scene, whatever its revision.
    pub(crate) fn request_refresh(&mut self, index: bool, cx: &mut Context<Self>) {
        self.refresh.requested = true;
        self.refresh.force_index |= index;
        self.start_job(cx);
    }

    pub(crate) fn refresh_scenes(&mut self, cx: &mut Context<Self>) {
        let scenes = scene_directory::list();
        let ids: Vec<(u64, &str)> = scenes.iter().map(|s| (s.id, s.label.as_str())).collect();
        let had: Vec<(u64, &str)> = self
            .scenes
            .iter()
            .map(|(id, label)| (*id, label.as_str()))
            .collect();
        let changed = ids != had;
        let current = self.scene.as_ref().map(|(id, _)| *id);
        if !current.is_some_and(|id| scenes.iter().any(|s| s.id == id)) {
            // The scene went away (or there was none): take the first.
            let next = scenes.first().map(|s| (s.id, Arc::downgrade(&s.scene)));
            if current.is_some() || next.is_some() {
                self.scene = next;
                self.reset_scene(cx);
            }
        }
        if changed {
            self.scenes = scenes.iter().map(|s| (s.id, s.label.clone())).collect();
            cx.notify();
        }
    }

    fn start_job(&mut self, cx: &mut Context<Self>) {
        if self.refresh.running {
            return;
        }
        let Some((scene_id, scene)) = self
            .scene
            .as_ref()
            .and_then(|(id, scene)| Some((*id, scene.upgrade()?)))
        else {
            self.refresh.requested = false;
            return;
        };
        let now = Instant::now();
        let force_index = std::mem::take(&mut self.refresh.force_index);
        let index_due =
            force_index || self.index.is_none() || (self.live && now >= self.refresh.index_due);
        let job = Job {
            scene,
            scene_id,
            index: index_due.then(|| {
                (!force_index)
                    .then(|| self.index.as_ref().map(|index| index.revision))
                    .flatten()
            }),
            follow: (self.follow_editor && self.live).then_some(self.followed),
            selected: self.selected,
            wanted: self.wanted,
            column: self.column_request(),
            selection_epoch: self.refresh.selection_epoch,
        };
        if index_due {
            self.refresh.index_due = now + self.refresh.index_interval;
        }
        self.refresh.requested = false;
        self.refresh.running = true;
        cx.spawn(async move |view, cx| {
            let done = cx
                .background_executor()
                .spawn(async move { run(job) })
                .await;
            let _ = view.update(cx, |view, cx| view.apply(done, cx));
        })
        .detach();
    }

    fn apply(&mut self, done: Done, cx: &mut Context<Self>) {
        self.refresh.running = false;
        self.refresh.value_due = Instant::now() + (done.read * 20).max(MIN_VALUE_INTERVAL);
        if self.scene.as_ref().map(|(id, _)| *id) != Some(done.scene_id) {
            self.refresh.requested = true;
            self.start_job(cx);
            return;
        }
        self.refresh.read_time = done.read;
        let mut changed = false;
        if let Some((index, took)) = done.index {
            self.refresh.index_interval = (took * 25).max(MIN_INDEX_INTERVAL);
            self.refresh.index_due = Instant::now() + self.refresh.index_interval;
            self.refresh.scan_time = took;
            self.index = Some(index);
            self.rebuild_rows();
            self.rebuild_column();
            changed = true;
        }
        // The selection moved while the job ran: what it read is for the old
        // one. Read again at once.
        if done.selection_epoch != self.refresh.selection_epoch {
            if changed {
                cx.notify();
            }
            self.refresh.requested = true;
            self.start_job(cx);
            return;
        }
        if let Some(editor_selection) = done.editor_selection {
            self.followed = editor_selection;
            if let Some(entity) = editor_selection {
                if self.selected != Some(entity) {
                    self.record_history();
                    self.selected = Some(entity);
                    debug_assert_eq!(done.selected, Some(entity));
                    self.reveal_selected();
                    changed = true;
                }
            }
        }
        changed |= self.set_detail(done.detail);
        changed |= self.set_data(done.data);
        changed |= self.set_column_page(done.column);
        if changed {
            cx.notify();
        }
        if self.refresh.requested {
            self.start_job(cx);
        }
    }
}

/// The refresh schedule.
#[derive(Debug)]
pub struct Refresh {
    pub running: bool,
    pub requested: bool,
    pub force_index: bool,
    pub index_due: Instant,
    pub value_due: Instant,
    pub scenes_due: Instant,
    pub index_interval: Duration,
    pub scan_time: Duration,
    pub read_time: Duration,
    /// Counts selection changes.
    pub selection_epoch: u64,
}

impl Default for Refresh {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            running: false,
            requested: false,
            force_index: false,
            index_due: now,
            value_due: now,
            scenes_due: now,
            index_interval: MIN_INDEX_INTERVAL,
            scan_time: Duration::ZERO,
            read_time: Duration::ZERO,
            selection_epoch: 0,
        }
    }
}
