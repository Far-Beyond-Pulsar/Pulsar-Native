//! Scene spline browser, point inspector, curve configuration and authoring tools.
use crate::{
    commands::{SceneCommand, execute_command},
    core::splines,
    scene_edit::SceneObjectData,
    state::{
        LevelEditorState,
        spline::{
            CurveAlgorithm, CurveAlgorithmText, DrawingPlane, SplineData, SplineDomain, SplinePoint,
            SplineTool,
        },
    },
};
use gpui::*;
use rust_i18n::t;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use ui::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{InputEvent, InputState, TextInput},
    v_flex,
};

type SharedState = Arc<parking_lot::RwLock<LevelEditorState>>;
type Signature = (u64, u64, Option<String>, SplineDomain);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Manage,
    Points,
    Curve,
    Tools,
}
impl Tab {
    const ALL: [Self; 4] = [Self::Manage, Self::Points, Self::Curve, Self::Tools];
    fn key(self) -> &'static str {
        match self {
            Self::Manage => "LevelEditor.SplinePanel.Manage",
            Self::Points => "LevelEditor.SplinePanel.Points",
            Self::Curve => "LevelEditor.SplinePanel.Curve",
            Self::Tools => "LevelEditor.SplinePanel.Tools",
        }
    }
}
struct Field {
    input: Entity<InputState>,
    value: String,
    _subscription: Subscription,
}
pub(crate) struct SplinePanel {
    owner: WeakEntity<super::LevelEditorPanel>,
    state: SharedState,
    focus_handle: FocusHandle,
    pump_started: bool,
    last_signature: Signature,
    tab: Tab,
    collapsed: HashSet<&'static str>,
    fields: HashMap<String, Field>,
    search: Entity<InputState>,
    _search_subscription: Subscription,
    field_target: Option<(String, Option<usize>)>,
    validation: Option<String>,
}
impl SplinePanel {
    pub(crate) fn new(
        state: SharedState,
        owner: WeakEntity<super::LevelEditorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("LevelEditor.SplinePanel.Search").to_string())
        });
        let sub = cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let last_signature = Self::signature(&state);
        Self {
            owner,
            state,
            focus_handle: cx.focus_handle(),
            pump_started: false,
            last_signature,
            tab: Tab::Manage,
            collapsed: HashSet::new(),
            fields: HashMap::new(),
            search,
            _search_subscription: sub,
            field_target: None,
            validation: None,
        }
    }
    fn signature(state: &SharedState) -> Signature {
        let st = state.read();
        (
            st.scene.world_revision(),
            st.scene.subscriptions_epoch(),
            st.scene.selected_object(),
            st.editor.spline.clone(),
        )
    }
    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        crate::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _, cx| {
                let signature = Self::signature(&this.state);
                if signature != this.last_signature {
                    splines::sync_selection(&mut this.state.write());
                    this.last_signature = Self::signature(&this.state);
                    cx.notify();
                }
            },
        );
    }
    fn action(
        &self,
        id: impl Into<String>,
        key: &'static str,
        disabled: bool,
        apply: impl Fn(&mut LevelEditorState) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.clone();
        Button::new(id.into())
            .label(t!(key))
            .small()
            .disabled(disabled)
            .on_click(cx.listener(move |_, _, _, cx| {
                apply(&mut state.write());
                cx.notify();
            }))
            .into_any_element()
    }
    fn toggle(
        &self,
        id: &'static str,
        key: &'static str,
        value: bool,
        apply: impl Fn(&mut LevelEditorState, bool) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.clone();
        Checkbox::new(id)
            .label(t!(key).to_string())
            .checked(value)
            .on_click(cx.listener(move |_, _, _, cx| {
                apply(&mut state.write(), !value);
                cx.notify();
            }))
            .into_any_element()
    }
    fn section(
        &self,
        key: &'static str,
        children: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.collapsed.contains(key);
        let theme = cx.theme().clone();
        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .id(key)
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(key) {
                            this.collapsed.insert(key);
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size_3p5(),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!(key).to_string().to_uppercase()),
                    ),
            )
            .children(if collapsed { vec![] } else { children })
            .into_any_element()
    }
    fn text_field(
        &mut self,
        id: String,
        label: String,
        value: String,
        apply: impl Fn(&mut LevelEditorState, String) -> bool + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.fields.contains_key(&id) {
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(value.clone(), window, cx);
                input
            });
            let field_id = id.clone();
            let subscription = cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let value = input.read(cx).text().to_string();
                        if this.fields.get(&field_id).is_some_and(|f| f.value != value) {
                            let valid = apply(&mut this.state.write(), value);
                            this.validation = if valid {
                                None
                            } else {
                                Some(t!("LevelEditor.SplinePanel.InvalidValue").to_string())
                            };
                            cx.notify();
                        }
                    }
                },
            );
            self.fields.insert(
                id.clone(),
                Field {
                    input,
                    value: value.clone(),
                    _subscription: subscription,
                },
            );
        }
        let field = self.fields.get_mut(&id).unwrap();
        if field.input.read(cx).text().to_string() != value
            && !field.input.read(cx).focus_handle(cx).is_focused(window)
        {
            field
                .input
                .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
        }
        field.value = value;
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(div().text_xs().flex_1().child(label))
            .child(
                div()
                    .w(px(145.))
                    .child(TextInput::new(&field.input).small()),
            )
            .into_any_element()
    }
    fn number(
        &mut self,
        id: String,
        key: &str,
        value: f32,
        min: f32,
        max: f32,
        apply: impl Fn(&mut LevelEditorState, f32) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.text_field(
            id,
            t!(key).to_string(),
            format!("{value:.3}"),
            move |state, text| {
                if let Ok(value) = text.parse::<f32>() {
                    if value.is_finite() {
                        apply(state, value.clamp(min, max));
                        return true;
                    }
                }
                false
            },
            window,
            cx,
        )
    }
    fn curve_number(
        &mut self,
        object: &SceneObjectData,
        id: &str,
        key: &str,
        value: f32,
        min: f32,
        max: f32,
        apply: impl Fn(&mut SplineData, f32) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = object.id.clone();
        self.number(
            format!("{}:{id}", object.id),
            key,
            value,
            min,
            max,
            move |state, value| {
                if state.scene.selected_object().as_ref() == Some(&target) {
                    splines::edit(state, |d| apply(d, value));
                }
            },
            window,
            cx,
        )
    }
    fn manage(
        &mut self,
        rows: &[(SceneObjectData, SplineData)],
        selected: &Option<(SceneObjectData, SplineData)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filter = self.search.read(cx).text().to_string().to_lowercase();
        let active = selected.as_ref().map(|(o, _)| o.id.as_str());
        let mut browser = vec![TextInput::new(&self.search).small().into_any_element()];
        let theme = cx.theme().clone();
        let mut matched = 0;
        for (object, curve) in rows
            .iter()
            .filter(|(o, _)| o.name.to_lowercase().contains(&filter))
        {
            matched += 1;
            let selected = Some(object.id.as_str()) == active;
            let id = object.id.clone();
            let state = self.state.clone();
            browser.push(
                v_flex()
                    .id(SharedString::from(format!("spline-row-{id}")))
                    .p_2()
                    .gap_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected {
                        theme.primary
                    } else {
                        theme.border
                    })
                    .bg(if selected {
                        theme.primary.opacity(0.12)
                    } else {
                        theme.muted.opacity(0.12)
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        splines::select(&mut state.write(), id.clone());
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::MapPin).size_4())
                            .child(div().flex_1().text_sm().child(object.name.clone()))
                            .children(object.locked.then(|| {
                                div()
                                    .text_xs()
                                    .child(t!("LevelEditor.SplinePanel.Locked").to_string())
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} · {} {} · {}",
                                t!(curve.algorithm.key()),
                                curve.points.len(),
                                t!("LevelEditor.Spline.PointCount"),
                                t!(if curve.closed {
                                    "LevelEditor.SplinePanel.Closed"
                                } else {
                                    "LevelEditor.SplinePanel.Open"
                                })
                            )),
                    )
                    .into_any_element(),
            );
        }
        if matched == 0 {
            browser.push(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.SplinePanel.NoSplines").to_string())
                    .into_any_element(),
            );
        }
        let mut sections = vec![self.section("LevelEditor.SplinePanel.SceneSplines", browser, cx)];
        let mut creation = vec![self.action(
            "new-spline",
            "LevelEditor.SplinePanel.New",
            false,
            |s| {
                splines::create(s, SplineData::default());
                s.editor.spline.tool = SplineTool::Draw;
            },
            cx,
        )];
        for (id, key) in [
            ("line", "LevelEditor.SplinePanel.LinePreset"),
            ("circle", "LevelEditor.SplinePanel.Circle"),
            ("rectangle", "LevelEditor.SplinePanel.Rectangle"),
            ("arc", "LevelEditor.SplinePanel.Arc"),
            ("helix", "LevelEditor.SplinePanel.Helix"),
        ] {
            creation.push(self.action(
                id,
                key,
                false,
                move |s| {
                    let d = splines::preset(s, id);
                    splines::create(s, d);
                },
                cx,
            ));
        }
        let settings = self.state.read().editor.spline.clone();
        creation.push(self.number(
            "preset-radius".into(),
            "LevelEditor.SplinePanel.Radius",
            settings.preset_radius,
            0.01,
            100000.,
            |s, v| s.editor.spline.preset_radius = v,
            window,
            cx,
        ));
        creation.push(self.number(
            "preset-count".into(),
            "LevelEditor.SplinePanel.PresetPoints",
            settings.preset_count as f32,
            3.,
            128.,
            |s, v| s.editor.spline.preset_count = v as usize,
            window,
            cx,
        ));
        creation.push(self.number(
            "helix-height".into(),
            "LevelEditor.SplinePanel.HelixHeight",
            settings.helix_height,
            -100000.,
            100000.,
            |s, v| s.editor.spline.helix_height = v,
            window,
            cx,
        ));
        sections.push(self.section("LevelEditor.SplinePanel.Create", creation, cx));
        if let Some((object, _)) = selected {
            let target = object.id.clone();
            let mut items = vec![self.text_field(
                format!("{}:name", object.id),
                t!("LevelEditor.SplinePanel.Name").to_string(),
                object.name.clone(),
                move |s, name| {
                    if s.scene.selected_object().as_ref() == Some(&target)
                        && !name.trim().is_empty()
                    {
                        execute_command(
                            s,
                            SceneCommand::SetName {
                                id: target.clone(),
                                name,
                            },
                        );
                        return true;
                    }
                    false
                },
                window,
                cx,
            )];
            items.push(self.action(
                "duplicate",
                "LevelEditor.SplinePanel.Duplicate",
                false,
                |s| {
                    if let Some((o, _)) = splines::selected(s) {
                        let r = execute_command(
                            s,
                            SceneCommand::DuplicateObject {
                                source_id: o.id,
                                count: 1,
                                position_offset: None,
                            },
                        );
                        if let Some(id) = r.affected_ids.last() {
                            splines::select(s, id.clone());
                        }
                    }
                },
                cx,
            ));
            items.push(self.toggle(
                "visible",
                "LevelEditor.SplinePanel.Visible",
                object.visible,
                |s, v| {
                    if let Some((o, _)) = splines::selected(s) {
                        execute_command(
                            s,
                            SceneCommand::SetVisibility {
                                id: o.id,
                                visible: Some(v),
                                locked: None,
                            },
                        );
                    }
                },
                cx,
            ));
            items.push(self.toggle(
                "locked",
                "LevelEditor.SplinePanel.Locked",
                object.locked,
                |s, v| {
                    if let Some((o, _)) = splines::selected(s) {
                        execute_command(
                            s,
                            SceneCommand::SetVisibility {
                                id: o.id,
                                visible: None,
                                locked: Some(v),
                            },
                        );
                    }
                },
                cx,
            ));
            items.push(self.action(
                "delete-spline",
                "LevelEditor.SplinePanel.DeleteSpline",
                object.locked,
                |s| {
                    if let Some((o, _)) = splines::selected(s) {
                        if !o.locked {
                            execute_command(s, SceneCommand::RemoveObject { id: o.id });
                            splines::sync_selection(s);
                        }
                    }
                },
                cx,
            ));
            sections.push(self.section("LevelEditor.SplinePanel.Object", items, cx));
        }
        sections
    }
    fn points(
        &mut self,
        object: &SceneObjectData,
        curve: &SplineData,
        settings: &SplineDomain,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![];
        for (i, p) in curve.points.iter().enumerate() {
            let state = self.state.clone();
            let label = format!(
                "{:03}   {:.2}, {:.2}, {:.2}",
                i + 1,
                p.position[0],
                p.position[1],
                p.position[2]
            );
            let btn = Button::new(format!("point-{i}"))
                .label(label)
                .small()
                .on_click(cx.listener(move |_, _, _, cx| {
                    state.write().editor.spline.selected_point = Some(i);
                    cx.notify();
                }));
            rows.push(
                if settings.selected_point == Some(i) {
                    btn.primary()
                } else {
                    btn.ghost()
                }
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .text_sm()
                    .child(t!("LevelEditor.SplinePanel.NoPoints").to_string())
                    .into_any_element(),
            );
        }
        let mut result = vec![self.section("LevelEditor.SplinePanel.ControlPoints", rows, cx)];
        if let Some(i) = settings.selected_point.filter(|&i| i < curve.points.len()) {
            let p = &curve.points[i];
            let mut position = vec![
                div()
                    .text_xs()
                    .child(t!("LevelEditor.SplinePanel.LocalSpace").to_string())
                    .into_any_element(),
            ];
            for (axis, key) in [
                "LevelEditor.SplinePanel.X",
                "LevelEditor.SplinePanel.Y",
                "LevelEditor.SplinePanel.Z",
            ]
            .into_iter()
            .enumerate()
            {
                position.push(self.curve_number(
                    object,
                    &format!("point-{i}-{axis}"),
                    key,
                    p.position[axis],
                    -1e7,
                    1e7,
                    move |d, v| {
                        if let Some(p) = d.points.get_mut(i) {
                            p.position[axis] = v;
                        }
                    },
                    window,
                    cx,
                ));
            }
            for (id, key, delta) in [
                ("prev", "LevelEditor.SplinePanel.Previous", -1isize),
                ("next", "LevelEditor.SplinePanel.Next", 1),
            ] {
                position.push(self.action(
                    id,
                    key,
                    false,
                    move |s| {
                        if let Some((_, d)) = splines::selected(s) {
                            if !d.points.is_empty() {
                                s.editor.spline.selected_point = Some(
                                    (i as isize + delta).clamp(0, d.points.len() as isize - 1)
                                        as usize,
                                );
                            }
                        }
                    },
                    cx,
                ));
            }
            position.push(self.action(
                "point-duplicate",
                "LevelEditor.SplinePanel.DuplicatePoint",
                object.locked,
                move |s| {
                    splines::edit(s, |d| {
                        if let Some(p) = d.points.get(i).cloned() {
                            d.points.insert(i + 1, p);
                        }
                    });
                    s.editor.spline.selected_point = Some(i + 1);
                },
                cx,
            ));
            position.push(self.action(
                "point-remove",
                "LevelEditor.SplinePanel.DeletePoint",
                object.locked,
                move |s| {
                    splines::edit(s, |d| {
                        if i < d.points.len() {
                            d.points.remove(i);
                        }
                    })
                },
                cx,
            ));
            result.push(self.section("LevelEditor.SplinePanel.Position", position, cx));
            if matches!(
                curve.algorithm,
                CurveAlgorithm::Bezier | CurveAlgorithm::Hermite
            ) {
                for (incoming, key) in [
                    (true, "LevelEditor.SplinePanel.Arrive"),
                    (false, "LevelEditor.SplinePanel.Leave"),
                ] {
                    let values = if incoming { p.arrive } else { p.leave };
                    let mut controls = vec![];
                    for (axis, axis_key) in [
                        "LevelEditor.SplinePanel.X",
                        "LevelEditor.SplinePanel.Y",
                        "LevelEditor.SplinePanel.Z",
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        controls.push(self.curve_number(
                            object,
                            &format!("tangent-{i}-{incoming}-{axis}"),
                            axis_key,
                            values[axis],
                            -1e7,
                            1e7,
                            move |d, v| {
                                if let Some(p) = d.points.get_mut(i) {
                                    if incoming {
                                        p.arrive[axis] = v;
                                    } else {
                                        p.leave[axis] = v;
                                    }
                                }
                            },
                            window,
                            cx,
                        ));
                    }
                    result.push(self.section(key, controls, cx));
                }
                let actions = vec![
                    self.action(
                        "mirror-handles",
                        "LevelEditor.SplinePanel.MirrorHandles",
                        object.locked,
                        move |s| {
                            splines::edit(s, |d| {
                                if let Some(p) = d.points.get_mut(i) {
                                    p.arrive = p.leave;
                                }
                            })
                        },
                        cx,
                    ),
                    self.action(
                        "zero-handles",
                        "LevelEditor.SplinePanel.ZeroHandles",
                        object.locked,
                        move |s| {
                            splines::edit(s, |d| {
                                if let Some(p) = d.points.get_mut(i) {
                                    p.arrive = [0.; 3];
                                    p.leave = [0.; 3];
                                }
                            })
                        },
                        cx,
                    ),
                ];
                result.push(self.section("LevelEditor.SplinePanel.TangentTools", actions, cx));
            }
        }
        result
    }
    fn curve(
        &mut self,
        object: &SceneObjectData,
        curve: &SplineData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut algorithms = vec![];
        for algorithm in CurveAlgorithm::ALL {
            let state = self.state.clone();
            let btn = Button::new(format!("algorithm-{algorithm:?}"))
                .label(t!(algorithm.key()))
                .small()
                .disabled(object.locked)
                .on_click(cx.listener(move |_, _, _, cx| {
                    splines::edit(&mut state.write(), |d| {
                        d.algorithm = algorithm;
                        d.auto_tangents();
                    });
                    cx.notify();
                }));
            algorithms.push(
                if algorithm == curve.algorithm {
                    btn.primary()
                } else {
                    btn.ghost()
                }
                .into_any_element(),
            );
        }
        algorithms.push(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t!(curve.algorithm.description()).to_string())
                .into_any_element(),
        );
        let mut settings = vec![
            self.toggle(
                "closed",
                "LevelEditor.SplinePanel.ClosedLoop",
                curve.closed,
                |s, v| splines::edit(s, |d| d.closed = v),
                cx,
            ),
            self.curve_number(
                object,
                "resolution",
                "LevelEditor.SplinePanel.Resolution",
                curve.resolution as f32,
                4.,
                128.,
                |d, v| d.resolution = v as u32,
                window,
                cx,
            ),
        ];
        if curve.algorithm == CurveAlgorithm::CatmullRom {
            settings.push(self.curve_number(
                object,
                "tension",
                "LevelEditor.SplinePanel.Tension",
                curve.tension,
                0.,
                1.,
                |d, v| d.tension = v,
                window,
                cx,
            ));
        }
        if matches!(
            curve.algorithm,
            CurveAlgorithm::Bezier | CurveAlgorithm::Hermite
        ) {
            settings.push(self.action(
                "auto-tangents",
                "LevelEditor.SplinePanel.AutoTangents",
                object.locked,
                |s| splines::edit(s, |d| d.auto_tangents()),
                cx,
            ));
        }
        vec![
            self.section("LevelEditor.SplinePanel.Algorithm", algorithms, cx),
            self.section("LevelEditor.SplinePanel.Interpolation", settings, cx),
        ]
    }
    fn tools(
        &mut self,
        selected: &Option<(SceneObjectData, SplineData)>,
        settings: &SplineDomain,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut plane = vec![];
        for (p, label) in [
            (DrawingPlane::XZ, "XZ"),
            (DrawingPlane::XY, "XY"),
            (DrawingPlane::YZ, "YZ"),
        ] {
            let state = self.state.clone();
            let b = Button::new(format!("plane-{label}"))
                .label(label)
                .small()
                .on_click(cx.listener(move |_, _, _, cx| {
                    state.write().editor.spline.plane = p;
                    cx.notify();
                }));
            plane.push(
                if settings.plane == p {
                    b.primary()
                } else {
                    b.ghost()
                }
                .into_any_element(),
            );
        }
        plane.push(self.number(
            "plane-offset".into(),
            "LevelEditor.SplinePanel.PlaneOffset",
            settings.plane_offset,
            -1e7,
            1e7,
            |s, v| s.editor.spline.plane_offset = v,
            window,
            cx,
        ));
        plane.push(self.toggle(
            "snap",
            "LevelEditor.SplinePanel.Snap",
            settings.snap,
            |s, v| s.editor.spline.snap = v,
            cx,
        ));
        plane.push(self.number(
            "snap-step".into(),
            "LevelEditor.SplinePanel.SnapStep",
            settings.snap_step,
            0.001,
            10000.,
            |s, v| s.editor.spline.snap_step = v,
            window,
            cx,
        ));
        let mut result = vec![self.section("LevelEditor.SplinePanel.Drawing", plane, cx)];
        let disabled = selected
            .as_ref()
            .is_none_or(|(o, d)| o.locked || d.points.len() < 2);
        let mut operations = vec![
            self.number(
                "resample-count".into(),
                "LevelEditor.SplinePanel.ResampleCount",
                settings.resample_count as f32,
                2.,
                512.,
                |s, v| s.editor.spline.resample_count = v as usize,
                window,
                cx,
            ),
            self.action(
                "resample",
                "LevelEditor.SplinePanel.Resample",
                disabled,
                |s| {
                    let n = s.editor.spline.resample_count;
                    splines::edit(s, |d| d.resample(n));
                    s.editor.spline.selected_point = None;
                },
                cx,
            ),
            self.number(
                "smooth-strength".into(),
                "LevelEditor.SplinePanel.SmoothStrength",
                settings.smooth_strength,
                0.,
                1.,
                |s, v| s.editor.spline.smooth_strength = v,
                window,
                cx,
            ),
            self.action(
                "smooth",
                "LevelEditor.SplinePanel.Smooth",
                disabled,
                |s| {
                    let strength = s.editor.spline.smooth_strength;
                    splines::edit(s, |d| d.smooth(strength));
                },
                cx,
            ),
            self.action(
                "reverse",
                "LevelEditor.SplinePanel.Reverse",
                disabled,
                |s| {
                    splines::edit(s, |d| d.reverse());
                    s.editor.spline.selected_point = None;
                },
                cx,
            ),
        ];
        operations.push(self.action(
            "flatten",
            "LevelEditor.SplinePanel.Flatten",
            disabled,
            |s| {
                let axis = s.editor.spline.plane.normal_axis();
                let offset = s.editor.spline.plane_offset;
                if let Some((object, _)) = splines::selected(s) {
                    let m = splines::matrix(&object);
                    splines::edit(s, |d| {
                        for p in &mut d.points {
                            let mut world =
                                m.transform_point3(glam::Vec3::from(p.position)).to_array();
                            world[axis] = offset;
                            if let Some(local) = splines::local_point(&object, world) {
                                p.position = local;
                            }
                        }
                        d.auto_tangents();
                    });
                }
            },
            cx,
        ));
        operations.push(self.action(
            "snap-all",
            "LevelEditor.SplinePanel.SnapAll",
            disabled,
            |s| {
                let step = s.editor.spline.snap_step.max(0.001);
                if let Some((object, _)) = splines::selected(s) {
                    let m = splines::matrix(&object);
                    splines::edit(s, |d| {
                        for p in &mut d.points {
                            let world = m
                                .transform_point3(glam::Vec3::from(p.position))
                                .to_array()
                                .map(|v| (v / step).round() * step);
                            if let Some(local) = splines::local_point(&object, world) {
                                p.position = local;
                            }
                        }
                    });
                }
            },
            cx,
        ));
        operations.push(
            self.action(
                "clear",
                "LevelEditor.SplinePanel.Clear",
                selected
                    .as_ref()
                    .is_none_or(|(o, d)| o.locked || d.points.is_empty()),
                |s| splines::edit(s, |d| d.points.clear()),
                cx,
            ),
        );
        result.push(self.section("LevelEditor.SplinePanel.Operations", operations, cx));
        let display = vec![
            self.toggle(
                "show-all",
                "LevelEditor.SplinePanel.ShowAll",
                settings.show_all,
                |s, v| s.editor.spline.show_all = v,
                cx,
            ),
            self.toggle(
                "show-points",
                "LevelEditor.SplinePanel.ShowPoints",
                settings.show_points,
                |s, v| s.editor.spline.show_points = v,
                cx,
            ),
            self.toggle(
                "show-polygon",
                "LevelEditor.SplinePanel.ShowPolygon",
                settings.show_polygon,
                |s, v| s.editor.spline.show_polygon = v,
                cx,
            ),
            self.toggle(
                "show-tangents",
                "LevelEditor.SplinePanel.ShowTangents",
                settings.show_tangents,
                |s, v| s.editor.spline.show_tangents = v,
                cx,
            ),
            self.number(
                "line-width".into(),
                "LevelEditor.SplinePanel.LineWidth",
                settings.line_width,
                1.,
                6.,
                |s, v| s.editor.spline.line_width = v,
                window,
                cx,
            ),
        ];
        result.push(self.section("LevelEditor.SplinePanel.Display", display, cx));
        result
    }
}
impl EventEmitter<ui::dock::PanelEvent> for SplinePanel {}
ui_common::panel_boilerplate!(SplinePanel);
impl Render for SplinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_pump(window, cx);
        let (rows, selected, settings, undo, redo) = {
            let state = self.state.read();
            (
                splines::all(&state),
                splines::selected(&state),
                state.editor.spline.clone(),
                state.scene.can_undo(),
                state.scene.can_redo(),
            )
        };
        let target = selected
            .as_ref()
            .map(|(o, _)| (o.id.clone(), settings.selected_point));
        if target != self.field_target {
            self.fields.clear();
            self.field_target = target;
        }
        let theme = cx.theme().clone();
        let mut tabs = h_flex().w_full().gap_1();
        for tab in Tab::ALL {
            let b = Button::new(tab.key())
                .label(t!(tab.key()))
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }));
            tabs = tabs.child(if self.tab == tab {
                b.primary()
            } else {
                b.ghost()
            });
        }
        let mut tools = h_flex().w_full().flex_wrap().gap_1();
        for tool in SplineTool::ALL {
            let state = self.state.clone();
            let b = Button::new(tool.key())
                .label(t!(tool.key()))
                .small()
                .on_click(cx.listener(move |_, _, _, cx| {
                    state.write().editor.spline.tool = tool;
                    cx.notify();
                }));
            tools = tools.child(if settings.tool == tool {
                b.primary()
            } else {
                b.ghost()
            });
        }
        let content = match (self.tab, &selected) {
            (Tab::Manage, _) => self.manage(&rows, &selected, window, cx),
            (Tab::Points, Some((o, d))) => self.points(o, d, &settings, window, cx),
            (Tab::Curve, Some((o, d))) => self.curve(o, d, window, cx),
            (Tab::Tools, _) => self.tools(&selected, &settings, window, cx),
            _ => vec![
                div()
                    .p_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t!("LevelEditor.SplinePanel.SelectSpline").to_string())
                    .into_any_element(),
            ],
        };
        let mut body = v_flex()
            .id("spline-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3();
        if let Some((object, curve)) = &selected {
            body = body
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(object.name.clone()),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} {}",
                            curve.points.len(),
                            t!("LevelEditor.Spline.PointCount")
                        ))
                        .child(format!("{:.2} m", curve.total_length_m())),
                )
                .child(
                    div()
                        .border_1()
                        .border_color(theme.border)
                        .rounded_md()
                        .bg(theme.muted.opacity(0.12))
                        .child(super::super::spline_preview::panel(
                            curve.clone(),
                            settings.clone(),
                            theme.primary,
                            theme.muted_foreground,
                        )),
                );
            if object.locked {
                body = body.child(
                    div()
                        .text_sm()
                        .child(t!("LevelEditor.SplinePanel.LockedHint").to_string()),
                );
            }
        }
        body = body.children(content);
        if let Some(error) = &self.validation {
            body = body.child(div().text_xs().child(error.clone()));
        }
        let undo_owner = self.owner.clone();
        let redo_owner = self.owner.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::MapPin)
                                    .size_5()
                                    .text_color(theme.primary),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::BOLD)
                                    .child(t!("LevelEditor.SplinePanel.Title").to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{}", rows.len())),
                            ),
                    )
                    .child(tabs)
                    .child(tools)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("LevelEditor.SplinePanel.ToolHint").to_string()),
                    ),
            )
            .child(body)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .p_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("spline-undo")
                            .label(t!("LevelEditor.SplinePanel.Undo"))
                            .small()
                            .disabled(!undo)
                            .on_click(move |_, window, cx| {
                                let _ = undo_owner.update(cx, |owner, cx| {
                                    owner.on_undo(&super::super::actions::Undo, window, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new("spline-redo")
                            .label(t!("LevelEditor.SplinePanel.Redo"))
                            .small()
                            .disabled(!redo)
                            .on_click(move |_, window, cx| {
                                let _ = redo_owner.update(cx, |owner, cx| {
                                    owner.on_redo(&super::super::actions::Redo, window, cx)
                                });
                            }),
                    ),
            )
    }
}
impl ui::dock::Panel for SplinePanel {
    fn panel_name(&self) -> &'static str {
        "spline.panel"
    }
    fn title(&self, _: &Window, _: &App) -> AnyElement {
        t!("LevelEditor.SplinePanel.Title")
            .to_string()
            .into_any_element()
    }
}
