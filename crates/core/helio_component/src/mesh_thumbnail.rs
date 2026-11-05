//! Asset-browser thumbnails for native `.mesh` assets.
//!
//! A small CPU rasteriser (z-buffered, one directional light) over the mesh's
//! own triangles. Each section is tinted with the base colour of the material
//! its slot resolves to — the mesh's default material when one is assigned —
//! so a thumbnail shows what the mesh will look like placed. The file's
//! modification time is part of the thumbnail cache key, so assigning a new
//! default material (which rewrites the `.mesh`) refreshes it.

use std::path::Path;

use glam::{Vec2, Vec3};

use crate::components::{StaticMeshMaterialSlot, resolve_slot_material};

const SIZE: u32 = 128;
const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

pulsar_reflection::inventory::submit! {
    ui_common::asset_thumbnails::ThumbnailRendererRegistration {
        extension: "mesh",
        render: render_mesh_thumbnail,
    }
}

fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x)
}

/// Linear scalar to an 8-bit sRGB-ish channel (gamma 2.2).
fn encode(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8
}

pub fn render_mesh_thumbnail(path: &Path) -> Option<image::RgbaImage> {
    let asset = crate::subsystems::load_mesh_asset_upload(path)?;
    let vertices = &asset.geometry.vertices;
    if vertices.is_empty() || asset.geometry.indices.is_empty() {
        return None;
    }

    // Three-quarter view, looking at the mesh's bounds centre.
    let (yaw, pitch) = (0.6f32, 0.45f32);
    let to_camera = Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    let right = (-to_camera).cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(-to_camera).normalize_or_zero();

    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in vertices {
        let p = Vec3::from_array(v.position);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let center = (lo + hi) * 0.5;

    let projected: Vec<(Vec2, f32, Vec3)> = vertices
        .iter()
        .map(|v| {
            let world = Vec3::from_array(v.position);
            let rel = world - center;
            (
                Vec2::new(rel.dot(right), rel.dot(up)),
                rel.dot(to_camera),
                world,
            )
        })
        .collect();
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (point, _, _) in &projected {
        min = min.min(*point);
        max = max.max(*point);
    }
    let extent = (max - min).max_element().max(1e-6);
    let scale = SIZE as f32 * 0.86 / extent;
    let middle = (min + max) * 0.5;
    let to_screen = |p: Vec2| {
        Vec2::new(
            (p.x - middle.x) * scale + SIZE as f32 * 0.5,
            SIZE as f32 * 0.5 - (p.y - middle.y) * scale,
        )
    };

    let section_color = |slot: usize| -> [f32; 3] {
        let Some(meta) = asset.material_slots.get(slot) else {
            return [0.7, 0.7, 0.7];
        };
        let slot = StaticMeshMaterialSlot {
            source_material: meta.source_material,
            name: meta.name.clone(),
            imported_surface: meta.surface,
            mesh_default_material: meta.material_asset.clone(),
            ..Default::default()
        };
        let color = resolve_slot_material(Some(&slot), None).row.base_color;
        [color[0], color[1], color[2]]
    };

    let mut image = image::RgbaImage::from_pixel(SIZE, SIZE, image::Rgba(BACKGROUND));
    let mut depth = vec![f32::NEG_INFINITY; (SIZE * SIZE) as usize];
    let light = Vec3::new(-0.35, 0.8, 0.55).normalize();

    for section in &asset.sections {
        let color = section_color(section.material_slot as usize);
        let start = section.first_index as usize;
        let end = (start + section.index_count as usize).min(asset.geometry.indices.len());
        for triangle in asset.geometry.indices[start..end].chunks_exact(3) {
            let (Some(a), Some(b), Some(c)) = (
                projected.get(triangle[0] as usize),
                projected.get(triangle[1] as usize),
                projected.get(triangle[2] as usize),
            ) else {
                continue;
            };
            let (sa, sb, sc) = (to_screen(a.0), to_screen(b.0), to_screen(c.0));
            let area = edge(sa, sb, sc);
            if area.abs() < 1e-4 {
                continue;
            }
            let normal = (b.2 - a.2).cross(c.2 - a.2).normalize_or_zero();
            let shade = 0.34 + 0.66 * normal.dot(light).abs();
            let min_x = sa.x.min(sb.x).min(sc.x).floor().max(0.0) as u32;
            let max_x = sa.x.max(sb.x).max(sc.x).ceil().min(SIZE as f32 - 1.0) as u32;
            let min_y = sa.y.min(sb.y).min(sc.y).floor().max(0.0) as u32;
            let max_y = sa.y.max(sb.y).max(sc.y).ceil().min(SIZE as f32 - 1.0) as u32;
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let (wa, wb, wc) = (
                        edge(sb, sc, p) / area,
                        edge(sc, sa, p) / area,
                        edge(sa, sb, p) / area,
                    );
                    if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                        continue;
                    }
                    let z = wa * a.1 + wb * b.1 + wc * c.1;
                    let index = (y * SIZE + x) as usize;
                    if z > depth[index] {
                        depth[index] = z;
                        image.put_pixel(
                            x,
                            y,
                            image::Rgba([
                                encode(color[0] * shade),
                                encode(color[1] * shade),
                                encode(color[2] * shade),
                                255,
                            ]),
                        );
                    }
                }
            }
        }
    }
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_cache::{
        ImportedSurfaceMaterial, MeshAssetUpload, MeshMaterialSlot, MeshSection, encode_asset,
    };
    use helio_mesh_api::{MeshUpload, PackedVertex};

    #[test]
    fn a_mesh_thumbnail_shows_the_section_colour() {
        let vertex = |x, y| {
            PackedVertex::from_components(
                [x, y, 0.0],
                [0.0, 0.0, 1.0],
                [0.0; 2],
                [1.0, 0.0, 0.0],
                1.0,
            )
        };
        let asset = MeshAssetUpload {
            geometry: MeshUpload {
                vertices: vec![vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(0.0, 1.0)],
                indices: vec![0, 1, 2],
            },
            sections: vec![MeshSection {
                first_index: 0,
                index_count: 3,
                material_slot: 0,
            }],
            material_slots: vec![MeshMaterialSlot {
                surface: ImportedSurfaceMaterial {
                    base_color: [1.0, 0.0, 0.0, 1.0],
                    ..Default::default()
                },
                ..Default::default()
            }],
            bounds: crate::mesh_cache::MeshLocalBounds::from_geometry(&MeshUpload {
                vertices: vec![vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(0.0, 1.0)],
                indices: vec![0, 1, 2],
            })
            .unwrap(),
        };
        let dir = std::env::temp_dir().join(format!("mesh-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.mesh");
        std::fs::write(&path, encode_asset(&asset, 1)).unwrap();
        let image = render_mesh_thumbnail(&path).expect("renders");
        let centre = image.get_pixel(SIZE / 2, SIZE / 2).0;
        assert!(
            centre[0] > 60 && centre[1] == 0 && centre[2] == 0,
            "{centre:?}"
        );
        assert_eq!(image.get_pixel(0, 0).0, BACKGROUND);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
