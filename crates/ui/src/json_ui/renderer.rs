use crate::button::Button;
use crate::json_ui::schema::*;
use crate::{gray, StyledExt};
use gpui::*;
use std::collections::HashMap;

pub struct UiRenderer;

impl UiRenderer {
    pub fn render_component(
        component: &UiComponent,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> AnyElement {
        Self::render_component_internal(component, &component.props, cx)
    }

    pub fn render_component_generic<T>(
        component: &UiComponent,
        _cx: &mut Context<T>,
    ) -> AnyElement {
        Self::render_component_standalone(component, &component.props)
    }

    fn render_component_internal(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> AnyElement {
        match component.component_type.as_str() {
            "div" => Self::render_div(component, props, cx).into_any_element(),
            "h1" => Self::render_h1(component, props, cx).into_any_element(),
            "h2" => Self::render_h2(component, props, cx).into_any_element(),
            "h3" => Self::render_h3(component, props, cx).into_any_element(),
            "button" => Self::render_button(component, props, cx).into_any_element(),
            "input" => Self::render_input(component, props, cx).into_any_element(),
            "text" => Self::render_text(component, props, cx).into_any_element(),
            "flex" => Self::render_flex(component, props, cx).into_any_element(),
            "column" => Self::render_column(component, props, cx).into_any_element(),
            "row" => Self::render_row(component, props, cx).into_any_element(),
            _ => Self::render_unknown(component, props, cx).into_any_element(),
        }
    }

    fn render_div(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div();

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_h1(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div().text_xl().font_bold();

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_h2(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div().text_lg().font_bold();

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_h3(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div().text_base().font_bold();

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_button(
        component: &UiComponent,
        _props: &HashMap<String, UiValue>,
        _cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let text = component
            .children
            .iter()
            .filter_map(|child| match child {
                UiChild::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");

        Button::new("json_button").label(text)
    }

    fn render_input(
        _component: &UiComponent,
        props: &HashMap<String, UiValue>,
        _cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let placeholder = props
            .get("placeholder")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        div()
            .border_1()
            .border_color(gpui::blue())
            .px_2()
            .py_1()
            .rounded_md()
            .child(placeholder)
    }

    fn render_text(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        _cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let content = props
            .get("content")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .or_else(|| {
                component
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        UiChild::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .next()
            })
            .unwrap_or_else(|| "".to_string());

        let mut element = div().child(content);
        element = Self::apply_common_props(element, props);
        element
    }

    fn render_flex(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div().flex();

        let direction = props
            .get("direction")
            .and_then(|v| v.as_string())
            .unwrap_or("row");

        element = match direction {
            "column" => element.flex_col(),
            _ => element.flex_row(),
        };

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_column(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div().flex().flex_col();

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_row(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div().flex().flex_row();

        element = Self::apply_common_props(element, props);
        element = Self::apply_children(element, &component.children, cx);

        element
    }

    fn render_unknown(
        component: &UiComponent,
        _props: &HashMap<String, UiValue>,
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> impl IntoElement {
        let mut element = div()
            .border_1()
            .border_color(gpui::red())
            .p_2()
            .child(format!("Unknown component: {}", component.component_type));

        element = Self::apply_children(element, &component.children, cx);
        element
    }

    fn apply_common_props<E: Styled>(mut element: E, props: &HashMap<String, UiValue>) -> E {
        if let Some(width) = props.get("width").and_then(|v| v.as_number()) {
            element = element.w(px(width as f32));
        }

        if let Some(height) = props.get("height").and_then(|v| v.as_number()) {
            element = element.h(px(height as f32));
        }

        if let Some(padding) = props.get("padding").and_then(|v| v.as_number()) {
            element = element.p(px(padding as f32));
        }

        if let Some(margin) = props.get("margin").and_then(|v| v.as_number()) {
            element = element.m(px(margin as f32));
        }

        if let Some(bg_color) = props.get("backgroundColor").and_then(|v| v.as_string()) {
            element = match bg_color {
                "red" => element.bg(gpui::red()),
                "blue" => element.bg(gpui::blue()),
                "green" => element.bg(gpui::green()),
                "yellow" => element.bg(gpui::yellow()),
                "gray" => element.bg(gray(500)),
                "white" => element.bg(gpui::white()),
                "black" => element.bg(gpui::black()),
                _ => element,
            };
        }

        if let Some(text_color) = props.get("color").and_then(|v| v.as_string()) {
            element = match text_color {
                "red" => element.text_color(gpui::red()),
                "blue" => element.text_color(gpui::blue()),
                "green" => element.text_color(gpui::green()),
                "yellow" => element.text_color(gpui::yellow()),
                "gray" => element.text_color(gray(500)),
                "white" => element.text_color(gpui::white()),
                "black" => element.text_color(gpui::black()),
                _ => element,
            };
        }

        element
    }

    fn apply_children<E: ParentElement>(
        mut element: E,
        children: &[UiChild],
        cx: &mut Context<crate::json_ui::JsonCanvas>,
    ) -> E {
        for child in children {
            match child {
                UiChild::Component(component) => {
                    element = element.child(Self::render_component_internal(
                        component,
                        &component.props,
                        cx,
                    ));
                }
                UiChild::Text(text) => {
                    element = element.child(text.clone());
                }
                UiChild::Reference { .. } => {
                    // References should have been resolved by the parser
                }
            }
        }
        element
    }

    fn render_component_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> AnyElement {
        match component.component_type.as_str() {
            "div" => Self::render_div_standalone(component, props).into_any_element(),
            "h1" => Self::render_h1_standalone(component, props).into_any_element(),
            "h2" => Self::render_h2_standalone(component, props).into_any_element(),
            "h3" => Self::render_h3_standalone(component, props).into_any_element(),
            "button" => Self::render_button_standalone(component, props).into_any_element(),
            "input" => Self::render_input_standalone(component, props).into_any_element(),
            "text" => Self::render_text_standalone(component, props).into_any_element(),
            "flex" => Self::render_flex_standalone(component, props).into_any_element(),
            "column" => Self::render_column_standalone(component, props).into_any_element(),
            "row" => Self::render_row_standalone(component, props).into_any_element(),
            _ => Self::render_unknown_standalone(component, props).into_any_element(),
        }
    }

    fn render_div_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div();
        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_h1_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div().text_xl().font_bold();
        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_h2_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div().text_lg().font_bold();
        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_h3_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div().text_base().font_bold();
        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_button_standalone(
        component: &UiComponent,
        _props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let text = component
            .children
            .iter()
            .filter_map(|child| match child {
                UiChild::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");

        Button::new("json_button").label(text)
    }

    fn render_input_standalone(
        _component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let placeholder = props
            .get("placeholder")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        div()
            .border_1()
            .border_color(gpui::blue())
            .px_2()
            .py_1()
            .rounded_md()
            .child(placeholder)
    }

    fn render_text_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let content = props
            .get("content")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .or_else(|| {
                component
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        UiChild::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .next()
            })
            .unwrap_or_else(|| "".to_string());

        let mut element = div().child(content);
        element = Self::apply_common_props(element, props);
        element
    }

    fn render_flex_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div().flex();

        let direction = props
            .get("direction")
            .and_then(|v| v.as_string())
            .unwrap_or("row");

        element = match direction {
            "column" => element.flex_col(),
            _ => element.flex_row(),
        };

        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_column_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div().flex().flex_col();
        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_row_standalone(
        component: &UiComponent,
        props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div().flex().flex_row();
        element = Self::apply_common_props(element, props);
        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn render_unknown_standalone(
        component: &UiComponent,
        _props: &HashMap<String, UiValue>,
    ) -> impl IntoElement {
        let mut element = div()
            .border_1()
            .border_color(gpui::red())
            .p_2()
            .child(format!("Unknown component: {}", component.component_type));

        element = Self::apply_children_standalone(element, &component.children);
        element
    }

    fn apply_children_standalone<E: ParentElement>(mut element: E, children: &[UiChild]) -> E {
        for child in children {
            match child {
                UiChild::Component(component) => {
                    element = element.child(Self::render_component_standalone(
                        component,
                        &component.props,
                    ));
                }
                UiChild::Text(text) => {
                    element = element.child(text.clone());
                }
                UiChild::Reference { .. } => {
                    // References should have been resolved by the parser
                }
            }
        }
        element
    }
}
