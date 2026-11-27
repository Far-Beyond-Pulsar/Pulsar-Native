//! The inspector's state, and how new reads of the scene become what is on
//! screen.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use gpui::*;
use pulsar_scenedb::{Entity, SceneDb};
use ui::{
    h_flex,
    input::{InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, ResizableState},
    scroll::ScrollbarState,
    v_flex, ActiveTheme,
};

use crate::handlers::refresh::{ColumnPage, ColumnRequest, Refresh, TICK};
use crate::utils::detail::{ComponentData, ComponentRef, EntityDetail};
use crate::utils::fields::{self, FieldRow, Toggled};
use crate::utils::hex::HexLayout;
use crate::utils::index::SceneIndex;
use crate::utils::outline::{self, Expansion, Mode, Row};
use crate::utils::query::Query;

/// How long a changed value stays highlighted.
const FLASH: Duration = Duration::from_millis(1500);
/// Most rows of one page of a column. Beyond a few million pixels a list's
/// scroll position loses precision.
const PAGE_ROWS: usize = 65_536;
const HISTORY_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DataTab {
    #[default]
    Fields,
    Hex,
    Binary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ByteScope {
    /// The selected entity's value.
    #[default]
    Value,
    /// The same component of every entity that has it.
    Column,
}

/// The byte viewer's own state.
pub struct ByteState {
    pub scope: ByteScope,
    pub hex_per_row: usize,
    pub binary_per_row: usize,
    pub big_endian: bool,
    /// Byte indices into what is shown (the value, or the page).
    pub cursor: usize,
    pub anchor: usize,
    pub page: usize,
    /// Every entity with the shown component.
    pub column: Arc<[Entity]>,
    pub page_data: Option<ColumnPage>,
    pub scroll: UniformListScrollHandle,
    pub scrollbar: ScrollbarState,
    pub goto: Entity_<InputState>,
    pub legend_scroll: ListScroll,
}

/// gpui's `Entity`, named apart from SceneDB's.
pub type Entity_<T> = gpui::Entity<T>;

impl ByteState {
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }
}

/// A list's scroll position and scrollbar.
#[derive(Default)]
pub struct ListScroll {
    pub handle: UniformListScrollHandle,
    pub bar: ScrollbarState,
}

pub struct SceneInspectorView {
    pub(crate) focus: FocusHandle,
    pub(crate) browser_focus: FocusHandle,
    pub(crate) bytes_focus: FocusHandle,

    /// Listed scenes: id and name.
    pub(crate) scenes: Vec<(u64, String)>,
    pub(crate) scene: Option<(u64, Weak<parking_lot::RwLock<SceneDb>>)>,

    pub(crate) live: bool,
    pub(crate) follow_editor: bool,
    /// The editor selection last followed.
    pub(crate) followed: Option<Entity>,
    pub(crate) refresh: Refresh,

    pub(crate) index: Option<Arc<SceneIndex>>,
    pub(crate) mode: Mode,
    pub(crate) search: Entity_<InputState>,
    pub(crate) query: Query,
    pub(crate) expansion: Expansion,
    pub(crate) rows: Arc<Vec<Row>>,
    pub(crate) browser_scroll: ListScroll,

    pub(crate) selected: Option<Entity>,
    /// The component asked for; what is shown is `data`'s.
    pub(crate) wanted: Option<ComponentRef>,
    pub(crate) back: Vec<(Entity, Option<ComponentRef>)>,
    pub(crate) forward: Vec<(Entity, Option<ComponentRef>)>,

    pub(crate) detail: Option<Arc<EntityDetail>>,
    pub(crate) component_scroll: ListScroll,

    pub(crate) data: Option<Arc<ComponentData>>,
    pub(crate) tab: DataTab,
    pub(crate) field_search: Entity_<InputState>,
    pub(crate) field_filter: String,
    pub(crate) toggled: Toggled,
    pub(crate) field_rows: Arc<Vec<FieldRow>>,
    pub(crate) field_search_truncated: bool,
    pub(crate) field_scroll: ListScroll,
    /// Field rows, by path, and bytes, by index, that changed and when.
    pub(crate) changed_fields: HashMap<String, Instant>,
    pub(crate) changed_bytes: HashMap<usize, Instant>,

    pub(crate) bytes: ByteState,
    pub(crate) split: Entity_<ResizableState>,
    pub(crate) notice: Option<(String, Instant)>,
    _poll: Task<()>,
}

impl SceneInspectorView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut input = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                state.set_placeholder(placeholder, window, cx);
                state
            })
        };
        let search = input("Search name, id, 12v1, has:Transform", cx);
        let field_search = input("Filter fields and values", cx);
        let goto = input("Offset: 0x40", cx);
        cx.subscribe(&search, |this: &mut Self, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = Query::parse(&input.read(cx).value());
                this.rebuild_rows();
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(
            &field_search,
            |this: &mut Self, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.field_filter = input.read(cx).value().to_string();
                    this.rebuild_field_rows();
                    cx.notify();
                }
            },
        )
        .detach();
        cx.subscribe(&goto, |this: &mut Self, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let text = input.read(cx).value().to_string();
                this.go_to_offset(&text, cx);
            }
        })
        .detach();

        let poll = cx.spawn(async move |view, cx| loop {
            if view.update(cx, |view, cx| view.tick(cx)).is_err() {
                break;
            }
            cx.background_executor().timer(TICK).await;
        });

        Self {
            focus: cx.focus_handle(),
            browser_focus: cx.focus_handle(),
            bytes_focus: cx.focus_handle(),
            scenes: Vec::new(),
            scene: None,
            live: true,
            follow_editor: true,
            followed: None,
            refresh: Refresh::default(),
            index: None,
            mode: Mode::default(),
            search,
            query: Query::default(),
            expansion: Expansion::default(),
            rows: Arc::default(),
            browser_scroll: ListScroll::default(),
            selected: None,
            wanted: None,
            back: Vec::new(),
            forward: Vec::new(),
            detail: None,
            component_scroll: ListScroll::default(),
            data: None,
            tab: DataTab::default(),
            field_search,
            field_filter: String::new(),
            toggled: Toggled::default(),
            field_rows: Arc::default(),
            field_search_truncated: false,
            field_scroll: ListScroll::default(),
            changed_fields: HashMap::new(),
            changed_bytes: HashMap::new(),
            bytes: ByteState {
                scope: ByteScope::default(),
                hex_per_row: 16,
                binary_per_row: 8,
                big_endian: false,
                cursor: 0,
                anchor: 0,
                page: 0,
                column: Arc::new([]),
                page_data: None,
                scroll: UniformListScrollHandle::new(),
                scrollbar: ScrollbarState::default(),
                goto,
                legend_scroll: ListScroll::default(),
            },
            split: ResizableState::new(cx),
            notice: None,
            _poll: poll,
        }
    }

    /// Forget everything read from the previous scene.
    pub(crate) fn reset_scene(&mut self, cx: &mut Context<Self>) {
        self.index = None;
        self.rows = Arc::default();
        self.expansion = Expansion::default();
        self.selected = None;
        self.wanted = None;
        self.followed = None;
        self.back.clear();
        self.forward.clear();
        self.detail = None;
        self.data = None;
        self.field_rows = Arc::default();
        self.changed_fields.clear();
        self.changed_bytes.clear();
        self.bytes.column = Arc::new([]);
        self.bytes.page_data = None;
        self.refresh.selection_epoch += 1;
        self.request_refresh(true, cx);
        cx.notify();
    }

    pub(crate) fn rebuild_rows(&mut self) {
        self.rows = Arc::new(match &self.index {
            Some(index) => outline::rows(index, self.mode, &self.query, &self.expansion),
            None => Vec::new(),
        });
    }

    pub(crate) fn rebuild_field_rows(&mut self) {
        let (rows, truncated) = match self.data.as_ref().and_then(|data| data.fields.as_ref()) {
            Some(set) => fields::rows(set, &self.toggled, &self.field_filter),
            None => (Vec::new(), false),
        };
        self.field_rows = Arc::new(rows);
        self.field_search_truncated = truncated;
    }

    /// `true` if what is drawn changed.
    pub(crate) fn set_detail(&mut self, detail: Option<EntityDetail>) -> bool {
        if self.detail.as_deref() == detail.as_ref() {
            return false;
        }
        self.detail = detail.map(Arc::new);
        true
    }

    /// Take a new read of the shown component, noting what changed since the
    /// last read of the same one. `true` if what is drawn changed.
    pub(crate) fn set_data(&mut self, data: Option<ComponentData>) -> bool {
        let Some(data) = data else {
            let had = self.data.take().is_some();
            if had {
                self.rebuild_field_rows();
            }
            return had;
        };
        let previous = self.data.clone();
        let same_target = previous
            .as_ref()
            .is_some_and(|previous| previous.target == data.target);
        if let Some(previous) = previous.as_ref().filter(|_| same_target) {
            let same_fields = previous.fields.as_ref().map(|set| {
                set.fields.iter().map(|f| &f.value).collect::<Vec<_>>()
            }) == data.fields.as_ref().map(|set| {
                set.fields.iter().map(|f| &f.value).collect::<Vec<_>>()
            });
            if previous.bytes == data.bytes && same_fields {
                return false;
            }
        }

        let now = Instant::now();
        if !same_target {
            self.changed_fields.clear();
            self.changed_bytes.clear();
            if previous.is_none_or(|previous| previous.target.id != data.target.id) {
                // Another type: its paths and offsets mean other things.
                self.toggled.clear();
                self.bytes.cursor = 0;
                self.bytes.anchor = 0;
                self.bytes.page = 0;
            }
        }
        let previous = self.data.take().filter(|_| same_target);
        self.data = Some(Arc::new(data));
        self.rebuild_field_rows();
        self.rebuild_column();
        if let (Some(previous), Some(data)) = (previous, self.data.clone()) {
            if let (Some(old), Some(new)) = (&previous.fields, &data.fields) {
                for row in self.field_rows.iter() {
                    let (old, new) = (old.value_at(&row.path), new.value_at(&row.path));
                    if new.is_some() && old != new {
                        self.changed_fields.insert(row.path.clone(), now);
                    }
                }
            }
            if self.bytes.scope == ByteScope::Value && previous.bytes.len() == data.bytes.len() {
                for (index, (old, new)) in previous.bytes.iter().zip(data.bytes.iter()).enumerate()
                {
                    if old != new {
                        self.changed_bytes.insert(index, now);
                    }
                }
            }
        }
        self.clamp_cursor();
        true
    }

    /// The entities of the column view, from the index.
    pub(crate) fn rebuild_column(&mut self) {
        let column: Vec<Entity> = match (self.bytes.scope, &self.index, &self.data) {
            (ByteScope::Column, Some(index), Some(data)) => index
                .entities_with(data.target.id)
                .into_iter()
                .map(|ix| index.entities[ix as usize].entity)
                .collect(),
            _ => Vec::new(),
        };
        if *self.bytes.column != *column {
            self.bytes.column = column.into();
        }
        self.bytes.page = self.bytes.page.min(self.page_count() - 1);
    }

    pub(crate) fn per_row(&self) -> usize {
        match self.tab {
            DataTab::Binary => self.bytes.binary_per_row,
            _ => self.bytes.hex_per_row,
        }
    }

    /// Bytes of one record: the size of one value.
    pub(crate) fn record_len(&self) -> usize {
        self.data.as_ref().map_or(0, |data| data.bytes.len())
    }

    pub(crate) fn records_per_page(&self) -> usize {
        let rows = HexLayout::new(self.record_len(), 1, self.per_row()).rows_per_record();
        (PAGE_ROWS / rows.max(1)).max(1)
    }

    pub(crate) fn page_count(&self) -> usize {
        self.bytes
            .column
            .len()
            .div_ceil(self.records_per_page())
            .max(1)
    }

    /// The records shown: one value, or one page of the column.
    pub(crate) fn layout(&self) -> HexLayout {
        let records = match self.bytes.scope {
            ByteScope::Value => usize::from(self.data.is_some()),
            ByteScope::Column => self.page_entities().len(),
        };
        HexLayout::new(self.record_len(), records, self.per_row())
    }

    pub(crate) fn page_entities(&self) -> &[Entity] {
        let size = self.records_per_page();
        let start = (self.bytes.page * size).min(self.bytes.column.len());
        let end = (start + size).min(self.bytes.column.len());
        &self.bytes.column[start..end]
    }

    /// What the next read must fetch for the column view.
    pub(crate) fn column_request(&self) -> Option<ColumnRequest> {
        if self.bytes.scope != ByteScope::Column || self.tab == DataTab::Fields {
            return None;
        }
        let data = self.data.as_ref()?;
        Some(ColumnRequest {
            id: data.target.id,
            stride: data.bytes.len(),
            entities: self.page_entities().into(),
        })
    }

    pub(crate) fn set_column_page(&mut self, page: Option<ColumnPage>) -> bool {
        let Some(page) = page else {
            return false;
        };
        if self.bytes.page_data.as_ref() == Some(&page) {
            return false;
        }
        if let Some(previous) = &self.bytes.page_data {
            if previous.request == page.request {
                let now = Instant::now();
                for (index, (old, new)) in previous.bytes.iter().zip(page.bytes.iter()).enumerate() {
                    if old != new {
                        self.changed_bytes.insert(index, now);
                    }
                }
            } else {
                self.changed_bytes.clear();
            }
        }
        self.bytes.page_data = Some(page);
        true
    }

    /// The byte at `index` of what is shown; `None` while a page is being
    /// read, or where its entity no longer has the component.
    pub(crate) fn byte(&self, index: usize) -> Option<u8> {
        match self.bytes.scope {
            ByteScope::Value => self.data.as_ref()?.bytes.get(index).copied(),
            ByteScope::Column => {
                let page = self.bytes.page_data.as_ref()?;
                let stride = page.request.stride;
                let record = index.checked_div(stride)?;
                let data = self.data.as_ref()?;
                if page.request.id != data.target.id
                    || stride != data.bytes.len()
                    || *page.missing.get(record)?
                    || page.request.entities.get(record) != self.page_entities().get(record)
                {
                    return None;
                }
                page.bytes.get(index).copied()
            }
        }
    }

    pub(crate) fn clamp_cursor(&mut self) {
        let last = self.layout().len().saturating_sub(1);
        self.bytes.cursor = self.bytes.cursor.min(last);
        self.bytes.anchor = self.bytes.anchor.min(last);
    }

    /// Drop old highlights. `true` if any went.
    pub(crate) fn expire_flashes(&mut self, now: Instant) -> bool {
        let before = self.changed_fields.len() + self.changed_bytes.len();
        self.changed_fields
            .retain(|_, at| now.duration_since(*at) < FLASH);
        self.changed_bytes
            .retain(|_, at| now.duration_since(*at) < FLASH);
        let mut changed = before != self.changed_fields.len() + self.changed_bytes.len();
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at) > Duration::from_secs(3))
        {
            self.notice = None;
            changed = true;
        }
        changed
    }

    pub(crate) fn record_history(&mut self) {
        if let Some(entity) = self.selected {
            self.back.push((entity, self.wanted));
            if self.back.len() > HISTORY_LIMIT {
                self.back.remove(0);
            }
            self.forward.clear();
        }
    }
}

impl Focusable for SceneInspectorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SceneInspectorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let toolbar = self.render_toolbar(cx);
        let status = self.render_status(cx);
        let body = if self.scene.is_none() {
            crate::components::widgets::empty_state(
                ui::IconName::Database,
                "No scene is open",
                "Open a level in the Level Editor to inspect its SceneDB.",
                cx,
            )
        } else {
            let browser = self.render_browser(cx);
            let components = self.render_components(cx);
            let data = self.render_data_pane(cx);
            h_resizable("scene-inspector-split")
                .state(self.split.clone())
                .child(
                    resizable_panel()
                        .size(px(340.0))
                        .size_range(px(240.0)..px(640.0))
                        .child(browser),
                )
                .child(
                    resizable_panel()
                        .size(px(290.0))
                        .size_range(px(200.0)..px(520.0))
                        .child(components),
                )
                .child(resizable_panel().child(data))
                .into_any_element()
        };
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .child(toolbar)
            .child(h_flex().flex_1().min_h_0().w_full().child(body))
            .child(status)
    }
}
