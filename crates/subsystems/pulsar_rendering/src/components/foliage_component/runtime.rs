use engine_class_derive::register_runtime_behavior;
use helio::{
    FoliageInteractor, FoliageLayer, FoliageTypeDescriptor, GpuMaterial, MaterialId, Renderer,
};
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet,
    RuntimeComponentOwner,
};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::FoliageComponent;
use crate::subsystems::{FoliageCache, FoliageEntry};

/// Park position for a disabled interactor — far below any real terrain so it
/// displaces nothing, without churning create/remove in the scene each sync.
const INTERACTOR_PARKED: [f32; 3] = [0.0, -100_000.0, 0.0];

fn hash_props<T: Serialize>(value: &T) -> u64 {
    let json = serde_json::to_string(value).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    hasher.finish()
}

/// World-space AABB of the layer, centred on the owner's XZ.
fn build_layer_bounds(component: &FoliageComponent, position: [f32; 3]) -> [glam::Vec3; 2] {
    let half = component.placement.layer_extent;
    [
        glam::Vec3::new(
            position[0] - half,
            component.placement.altitude_min,
            position[2] - half,
        ),
        glam::Vec3::new(
            position[0] + half,
            component.placement.altitude_max,
            position[2] + half,
        ),
    ]
}

fn build_layer(
    component: &FoliageComponent,
    type_id: helio::FoliageTypeId,
    position: [f32; 3],
) -> FoliageLayer {
    FoliageLayer {
        types: vec![type_id],
        bounds: build_layer_bounds(component, position),
        seed: 0x5EED,
        has_infinite_extent: component.placement.has_infinite_extent,
    }
}

/// Fingerprint of the layer-relevant props only. Bounds edits are the one piece of
/// placement the type update cannot express, so they get their own gate.
fn bounds_hash(component: &FoliageComponent) -> u64 {
    let p = &component.placement;
    hash_props(&(p.layer_extent, p.altitude_min, p.altitude_max, p.has_infinite_extent))
}

fn build_material(component: &FoliageComponent) -> GpuMaterial {
    let rendering = &component.rendering;
    GpuMaterial {
        base_color: rendering.base_color,
        emissive: [0.0, 0.0, 0.0, 0.0],
        roughness_metallic: [rendering.roughness, rendering.metallic, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness: GpuMaterial::NO_TEXTURE,
        tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion: GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        material_class: 0,
        class_params: [0.0; 4],
    }
}

fn build_type_descriptor(
    component: &FoliageComponent,
    material_id: MaterialId,
) -> FoliageTypeDescriptor {
    // `kind` stays on the descriptor default (`FoliageKind::Blade`) — grass.
    FoliageTypeDescriptor {
        density: component.general.density,
        height_range: [
            component.placement.height_min,
            component.placement.height_max,
        ],
        width_range: [component.placement.width_min, component.placement.width_max],
        slope_range: [
            component.placement.slope_min_degrees.to_radians(),
            component.placement.slope_max_degrees.to_radians(),
        ],
        altitude_range: [
            component.placement.altitude_min,
            component.placement.altitude_max,
        ],
        lod_distances: [
            component.rendering.lod_distance_0,
            component.rendering.lod_distance_1,
            component.rendering.lod_distance_2,
            component.rendering.lod_distance_3,
        ],
        wind_response: [
            component.wind.trunk_sway,
            component.wind.branch_flutter,
            component.wind.leaf_jitter,
        ],
        interaction_stiffness: component.wind.interaction_stiffness,
        material_id: material_id.slot(),
        density_layer: component.general.density_layer as u32,
        two_sided: component.rendering.two_sided,
        casts_shadow: component.rendering.casts_shadow,
        receives_interaction: component.interaction.receives_interaction,
        ..Default::default()
    }
}

/// Push the component's wind settings into the scene's global wind. The pinned
/// `helio` does not re-export the `Wind` struct, so we mutate the scene's own
/// `wind()` value in place — which also preserves the motion-vector clock.
fn apply_wind(scene: &mut helio::Scene, component: &FoliageComponent) {
    let wind = &component.wind;
    let mut current = scene.wind();
    current.direction = glam::Vec3::from_array(wind.wind_direction);
    current.speed = wind.wind_speed;
    current.gust_amplitude = wind.gust_amplitude;
    current.gust_frequency = wind.gust_frequency;
    current.turbulence_scale = wind.turbulence_scale;
    scene.set_wind(current);
}

fn remove_foliage_with_context(
    context: &mut dyn ComponentRuntimeContext,
    key: &str,
    entry: FoliageEntry,
) {
    {
        let renderer = get_subsystem!(context, Renderer);
        let scene = renderer.scene_mut();
        let _ = scene.remove_foliage_type(entry.type_id);
        let _ = scene.remove_foliage_layer(entry.layer_id);
        let _ = scene.remove_foliage_interactor(entry.interactor_id);
        let _ = scene.remove_material(entry.material_id);
    }
    let cache = get_subsystem!(context, FoliageCache);
    cache.map.remove(key);
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for FoliageComponent {
    const CLASS_NAME: &'static str = "FoliageComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let key = format!("{}:{component_index}", owner.scene_object_id);

        // Mark live so the editor's post-sync stale pass keeps our handles.
        get_subsystem!(context, LiveKeySet).insert(key.clone());

        // Read cached handles before borrowing the renderer.
        let cached = {
            let cache = get_subsystem!(context, FoliageCache);
            cache.map.get(&key).copied()
        };

        if !component.general.enabled {
            if let Some(entry) = cached {
                remove_foliage_with_context(context, &key, entry);
            }
            return;
        }

        let material_hash = hash_props(&component.rendering);
        let descriptor_hash = hash_props(component);
        let bounds_hash = bounds_hash(component);
        let wind_hash = hash_props(&component.wind);

        match cached {
            Some(entry) => {
                let position = owner.position;

                // Borrow the renderer only for actual scene mutations; every
                // subsystem access through `context` must be in its own scope.
                let entry = {
                    let renderer = get_subsystem!(context, Renderer);
                    let scene = renderer.scene_mut();

                    let mut entry = entry;
                    if entry.bounds_hash != bounds_hash {
                        // The layer table has no in-place update API, so a bounds edit
                        // re-registers the layer. Same type, same seed — only the AABB
                        // and the infinite-extent flag change. The generation bump the
                        // removal/insertion causes re-rolls the ring, which is exactly
                        // what an edit to "where grass grows" must do.
                        let _ = scene.remove_foliage_layer(entry.layer_id);
                        entry.layer_id =
                            scene.add_foliage_layer(build_layer(component, entry.type_id, position));
                        entry.bounds_hash = bounds_hash;
                    }
                    if entry.descriptor_hash != descriptor_hash {
                        let material_id = if entry.material_hash != material_hash {
                            let new_material = scene.insert_material(build_material(component));
                            let _ = scene.remove_material(entry.material_id);
                            new_material
                        } else {
                            entry.material_id
                        };
                        let _ = scene.update_foliage_type(
                            entry.type_id,
                            build_type_descriptor(component, material_id),
                        );
                        if component.wind.wind_enabled && entry.wind_hash != wind_hash {
                            apply_wind(scene, component);
                        }
                        entry.material_id = material_id;
                        entry.material_hash = material_hash;
                        entry.descriptor_hash = descriptor_hash;
                        entry.wind_hash = wind_hash;
                    }

                    let target = if component.interaction.interactor_enabled {
                        position
                    } else {
                        INTERACTOR_PARKED
                    };
                    let _ = scene.update_foliage_interactor(
                        entry.interactor_id,
                        glam::Vec3::from_array(target),
                        glam::Vec3::ZERO,
                    );
                    entry
                };

                let cache = get_subsystem!(context, FoliageCache);
                if let Some(existing) = cache.map.get_mut(&key) {
                    *existing = entry;
                }
            }
            None => {
                let position = owner.position;
                let position_v3 = glam::Vec3::from_array(position);

                let (type_id, layer_id, interactor_id, material_id) = {
                    let renderer = get_subsystem!(context, Renderer);
                    let scene = renderer.scene_mut();

                    let material_id = scene.insert_material(build_material(component));
                    let type_id =
                        scene.add_foliage_type(build_type_descriptor(component, material_id));
                    let layer_id =
                        scene.add_foliage_layer(build_layer(component, type_id, position));
                    let interactor_position = if component.interaction.interactor_enabled {
                        position_v3
                    } else {
                        glam::Vec3::from_array(INTERACTOR_PARKED)
                    };
                    let interactor_id = scene.add_foliage_interactor(FoliageInteractor {
                        position: interactor_position,
                        radius: component.interaction.interactor_radius,
                        velocity: glam::Vec3::ZERO,
                    });
                    if component.wind.wind_enabled {
                        apply_wind(scene, component);
                    }
                    (type_id, layer_id, interactor_id, material_id)
                };

                let entry = FoliageEntry {
                    type_id,
                    layer_id,
                    interactor_id,
                    material_id,
                    material_hash,
                    descriptor_hash,
                    bounds_hash,
                    wind_hash,
                };
                let cache = get_subsystem!(context, FoliageCache);
                cache.map.insert(key, entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_behavior_has_the_reflected_component_name() {
        assert_eq!(
            <FoliageComponent as ComponentRuntimeBehavior>::CLASS_NAME,
            "FoliageComponent"
        );
    }

    #[test]
    fn default_descriptor_places_grass_on_flat_ground() {
        let component = FoliageComponent::default();
        let descriptor = build_type_descriptor(&component, MaterialId::from_raw(0, 0));
        // The default slope band is in degrees here; the GPU conversion in helio
        // accepts radians, so verify our component emits a sane radian band that
        // still accepts a flat plane (helio converts `[min,max]` angles to a
        // `[max_cos, min_cos]` band internally).
        assert_eq!(descriptor.slope_range[0], 0.0);
        assert!(descriptor.slope_range[1] > 0.0);
    }
}
