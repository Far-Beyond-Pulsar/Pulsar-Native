//! Splines (`pulsar.splines`): the component crate of the plugin that owns
//! authored curves.
//!
//! - The component ([`component`]): `SplineComponent`, the curve and every
//!   edit the editor's spline tools make, as script methods (`reverse`,
//!   `auto_tangents`, `smooth`, `resample`, `insert_at`, `total_length_m`):
//!   the editor calls the same methods scripts do.
//! - Its editor lines: the engine's viewports draw every visible curve
//!   through the renderer's editor debug lines ([`SplineLines`], an
//!   `engine_backend` editor line set). Splines have no Helio feature.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate splines_component`; the editor through
//! the plugin's editor crate, `splines_plugin`) and know no spline type.

pub mod component;

pub use component::*;

impl engine_backend::subsystems::render::EditorLines for SplineLines {
    fn poll(&mut self, world: &pulsar_scenedb::World) -> Option<Vec<helio::WorldVertex>> {
        SplineLines::poll(self, world)
    }
}

fn new_spline_lines(
    world: &pulsar_scenedb::World,
) -> Box<dyn engine_backend::subsystems::render::EditorLines> {
    Box::new(SplineLines::new(world))
}

inventory::submit! {
    engine_backend::subsystems::render::EditorLinesRegistration {
        name: "splines",
        new: new_spline_lines,
    }
}

fn spline(instance: &mut dyn pulsar_reflection::EngineClass) -> Option<&mut SplineComponent> {
    instance.as_any_mut().downcast_mut::<SplineComponent>()
}

/// The spline's script methods (beside its property accessors).
fn spline_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    use pulsar_reflection::{
        MethodFlags, MethodMetadata, MethodParameter, MethodReturnType, RUNTIME_TYPE_REGISTRY,
    };
    let (Some(f32_info), Some(i32_info)) = (
        RUNTIME_TYPE_REGISTRY.get::<f32>(),
        RUNTIME_TYPE_REGISTRY.get::<i32>(),
    ) else {
        return Vec::new();
    };
    let method = |name: &'static str, display_name: &str| MethodMetadata {
        name,
        display_name: display_name.into(),
        category: Some("Spline"),
        params: Vec::new(),
        return_type: None,
        flags: MethodFlags::NONE,
        caller: Box::new(|_, _| None),
    };
    vec![
        MethodMetadata {
            caller: Box::new(|instance, _| {
                spline(instance)?.reverse();
                Some(Box::new(()))
            }),
            ..method("reverse", "Reverse")
        },
        MethodMetadata {
            caller: Box::new(|instance, _| {
                spline(instance)?.auto_tangents();
                Some(Box::new(()))
            }),
            ..method("auto_tangents", "Auto Tangents")
        },
        MethodMetadata {
            params: vec![MethodParameter {
                name: "strength",
                type_info: f32_info,
            }],
            caller: Box::new(|instance, args| {
                let strength = *args.first()?.downcast_ref::<f32>()?;
                spline(instance)?.smooth(strength);
                Some(Box::new(()))
            }),
            ..method("smooth", "Smooth")
        },
        MethodMetadata {
            params: vec![MethodParameter {
                name: "count",
                type_info: i32_info,
            }],
            caller: Box::new(|instance, args| {
                let count = *args.first()?.downcast_ref::<i32>()?;
                spline(instance)?.resample(count.max(0) as usize);
                Some(Box::new(()))
            }),
            ..method("resample", "Resample")
        },
        MethodMetadata {
            params: vec![MethodParameter {
                name: "t",
                type_info: f32_info,
            }],
            return_type: Some(MethodReturnType {
                type_info: i32_info,
            }),
            caller: Box::new(|instance, args| {
                let t = *args.first()?.downcast_ref::<f32>()?;
                let index = spline(instance)?
                    .insert_at(t)
                    .map_or(-1, |index| index as i32);
                Some(Box::new(index))
            }),
            ..method("insert_at", "Insert At")
        },
        MethodMetadata {
            return_type: Some(MethodReturnType {
                type_info: f32_info,
            }),
            caller: Box::new(|instance, _| Some(Box::new(spline(instance)?.total_length_m()))),
            ..method("total_length_m", "Total Length (m)")
        },
    ]
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::ComponentMethodRegistration {
        class_name: SPLINE_CLASS_NAME,
        methods: spline_methods,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::EngineClass;

    #[test]
    fn the_editors_operations_are_script_methods() {
        let methods = <SplineComponent as EngineClass>::get_methods();
        for name in [
            "reverse",
            "auto_tangents",
            "smooth",
            "resample",
            "insert_at",
            "total_length_m",
        ] {
            assert!(
                methods.iter().any(|method| method.name == name),
                "{name} is not a script method"
            );
        }
        let mut spline = SplineComponent::default();
        let resample = methods
            .iter()
            .find(|method| method.name == "resample")
            .unwrap();
        (resample.caller)(&mut spline, vec![Box::new(5i32)]).unwrap();
        assert_eq!(spline.points.len(), 5);
    }

    #[test]
    fn registers_its_editor_lines() {
        assert!(
            inventory::iter::<engine_backend::subsystems::render::EditorLinesRegistration>
                .into_iter()
                .any(|registration| registration.name == "splines")
        );
    }
}
