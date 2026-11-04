//! Maps of the default planet (Earth's stack): the whole planet
//! (equirectangular) and regional views, shaded relief over the ground's
//! materials, as PNGs.
//!
//! cargo run -p helio-voxel-world --release --example planet_map -- OUT_DIR [lat,lon,km ...]
//!
//! `global.png` is the planet (sea by depth, land by surface material,
//! lit from the north-west); each `lat,lon,km` (degrees, degrees, the
//! view's width; default the planet's top, where voxel_flight starts, 400 km
//! across; `river,km` centres it on the largest river's valley) writes
//! `region_<n>.png` (512 x 512). River channels are drawn
//! blue. `climate.png` is the planet's climate at its ground: moisture from
//! brown (dry) to green (wet), cold toward white. `erosion.png` is what the
//! drainage wore: grey relief, red where it cut the terrain down (bright:
//! 300 m or more), yellow where sediment buries the ground (bright: 100 m).
//!
//! `HELIO_PLANET_MAP_LAYERS=<json>` maps another layer stack (Earth's
//! settings JSON, e.g. with another climate) at seed 7.
use glam::DVec3;
use helio_voxel_world::grid::face_of;
use helio_voxel_world::terrain::{self, material, Column};
use helio_voxel_world::{Planet, PlanetRecipe};
use std::path::Path;

/// Height (mm), surface material and its slope-free colour at direction `d`
/// sampled at `level`.
fn sample(planet: &Planet, d: DVec3, level: u32) -> (i32, u32) {
    let (h, m, _) = sample_at(planet, d, level);
    (h, m)
}

/// [`sample`] and the ground's domain point.
fn sample_at(planet: &Planet, d: DVec3, level: u32) -> (i32, u32, glam::IVec3) {
    let g = planet.grid();
    let face = face_of(d);
    let c = g.face_coords(face, d * g.radius()).expect("face");
    let last = g.cells() - 1;
    let (i, j) = (
        (c[0] as i32).clamp(0, last) >> level,
        (c[1] as i32).clamp(0, last) >> level,
    );
    let h = planet.column_height(face, i, j, level);
    let column = Column { face, i, j, level };
    let surface = planet.field().surface(column, h) & 0xff;
    let top = terrain::top_cells(g, h, level);
    let p = g.volume_point(face, i, j, top - 1, level);
    let m = planet
        .field()
        .ground_material(p, surface, h, 0, 0, (top - 1) << level)
        & material::ID;
    // River channels show as water on the map.
    let m = if surface == helio_voxel_world::landform::RIVER_BED {
        material::WATER
    } else {
        m
    };
    (h, m, p)
}

fn colour(h: i32, m: u32) -> [f64; 3] {
    if m == material::WATER {
        return [40.0, 90.0, 170.0];
    }
    // The sea (land a few metres under the datum stays land: river mouths
    // and the coast's lowlands).
    if h < -3_000 {
        let t = (1.0 + f64::from(h) / 4_000_000.0).clamp(0.0, 1.0);
        return [20.0 + 40.0 * t, 50.0 + 80.0 * t, 110.0 + 90.0 * t];
    }
    match m {
        material::GRASS => [86.0, 128.0, 56.0],
        material::DIRT => [128.0, 100.0, 70.0],
        material::SAND => [214.0, 194.0, 140.0],
        material::SNOW => [242.0, 244.0, 248.0],
        material::GRAVEL => [150.0, 146.0, 138.0],
        material::SANDSTONE | material::CLAY => [190.0, 120.0, 80.0],
        material::STONE | material::DARK_STONE => [120.0, 120.0, 122.0],
        _ => [140.0, 100.0, 70.0],
    }
}

/// Shaded colours of a `w` x `h` grid of directions.
fn render(
    planet: &Planet,
    w: usize,
    h: usize,
    level: u32,
    pixel_m: f64,
    at: &dyn Fn(usize, usize) -> DVec3,
) -> Vec<u8> {
    let samples: Vec<(i32, u32)> = (0..w * h)
        .map(|n| sample(planet, at(n % w, n / w), level))
        .collect();
    let height =
        |x: usize, y: usize| f64::from(samples[y.min(h - 1) * w + x.min(w - 1)].0.max(0)) / 1000.0;
    let mut img = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let (hh, m) = samples[y * w + x];
            // Lit from the north-west, slopes exaggerated a little.
            let dx = (height(x + 1, y) - height(x.saturating_sub(1), y)) / (2.0 * pixel_m);
            let dy = (height(x, y + 1) - height(x, y.saturating_sub(1))) / (2.0 * pixel_m);
            let n = DVec3::new(-dx * 3.0, -dy * 3.0, 1.0).normalize();
            let shade = if hh < -3_000 {
                1.0
            } else {
                (0.35 + 0.75 * n.dot(DVec3::new(-0.6, -0.6, 0.55).normalize())).clamp(0.2, 1.3)
            };
            let c = colour(hh, m).map(|v| (v * shade).min(255.0) as u8);
            img[(y * w + x) * 3..][..3].copy_from_slice(&c);
        }
    }
    img
}

fn save(path: &Path, img: &[u8], w: usize, h: usize) {
    image::save_buffer(path, img, w as u32, h as u32, image::ColorType::Rgb8).expect("png");
    println!("wrote {}", path.display());
}

/// The coarsest level whose cells are at most `pixel_m` wide.
fn level_for(planet: &Planet, pixel_m: f64) -> u32 {
    (0..planet.grid().levels())
        .rev()
        .find(|&l| planet.grid().level_size(l) <= pixel_m)
        .unwrap_or(0)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().unwrap_or("planet_map".into()));
    std::fs::create_dir_all(&out).expect("output directory");
    let args: Vec<String> = args.collect();
    let started = std::time::Instant::now();
    let stack: helio_voxel_world::layers::TerrainLayers = std::env::var("HELIO_PLANET_MAP_LAYERS")
        .ok()
        .map(|json| serde_json::from_str(&json).expect("layer stack JSON"))
        .unwrap_or_default();
    let planet = Planet::new(PlanetRecipe {
        terrain: stack.source(7),
        ..PlanetRecipe::default()
    })
    .unwrap();
    println!("planet in {:.2} s", started.elapsed().as_secs_f64());
    let radius = planet.grid().radius();
    // The default world's stack (seed 7): its maps are the planet's (one
    // bake a process).
    let field = stack.field(planet.grid(), 7).unwrap();
    // The widest valley's node, as latitude and longitude.
    let river = field.maps().and_then(|m| {
        m.river_nodes()
            .into_iter()
            .filter(|n| !n.2)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|n| {
                let d = n.0.as_dvec3().normalize();
                [d.y.asin().to_degrees(), d.z.atan2(d.x).to_degrees()]
            })
    });
    let regions: Vec<[f64; 3]> = {
        let r: Vec<[f64; 3]> = args
            .iter()
            .map(|a| {
                let v: Vec<&str> = a.split(',').collect();
                if v[0] == "river" {
                    let [lat, lon] = river.expect("a world with rivers");
                    [lat, lon, v[1].parse().expect("km")]
                } else {
                    let v: Vec<f64> = v
                        .iter()
                        .map(|x| x.trim().parse().expect("lat,lon,km"))
                        .collect();
                    [v[0], v[1], v[2]]
                }
            })
            .collect();
        if r.is_empty() {
            vec![[90.0, 0.0, 400.0]]
        } else {
            r
        }
    };

    let (w, h) = (1024usize, 512usize);
    let pixel = std::f64::consts::TAU * radius / w as f64;
    let started = std::time::Instant::now();
    let img = render(
        &planet,
        w,
        h,
        level_for(&planet, pixel / 4.0),
        pixel,
        &|x, y| {
            let lon = (x as f64 + 0.5) / w as f64 * std::f64::consts::TAU;
            let lat = (0.5 - (y as f64 + 0.5) / h as f64) * std::f64::consts::PI;
            DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin())
        },
    );
    println!("global map in {:.2} s", started.elapsed().as_secs_f64());
    save(&out.join("global.png"), &img, w, h);

    // The climate at the ground.
    let level = level_for(&planet, pixel / 4.0);
    let mut img = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let lon = (x as f64 + 0.5) / w as f64 * std::f64::consts::TAU;
            let lat = (0.5 - (y as f64 + 0.5) / h as f64) * std::f64::consts::PI;
            let d = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let (height, _, p) = sample_at(&planet, d, level);
            let (t, m) = planet.field().climate(p, height.max(0));
            let wet = f64::from(m) / 65_535.0;
            let mut c = [190.0 - 140.0 * wet, 150.0 + 20.0 * wet, 90.0 - 40.0 * wet];
            let cold = ((5.0 - f64::from(t) / 128.0) / 25.0).clamp(0.0, 1.0);
            for v in &mut c {
                *v += (245.0 - *v) * cold;
            }
            if height < -3_000 {
                c = [30.0, 60.0, 120.0];
            }
            img[(y * w + x) * 3..][..3].copy_from_slice(&c.map(|v| v as u8));
        }
    }
    save(&out.join("climate.png"), &img, w, h);

    // The worn terrain: cuts and sediment.
    for y in 0..h {
        for x in 0..w {
            let lon = (x as f64 + 0.5) / w as f64 * std::f64::consts::TAU;
            let lat = (0.5 - (y as f64 + 0.5) / h as f64) * std::f64::consts::PI;
            let d = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let (height, _, p) = sample_at(&planet, d, level);
            let (cut, fill) = planet.field().erosion(p);
            let grey = (90.0 + f64::from(height) / 40_000.0).clamp(40.0, 200.0);
            let mut c = [grey; 3];
            let worn = (f64::from(-cut) / 300_000.0).clamp(0.0, 1.0);
            let buried = if fill > i32::MIN / 2 && fill >= height - 1_000 {
                (f64::from(fill.max(-1_000_000)) - f64::from(height - 100_000))
                    .clamp(0.0, 100_000.0)
                    / 100_000.0
            } else {
                0.0
            };
            c = [
                c[0] + (230.0 - c[0]) * worn.max(buried),
                c[1] * (1.0 - worn) + (210.0 - c[1]) * buried,
                c[2] * (1.0 - worn.max(buried)),
            ];
            if height < -3_000 {
                c = [30.0, 60.0, 120.0];
            }
            img[(y * w + x) * 3..][..3].copy_from_slice(&c.map(|v| v.clamp(0.0, 255.0) as u8));
        }
    }
    save(&out.join("erosion.png"), &img, w, h);
    if let Some(maps) = field.maps() {
        println!("maps: {:?}", maps.stats);
        // The rivers nearest the planet's top (voxel_flight's spawn): their
        // nodes' directions and floodplains, widest first among the ten
        // nearest wide ones.
        let mut near: Vec<(f64, DVec3, f64)> = maps
            .river_nodes()
            .into_iter()
            .filter(|n| !n.2 && n.1 >= 500.0)
            .map(|n| {
                let d = n.0.as_dvec3().normalize();
                (d.angle_between(DVec3::Y) * radius / 1000.0, d, n.1)
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (km, d, f) in near.iter().take(10) {
            println!(
                "river {km:.0} km from the top: direction {:.6},{:.6},{:.6}, floodplain {f:.0} m",
                d.x, d.y, d.z
            );
        }
    }
    // Moisture over land, in tenths.
    let mut tenths = [0usize; 10];
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let lon = (x as f64 + 0.5) / w as f64 * std::f64::consts::TAU;
            let lat = (0.5 - (y as f64 + 0.5) / h as f64) * std::f64::consts::PI;
            let d = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let (height, _, p) = sample_at(&planet, d, level);
            if height > 0 {
                let m = planet.field().climate(p, height).1;
                tenths[((m * 10 / 65_536) as usize).min(9)] += 1;
            }
        }
    }
    println!("land moisture by tenths: {tenths:?}");

    for (n, [lat, lon, km]) in regions.iter().enumerate() {
        let (lat, lon) = (lat.to_radians(), lon.to_radians());
        let centre = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
        let east = DVec3::Y.cross(centre).normalize_or(DVec3::X);
        let north = centre.cross(east);
        let side = 512usize;
        let pixel = km * 1000.0 / side as f64;
        let img = render(
            &planet,
            side,
            side,
            level_for(&planet, pixel / 2.0),
            pixel,
            &|x, y| {
                let u = (x as f64 + 0.5) / side as f64 - 0.5;
                let v = 0.5 - (y as f64 + 0.5) / side as f64;
                (centre * radius + (east * u + north * v) * km * 1000.0).normalize()
            },
        );
        save(&out.join(format!("region_{n}.png")), &img, side, side);
    }
}
