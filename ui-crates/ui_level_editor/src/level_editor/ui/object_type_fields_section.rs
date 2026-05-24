use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyType, PropertyValue, RuntimeTypeInfo, TypeStructure, WrapperType, REGISTRY};
use serde_json::Value;
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::popover::Popover;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};
use ui_common::{
    AssetPickedEvent, AssetQuery, MeshAssetPicker, PropertyStateManager,
    json_to_property_value, property_value_to_json, render_component_section,
};

use super::add_component_dialog::AddComponentDialog;
use super::state::LevelEditorState;
use super::ComponentHierarchyPanel;
use crate::level_editor::scene_database::{ObjectType, SceneDatabase};

const OBJECT_ICON_PROP_KEY: &str = "icon_asset";
const OBJECT_ICON_PICKER_SCOPE: &str = "__object__";

pub struct ObjectTypeFieldsSection {
    object_id: String,
    scene_db: SceneDatabase,
    /// Selected component index in the component list (for highlighting).
    selected_component: Option<usize>,
    /// Add component dialog entity
    add_component_dialog: Entity<AddComponentDialog>,
    /// Shared state for expand/collapse tracking
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    /// Shared property state manager
    property_state: PropertyStateManager,
    /// Icon asset picker (special case for object-level icon)
    icon_asset_picker: Option<Entity<MeshAssetPicker>>,
}

impl ObjectTypeFieldsSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create the add component dialog entity
        let dialog_object_id = object_id.clone();
        let dialog_scene_db = scene_db.clone();
        let add_component_dialog =
            cx.new(|cx| AddComponentDialog::new(dialog_object_id, dialog_scene_db, window, cx));

        // Subscribe to ComponentAddedEvent to refresh the UI
        cx.subscribe(
            &add_component_dialog,
            |this, _dialog, event: &super::add_component_dialog::ComponentAddedEvent, cx| {
                // Refresh the UI when a component is added
                let _ = event; // Event contains class_name but we don't need it
                cx.notify();
            },
        )
        .detach();

        Self {
            object_id,
            scene_db,
            selected_component: None,
            add_component_dialog,
            state_arc,
            property_state: PropertyStateManager::new(),
            icon_asset_picker: None,
        }
    }

    fn read_property(
        &self,
        class_name: &str,
        property_name: &str,
        property_type: &PropertyType,
        default_value: &PropertyValue,
    ) -> PropertyValue {
        let components = self.scene_db.get_components(&self.object_id);
        let component = components.iter().find(|c| c.class_name == class_name);

        if let Some(component) = component {
            if let Some(value) = component.data.get(property_name) {
                if let Some(parsed) = json_to_property_value(property_type, value) {
                    return parsed;
                }
            }
        }

        default_value.clone()
    }

    fn write_property(&self, class_name: &str, property_name: &str, value: PropertyValue) {
        let components = self.scene_db.get_components(&self.object_id);
        let Some((idx, component)) = components
            .iter()
            .enumerate()
            .find(|(_, c)| c.class_name == class_name)
        else {
            return;
        };

        let mut map = component
            .data
            .as_object()
            .cloned()
            .unwrap_or_else(serde_json::Map::new);
        map.insert(
            property_name.to_string(),
            property_value_to_json(&value),
        );

        self.scene_db
            .update_component(&self.object_id, idx, Value::Object(map));
    }

    fn read_object_icon_path(&self) -> String {
        self.scene_db
            .get_object(&self.object_id)
            .and_then(|obj| obj.props.get(OBJECT_ICON_PROP_KEY).cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn write_object_icon_path(&self, path: String) {
        let Some(mut obj) = self.scene_db.get_object(&self.object_id) else {
            return;
        };

        if path.is_empty() {
            obj.props.remove(OBJECT_ICON_PROP_KEY);
        } else {
            obj.props
                .insert(OBJECT_ICON_PROP_KEY.to_string(), Value::String(path));
        }

        let _ = self.scene_db.update_object(obj);
    }

    fn ensure_object_icon_picker(&mut self, current: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.icon_asset_picker.is_some() {
            return;
        }

        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let queries = vec![
            AssetQuery::extension("png"),
            AssetQuery::extension("jpg"),
            AssetQuery::extension("jpeg"),
            AssetQuery::extension("webp"),
        ];

        let picker = cx.new(|cx| {
            MeshAssetPicker::new(
                current.to_string(),
                vec![],
                project_root,
                queries,
                window,
                cx,
            )
        });

        cx.subscribe(&picker, move |this, picker, _event: &AssetPickedEvent, cx| {
            let selected = picker.read(cx).selected_path().to_string();
            this.write_object_icon_path(selected);
            cx.notify();
        })
        .detach();

        self.icon_asset_picker = Some(picker);
    }
}

impl Render for ObjectTypeFieldsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // ── Object type label ─────────────────────────────────────────────
        let type_label = match self.scene_db.get_object(&self.object_id) {
            Some(obj) => match obj.object_type {
                ObjectType::Empty => "Empty".to_string(),
                ObjectType::Folder => "Folder".to_string(),
                ObjectType::Camera => "Camera".to_string(),
                ObjectType::ParticleSystem => "Particle System".to_string(),
                ObjectType::AudioSource => "Audio Source".to_string(),
                ObjectType::Light(lt) => format!("Light ({lt:?})"),
                ObjectType::Mesh(mt) => format!("Mesh ({mt:?})"),
            },
            None => "Unknown".to_string(),
        };

        // ── Type card (always rendered) ───────────────────────────────────
        let type_card = v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Object Type"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(type_label),
                    ),
            );

        // ── Object icon picker row ─────────────────────────────────────────
        let object_icon_path = self.read_object_icon_path();
        self.ensure_object_icon_picker(&object_icon_path, window, cx);
        let object_icon_row = if let Some(picker) = self.icon_asset_picker.clone() {
            let display = if object_icon_path.is_empty() {
                "Select icon asset...".to_string()
            } else {
                std::path::Path::new(&object_icon_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&object_icon_path)
                    .to_string()
            };
            let thumb = picker.read(cx).thumbnail_for_path(&object_icon_path);
            let pop = Popover::<MeshAssetPicker>::new(format!(
                "object-icon-picker-{}",
                self.object_id
            ))
            .anchor(Corner::BottomRight)
            .trigger(
                ui::button::Button::new(format!("object-icon-btn-{}", self.object_id))
                    .label(display)
                    .small()
                    .ghost()
                    .dropdown_caret(true),
            )
            .content(move |_window, _cx| picker.clone())
            .into_any_element();

            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .p_3()
                .bg(cx.theme().sidebar)
                .rounded(px(8.0))
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("Object Icon"),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(pop)
                        .map(|el| match thumb {
                            Some(render_img) => el.child(
                                div()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded(px(4.0))
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .flex_shrink_0()
                                    .child(
                                        gpui::img(gpui::ImageSource::Render(render_img))
                                            .w(px(32.0))
                                            .h(px(32.0))
                                            .object_fit(gpui::ObjectFit::Cover),
                                    ),
                            ),
                            None => el,
                        }),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // ── Attached components ───────────────────────────────────────────
        let attached = self.scene_db.get_components(&self.object_id);

        // Diagnostics
        let registry_classes = REGISTRY.get_class_names();
        tracing::debug!(
            "[ObjectTypeFieldsSection] object_id={} attached={} registry={}",
            self.object_id,
            attached.len(),
            registry_classes.len(),
        );

        let diag_card: Option<AnyElement> = if attached.is_empty() {
            Some(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_3()
                    .bg(cx.theme().sidebar)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("⚠ No components attached"),
                    )
                    .into_any_element(),
            )
        } else if attached
            .iter()
            .all(|c| !REGISTRY.has_class(c.class_name.as_str()))
        {
            Some(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_3()
                    .bg(cx.theme().sidebar)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("⚠ Components not found in registry"),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        // ── Component hierarchy panel ─────────────────────────────────────
        let dialog = self.add_component_dialog.clone();
        let add_popover = Popover::<AddComponentDialog>::new("add-component-picker")
            .anchor(Corner::TopRight)
            .trigger(
                ui::button::Button::new("add-component-btn")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost(),
            )
            .content(move |_window, _cx| dialog.clone())
            .into_any_element();

        let component_hierarchy =
            ComponentHierarchyPanel::new(self.object_id.clone(), self.scene_db.clone());
        let state = self.state_arc.read();
        let component_panel = component_hierarchy
            .render(&state, self.state_arc.clone(), add_popover, cx)
            .into_any_element();
        drop(state);

        // ── Property sections for every attached component ─────────────────
        // Use shared panel system
        let component_sections = attached
            .iter()
            .filter_map(|component| {
                let class_name = component.class_name.as_str();
                let instance = REGISTRY.create_instance(class_name)?;
                let properties = instance.get_properties();
                if properties.is_empty() {
                    return None;
                }

                // Prepare properties data for shared renderer
                let mut props_data = Vec::new();
                for prop in properties {
                    let default_any = (prop.getter)(instance.as_ref());
                    let property_type = runtime_type_to_property_type(prop.type_info);
                    let default = any_to_property_value(default_any.as_ref(), prop.type_info)
                        .unwrap_or(PropertyValue::String("unsupported".to_string()));
                    let current = self.read_property(
                        class_name,
                        prop.name,
                        &property_type,
                        &default,
                    );

                    // Set up state for different property types
                    let numeric_input = match (&property_type, current.clone()) {
                        (PropertyType::F32 { step, .. }, PropertyValue::F32(v)) => {
                            let cls = class_name.to_string();
                            let pn = prop.name.to_string();
                            let db = self.scene_db.clone();
                            let oid = self.object_id.clone();
                            Some(self.property_state.ensure_f32_input(
                                class_name,
                                prop.name,
                                v,
                                step.unwrap_or(1.0),
                                move |new_val| {
                                    db.update_component_property(&oid, &cls, &pn, Value::from(new_val));
                                },
                                window,
                                cx,
                            ))
                        }
                        (PropertyType::I32 { .. }, PropertyValue::I32(v)) => {
                            let cls = class_name.to_string();
                            let pn = prop.name.to_string();
                            let db = self.scene_db.clone();
                            let oid = self.object_id.clone();
                            Some(self.property_state.ensure_i32_input(
                                class_name,
                                prop.name,
                                v,
                                move |new_val| {
                                    db.update_component_property(&oid, &cls, &pn, Value::from(new_val));
                                },
                                window,
                                cx,
                            ))
                        }
                        _ => None,
                    };

                    let mesh_picker = if matches!(&property_type, PropertyType::String { .. })
                        && prop.name == "mesh_asset"
                    {
                        if let PropertyValue::String(v) = &current {
                            let cls = class_name.to_string();
                            let pn = prop.name.to_string();
                            let db = self.scene_db.clone();
                            let oid = self.object_id.clone();
                            Some(self.property_state.ensure_mesh_asset_picker(
                                class_name,
                                prop.name,
                                v,
                                move |new_val| {
                                    db.update_component_property(&oid, &cls, &pn, Value::String(new_val));
                                },
                                window,
                                cx,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let should_create_color_picker = matches!(&property_type, PropertyType::Color)
                        || (matches!(&default, PropertyValue::String(s) if s == "unsupported")
                            && ui_common::reflected_properties_panel::is_color_field_name(prop.name));

                    let color_picker = if should_create_color_picker {
                        let rgba = if let PropertyValue::Color(c) = current {
                            c
                        } else {
                            // Fallback read from JSON
                            let components = self.scene_db.get_components(&self.object_id);
                            components
                                .iter()
                                .find(|c| c.class_name == class_name)
                                .and_then(|comp| comp.data.get(prop.name))
                                .map(ui_common::reflected_properties_panel::json_to_rgba_fallback)
                                .unwrap_or([1.0, 1.0, 1.0, 1.0])
                        };
                        let cls = class_name.to_string();
                        let pn = prop.name.to_string();
                        let db = self.scene_db.clone();
                        let oid = self.object_id.clone();
                        Some(self.property_state.ensure_color_picker(
                            class_name,
                            prop.name,
                            rgba,
                            move |rgba| {
                                db.update_component_property(&oid, &cls, &pn, serde_json::json!(rgba));
                            },
                            window,
                            cx,
                        ))
                    } else {
                        None
                    };

                    props_data.push((
                        prop.display_name.to_string(),
                        prop.name.to_string(),
                        property_type,
                        current,
                        numeric_input,
                        color_picker,
                        mesh_picker,
                    ));
                }

                // Use shared component section renderer
                let db_bool = self.scene_db.clone();
                let oid_bool = self.object_id.clone();
                let cls_bool = class_name.to_string();
                let on_bool_toggle = Arc::new(move |prop_name: &str, checked: bool, _window: &mut Window, _cx: &mut App| {
                    db_bool.update_component_property(&oid_bool, &cls_bool, prop_name, Value::from(checked));
                });

                let db_enum = self.scene_db.clone();
                let oid_enum = self.object_id.clone();
                let cls_enum = class_name.to_string();
                let on_enum_select = Arc::new(move |prop_name: &str, ix: usize, _window: &mut Window, _cx: &mut App| {
                    db_enum.update_component_property(&oid_enum, &cls_enum, prop_name, Value::from(ix as u64));
                });

                Some(render_component_section(
                    "level",
                    class_name,
                    props_data,
                    on_bool_toggle,
                    on_enum_select,
                    cx,
                ))
            })
            .collect::<Vec<_>>();

        v_flex()
            .w_full()
            .gap_3()
            .child(type_card)
            .child(object_icon_row)
            .child(component_panel)
            .children(diag_card)
            .children(component_sections)
            .into_any_element()
    }
}

fn runtime_type_to_property_type(type_info: &RuntimeTypeInfo) -> PropertyType {
    match &type_info.structure {
        TypeStructure::Primitive => match type_info.base_name() {
            "f32" => PropertyType::F32 {
                min: None,
                max: None,
                step: None,
            },
            "i32" => PropertyType::I32 {
                min: None,
                max: None,
            },
            "bool" => PropertyType::Bool,
            "[f32; 3]" => PropertyType::Vec3,
            "[f32; 4]" => PropertyType::Color,
            _ => PropertyType::String { max_length: None },
        },
        TypeStructure::String => PropertyType::String { max_length: None },
        TypeStructure::Enum { variants } => PropertyType::Enum {
            variants: variants.to_vec(),
        },
        TypeStructure::Wrapper {
            wrapper_kind: WrapperType::Vec,
            inner,
        } => PropertyType::Vec {
            element_type: Box::new(runtime_type_to_property_type(inner)),
        },
        TypeStructure::Struct { .. } => PropertyType::Component {
            class_name: type_info.type_name,
        },
        TypeStructure::Wrapper { .. } => PropertyType::String { max_length: None },
    }
}

fn any_to_property_value(value: &dyn std::any::Any, type_info: &RuntimeTypeInfo) -> Option<PropertyValue> {
    match &type_info.structure {
        TypeStructure::Primitive => match type_info.base_name() {
            "f32" => value.downcast_ref::<f32>().copied().map(PropertyValue::F32),
            "i32" => value.downcast_ref::<i32>().copied().map(PropertyValue::I32),
            "bool" => value.downcast_ref::<bool>().copied().map(PropertyValue::Bool),
            "[f32; 3]" => value.downcast_ref::<[f32; 3]>().copied().map(PropertyValue::Vec3),
            "[f32; 4]" => value.downcast_ref::<[f32; 4]>().copied().map(PropertyValue::Color),
            _ => Some(PropertyValue::String("unsupported".to_string())),
        },
        TypeStructure::String => value
            .downcast_ref::<String>()
            .map(|v| PropertyValue::String(v.clone())),
        TypeStructure::Enum { variants } => value.downcast_ref::<String>().and_then(|name| {
            variants
                .iter()
                .position(|v| v == name)
                .map(PropertyValue::EnumVariant)
        }),
        TypeStructure::Wrapper {
            wrapper_kind: WrapperType::Vec,
            ..
        } => Some(PropertyValue::Vec(Vec::new())),
        TypeStructure::Struct { .. } => Some(PropertyValue::Component {
            class_name: type_info.base_name().to_string(),
        }),
        TypeStructure::Wrapper { .. } => Some(PropertyValue::String("unsupported".to_string())),
    }
}
