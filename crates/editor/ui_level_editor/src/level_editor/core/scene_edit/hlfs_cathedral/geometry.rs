use super::architectural_mesh::Mesh;
use glam::Vec3;
#[allow(clippy::type_complexity)]
pub type MatProps = ([f32; 4], f32, f32, [f32; 3], f32);
pub fn build() -> (Vec<Mesh>, Vec<Mesh>, [MatProps; 8], [[f32; 3]; 6]) {
    // Limestone, mouldings, basalt, marble, oak, bronze, wax, luminous flame.
    let properties = [
        ([0.53, 0.47, 0.37, 1.], 0.88, 0., [0., 0., 0.], 0.),
        ([0.72, 0.65, 0.51, 1.], 0.72, 0., [0., 0., 0.], 0.),
        ([0.07, 0.085, 0.095, 1.], 0.32, 0., [0., 0., 0.], 0.),
        ([0.51, 0.49, 0.42, 1.], 0.3, 0., [0., 0., 0.], 0.),
        ([0.14, 0.062, 0.027, 1.], 0.53, 0., [0., 0., 0.], 0.),
        ([0.42, 0.25, 0.075, 1.], 0.24, 0.8, [0., 0., 0.], 0.),
        ([0.86, 0.74, 0.50, 1.], 0.65, 0., [0., 0., 0.], 0.),
        ([1., 0.6, 0.15, 1.], 0.5, 0., [1., 0.36, 0.045], 5.),
    ];
    let mut meshes: Vec<Mesh> = (0..properties.len()).map(|_| Mesh::default()).collect();
    // Stone paving, fine mortar gaps and a dark processional border.
    meshes[0].block([0., -0.13, 0.], [11., 0.1, 28.]);
    for x in -9_i32..9 {
        for z in -23_i32..23 {
            let mat = if x.abs() <= 1 {
                if (x + z).rem_euclid(2) == 0 {
                    2
                } else {
                    3
                }
            } else {
                if (x * 13 + z * 7).rem_euclid(11) == 0 {
                    0
                } else {
                    3
                }
            };
            meshes[mat].block(
                [x as f32 * 1.2 + 0.6, -0.015, z as f32 * 1.2 + 0.6],
                [0.592, 0.015, 0.592],
            );
        }
    }
    for x in [-1.9, 1.9] {
        meshes[2].block([x, 0.015, 0.], [0.08, 0.015, 28.]);
    }
    // Side walls are built around actual window openings, not solid boxes
    // hidden behind an emissive rectangle.
    for side in [-1., 1.] {
        let x = side * 11.;
        meshes[0].block([x, 2.25, 0.], [0.3, 2.25, 28.]);
        // Staggered ashlar courses with recessed mortar; the joints are real
        // geometry and therefore participate in both raster and RT visibility.
        for course in 0..8 {
            for stone in 0..28 {
                let z = -27.0 + stone as f32 * 2.0 + (course % 2) as f32 * 0.4;
                let material = if (stone * 7 + course * 3) % 9 == 0 {
                    0
                } else {
                    1
                };
                meshes[material].block(
                    [x - side * 0.30, 0.28 + course as f32 * 0.55, z],
                    [0.025, 0.266, 0.988],
                );
            }
        }
        meshes[0].block([x, 12., 0.], [0.3, 1., 28.]);
        for z in [-26., -18., -10., -2., 6., 14., 22., 28.] {
            meshes[0].block([x, 7.75, z], [0.3, 3.25, 2.75]);
        }
        for y in [0.3, 1.2, 4.5, 11., 12.8] {
            meshes[1].block([x - side * 0.3, y, 0.], [0.18, 0.09, 28.]);
        }
        // Nave clerestory and its continuous string courses.
        meshes[0].block([side * 5.65, 15.3, 0.], [0.22, 2.25, 28.]);
        for y in [11.8, 13., 17.5] {
            meshes[1].block([side * 5.4, y, 0.], [0.23, 0.12, 28.]);
        }
        meshes[0].block([side * 8.25, 12.8, 0.], [2.55, 0.15, 28.]);
        for &z in super::COLUMN_Z {
            let center = Vec3::new(side * 5.5, 0., z);
            meshes[1].smooth_rod(center, center + Vec3::Y * 13., 0.51, 48);
            for i in 0..8 {
                let t = i as f32 * std::f32::consts::TAU / 8.;
                let p = center + Vec3::new(t.cos() * 0.48, 0., t.sin() * 0.48);
                meshes[1].smooth_rod(p + Vec3::Y * 0.65, p + Vec3::Y * 12.1, 0.14, 24);
            }
            for (y, r, h) in [
                (0.15, 0.85, 0.15),
                (0.43, 0.7, 0.11),
                (11.85, 0.68, 0.12),
                (12.15, 0.85, 0.17),
            ] {
                meshes[1].smooth_rod(
                    center + Vec3::Y * (y - h),
                    center + Vec3::Y * (y + h),
                    r,
                    32,
                );
            }
            // Transverse vault ribs and wall pilasters.
            if side < 0. {
                meshes[1].smooth_arch(Vec3::new(-5.5, 12.4, z), Vec3::new(5.5, 12.4, z), 8.1, 0.19);
            }
            meshes[1].smooth_arch(
                Vec3::new(side * 5.5, 8.5, z),
                Vec3::new(side * 10.7, 8.5, z),
                3.8,
                0.12,
            );
            meshes[1].block([side * 10.55, 6., z], [0.2, 6., 0.28]);
        }
        for pair in super::COLUMN_Z.windows(2) {
            meshes[1].smooth_arch(
                Vec3::new(side * 5.5, 8., pair[0]),
                Vec3::new(side * 5.5, 8., pair[1]),
                4.7,
                0.22,
            );
            // Crossed diagonal ribs make the bays read as ribbed vaults.
            meshes[1].smooth_arch(
                Vec3::new(side * 5.5, 12.4, pair[0]),
                Vec3::new(-side * 5.5, 12.4, pair[1]),
                8.1,
                0.12,
            );
        }
    }
    // Curved ceiling shell, underside facing inward.
    for i in 0..40 {
        let x0 = -5.65 + 11.3 * i as f32 / 40.;
        let x1 = -5.65 + 11.3 * (i + 1) as f32 / 40.;
        let height = |x: f32| 17.6 + 3.2 * (1. - (x / 5.65).abs().powf(1.6));
        meshes[0].quad(
            Vec3::new(x0, height(x0), -28.),
            Vec3::new(x1, height(x1), -28.),
            Vec3::new(x1, height(x1), 28.),
            Vec3::new(x0, height(x0), 28.),
        );
    }
    meshes[1].rod(
        Vec3::new(0., 20.45, -28.),
        Vec3::new(0., 20.45, 28.),
        0.16,
        12,
    );
    // East end: raised sanctuary, altar, reredos and gilded cross.
    for i in 0..3 {
        meshes[3].block(
            [0., 0.12 + i as f32 * 0.18, -25.1 - i as f32 * 0.3],
            [5.2 - i as f32 * 0.35, 0.12, 2.7 - i as f32 * 0.3],
        );
    }
    meshes[3].block([0., 1.65, -25.6], [2.4, 0.17, 0.9]);
    for x in [-1.8, 1.8] {
        meshes[1].block([x, 1.05, -25.6], [0.28, 0.6, 0.6]);
    }
    meshes[5].block([0., 3.8, -26.5], [0.105, 2., 0.09]);
    meshes[5].block([0., 4.7, -26.5], [1., 0.105, 0.09]);
    for x in [-4., -3., -2., 2., 3., 4.] {
        meshes[1].rod(Vec3::new(x, 0.6, -27.3), Vec3::new(x, 6., -27.3), 0.12, 12);
        meshes[1].smooth_arch(
            Vec3::new(x - 0.42, 4.8, -27.3),
            Vec3::new(x + 0.42, 4.8, -27.3),
            1.5,
            0.09,
        );
    }
    // Both end walls leave a square opening for the rose; stone spandrels
    // below fill the corners outside its circular aperture.
    for z in [-28., 28.] {
        meshes[0].block([0., 4.2, z], [11., 4.2, 0.3]);
        meshes[0].block([0., 19.1, z], [11., 1.9, 0.3]);
        for x in [-7.8, 7.8] {
            meshes[0].block([x, 12.8, z], [3.2, 4.4, 0.3]);
        }
    }
    // Oak pews with separate seats, backs, feet, end panels and kneelers.
    for side in [-1., 1.] {
        for row in 0..9 {
            let x = side * 3.55;
            let z = -19. + row as f32 * 3.25;
            meshes[4].block([x, 0.65, z], [1.55, 0.08, 0.33]);
            meshes[4].block([x, 1.15, z + 0.30], [1.55, 0.43, 0.07]);
            meshes[4].block([x, 1.6, z + 0.30], [1.6, 0.05, 0.10]);
            meshes[4].block([x, 0.22, z - 0.55], [1.45, 0.08, 0.17]);
            for dx in [-1.48, 1.48] {
                meshes[4].block([x + dx, 0.70, z], [0.08, 0.70, 0.40]);
                meshes[1].rod(
                    Vec3::new(x + dx, 1.4, z + 0.15),
                    Vec3::new(x + dx, 1.55, z + 0.15),
                    0.11,
                    10,
                );
            }
            for dx in [-0.95, -0.32, 0.32, 0.95] {
                meshes[4].block([x + dx, 1.14, z + 0.21], [0.025, 0.32, 0.025]);
            }
        }
    }
    // Bronze chandeliers, suspension rods, real rings and individual candles.
    for &z in super::CHANDELIER_Z {
        let center = Vec3::new(0., 15.2, z);
        meshes[5].rod(center, Vec3::new(0., 20.4, z), 0.035, 8);
        for r in [1.25, 0.9] {
            meshes[5].ring(center, Vec3::X, Vec3::Z, r, 0.045);
        }
        for i in 0..12 {
            let t = i as f32 * std::f32::consts::TAU / 12.;
            let p = center + Vec3::new(t.cos() * 1.25, 0., t.sin() * 1.25);
            meshes[5].rod(center + Vec3::Y * 1.2, p, 0.025, 6);
            meshes[6].rod(p, p + Vec3::Y * 0.34, 0.06, 8);
            meshes[7].rod(p + Vec3::Y * 0.34, p + Vec3::Y * 0.44, 0.025, 6);
        }
    }
    for &(x, y, z) in super::CANDLES {
        for j in -1..=1 {
            let p = Vec3::new(x + j as f32 * 0.18, y - 0.3, z);
            meshes[5].rod(p - Vec3::Y * 0.6, p, 0.045, 8);
            meshes[6].rod(p, p + Vec3::Y * 0.27, 0.065, 10);
            meshes[7].rod(p + Vec3::Y * 0.27, p + Vec3::Y * 0.37, 0.028, 6);
        }
    }
    let colours = [
        [0.12, 0.32, 0.85],
        [0.8, 0.12, 0.08],
        [0.1, 0.55, 0.3],
        [0.8, 0.48, 0.08],
        [0.45, 0.12, 0.62],
        [0.14, 0.65, 0.8],
    ];
    let mut panes: Vec<Mesh> = (0..colours.len()).map(|_| Mesh::default()).collect();
    // Leaded lancets in each side bay, with diamond glazing and stone surrounds.
    for side in [-1., 1.] {
        for (bay, z) in [-22., -14., -6., 2., 10., 18., 25.].into_iter().enumerate() {
            let x = side * 10.96;
            for dz in [-1.25, 0., 1.25] {
                meshes[1].rod(
                    Vec3::new(x, 4.6, z + dz),
                    Vec3::new(x, 10.6, z + dz),
                    0.065,
                    8,
                );
            }
            for y in [4.6, 6.1, 7.6, 9.1, 10.6] {
                meshes[5].rod(
                    Vec3::new(x, y, z - 1.25),
                    Vec3::new(x, y, z + 1.25),
                    0.025,
                    6,
                );
            }
            for column in 0..4 {
                for row in 0..8 {
                    let z0 = z - 1.25 + column as f32 * 0.625;
                    let y = 4.6 + row as f32 * 0.75;
                    let a = Vec3::new(x, y, z0);
                    let b = Vec3::new(x, y + 0.75, z0);
                    let c = Vec3::new(x, y + 0.75, z0 + 0.625);
                    let d = Vec3::new(x, y, z0 + 0.625);
                    panes[(bay + row + column) % colours.len()].quad(a, b, c, d);
                    meshes[5].rod(a, c, 0.012, 5);
                }
            }
            meshes[1].smooth_arch(
                Vec3::new(x, 9.7, z - 1.4),
                Vec3::new(x, 9.7, z + 1.4),
                1.5,
                0.12,
            );
        }
    }
    // Radial rose windows: nested rings, twelve petals and coloured segments.
    for z in [-27.65, 27.65] {
        let center = Vec3::new(0., 12.8, z);
        for r in [0.7, 2.0, 4.2] {
            meshes[1].ring(
                center,
                Vec3::X,
                Vec3::Y,
                r,
                if r > 4. { 0.20 } else { 0.09 },
            );
        }
        for petal in 0..12 {
            let angle = (petal as f32 + 0.5) * std::f32::consts::TAU / 12.;
            let p = center + Vec3::new(angle.cos(), angle.sin(), 0.) * 3.05;
            meshes[1].ring(p, Vec3::X, Vec3::Y, 0.74, 0.045);
            meshes[5].ring(p, Vec3::X, Vec3::Y, 0.38, 0.018);
        }
        for i in 0..48 {
            let a = i as f32 * std::f32::consts::TAU / 48.;
            let b = (i + 1) as f32 * std::f32::consts::TAU / 48.;
            let u = Vec3::new(a.cos(), a.sin(), 0.);
            let v = Vec3::new(b.cos(), b.sin(), 0.);
            for (j, (r0, r1)) in [(0., 0.7), (0.7, 2.), (2., 4.2)].into_iter().enumerate() {
                if r0 == 0. {
                    panes[(i / 4 + j) % 6].triangle(center, center + u * r1, center + v * r1);
                } else {
                    panes[(i / 4 + j) % 6].quad(
                        center + u * r0,
                        center + u * r1,
                        center + v * r1,
                        center + v * r0,
                    );
                }
            }
            meshes[5].rod(center + u * 2.0, center + u * 4.2, 0.016, 6);
            if i % 4 == 0 {
                meshes[1].rod(center + u * 0.7, center + u * 4.2, 0.075, 8);
            }
            let edge_u = u * (4.6 / u.x.abs().max(u.y.abs()));
            let edge_v = v * (4.6 / v.x.abs().max(v.y.abs()));
            // Double-sided masonry so both end-wall orientations are valid.
            meshes[0].quad(
                center + u * 4.2,
                center + edge_u,
                center + edge_v,
                center + v * 4.2,
            );
            meshes[0].quad(
                center + v * 4.2,
                center + edge_v,
                center + edge_u,
                center + u * 4.2,
            );
        }
    }
    (meshes, panes, properties, colours)
}
