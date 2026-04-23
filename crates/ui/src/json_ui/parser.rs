use crate::json_ui::schema::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct UiParser {
    base_path: PathBuf,
    cache: HashMap<PathBuf, UiDocument>,
}

impl UiParser {
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            cache: HashMap::new(),
        }
    }

    pub fn parse_file(path: impl AsRef<Path>) -> Result<UiComponent, Box<dyn std::error::Error>> {
        let mut parser = Self::new(path.as_ref().parent().unwrap_or(Path::new(".")));
        parser.parse_document(path.as_ref())
    }

    pub fn parse_document(
        &mut self,
        path: &Path,
    ) -> Result<UiComponent, Box<dyn std::error::Error>> {
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base_path.join(path)
        };

        if let Some(cached) = self.cache.get(&absolute_path) {
            return Ok(cached.root.clone());
        }

        let content = fs::read_to_string(&absolute_path)?;
        let document: UiDocument = serde_json::from_str(&content)?;

        let resolved = self.resolve_component(&document.root)?;
        self.cache.insert(absolute_path, document);

        Ok(resolved)
    }

    fn resolve_component(
        &mut self,
        component: &UiComponent,
    ) -> Result<UiComponent, Box<dyn std::error::Error>> {
        self.resolve_component_with_props(component, &HashMap::new())
    }

    fn resolve_component_with_props(
        &mut self,
        component: &UiComponent,
        inherited_props: &HashMap<String, UiValue>,
    ) -> Result<UiComponent, Box<dyn std::error::Error>> {
        if let Some(ref_path) = &component.reference {
            let referenced_path = self.base_path.join(ref_path);
            let referenced_component = self.parse_document(&referenced_path)?;

            let mut merged_props = inherited_props.clone();
            for (key, value) in &component.props {
                merged_props.insert(key.clone(), value.clone());
            }

            let mut resolved = referenced_component;
            resolved.props = self.interpolate_props(&resolved.props, &merged_props);
            resolved.children =
                self.resolve_children_with_props(&resolved.children, &merged_props)?;
            return Ok(resolved);
        }

        let mut resolved_children = Vec::new();
        for child in &component.children {
            resolved_children.push(self.resolve_child_with_props(child, inherited_props)?);
        }

        Ok(UiComponent {
            component_type: component.component_type.clone(),
            props: self.interpolate_props(&component.props, inherited_props),
            children: resolved_children,
            reference: None,
        })
    }

    fn interpolate_props(
        &self,
        props: &HashMap<String, UiValue>,
        context: &HashMap<String, UiValue>,
    ) -> HashMap<String, UiValue> {
        let mut interpolated = HashMap::new();

        for (key, value) in props {
            interpolated.insert(key.clone(), self.interpolate_value(value, context));
        }

        interpolated
    }

    fn interpolate_value(&self, value: &UiValue, context: &HashMap<String, UiValue>) -> UiValue {
        match value {
            UiValue::String(s) => {
                if s.starts_with("${") && s.ends_with("}") {
                    let var_name = &s[2..s.len() - 1];
                    context.get(var_name).unwrap_or(value).clone()
                } else {
                    value.clone()
                }
            }
            UiValue::Array(arr) => UiValue::Array(
                arr.iter()
                    .map(|v| self.interpolate_value(v, context))
                    .collect(),
            ),
            UiValue::Object(obj) => {
                let mut interpolated_obj = HashMap::new();
                for (k, v) in obj {
                    interpolated_obj.insert(k.clone(), self.interpolate_value(v, context));
                }
                UiValue::Object(interpolated_obj)
            }
            _ => value.clone(),
        }
    }

    fn resolve_child(&mut self, child: &UiChild) -> Result<UiChild, Box<dyn std::error::Error>> {
        self.resolve_child_with_props(child, &HashMap::new())
    }

    fn resolve_child_with_props(
        &mut self,
        child: &UiChild,
        inherited_props: &HashMap<String, UiValue>,
    ) -> Result<UiChild, Box<dyn std::error::Error>> {
        match child {
            UiChild::Component(component) => Ok(UiChild::Component(
                self.resolve_component_with_props(component, inherited_props)?,
            )),
            UiChild::Text(text) => Ok(UiChild::Text(text.clone())),
            UiChild::Reference { reference, props } => {
                let referenced_path = self.base_path.join(reference);
                let mut referenced_component = self.parse_document(&referenced_path)?;

                let mut merged_props = inherited_props.clone();
                for (key, value) in props {
                    merged_props.insert(key.clone(), value.clone());
                }

                referenced_component.props =
                    self.interpolate_props(&referenced_component.props, &merged_props);
                referenced_component.children = self
                    .resolve_children_with_props(&referenced_component.children, &merged_props)?;

                Ok(UiChild::Component(referenced_component))
            }
        }
    }

    fn resolve_children(
        &mut self,
        children: &[UiChild],
    ) -> Result<Vec<UiChild>, Box<dyn std::error::Error>> {
        self.resolve_children_with_props(children, &HashMap::new())
    }

    fn resolve_children_with_props(
        &mut self,
        children: &[UiChild],
        inherited_props: &HashMap<String, UiValue>,
    ) -> Result<Vec<UiChild>, Box<dyn std::error::Error>> {
        children
            .iter()
            .map(|child| self.resolve_child_with_props(child, inherited_props))
            .collect()
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    pub fn parse_from_string(
        &mut self,
        content: &str,
    ) -> Result<UiComponent, Box<dyn std::error::Error>> {
        let document: UiDocument = serde_json::from_str(content)?;
        self.resolve_component(&document.root)
    }

    pub fn remove_from_cache(&mut self, path: &Path) {
        self.cache.remove(path);
    }
}
