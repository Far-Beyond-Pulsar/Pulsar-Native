//! 2D side-scrolling sandbox/mining platformer demo — the `assets/sprites/`
//! pack ships as *sheets* (a hero with idle/run/jump/attack/dead strips, a
//! boar, a bee, a snail, a 16px terrain tileset, hand-composed building,
//! hive, rock and tree tiles, plus a sky backdrop), so every PNG is sliced
//! into individual frames at startup (`slice_sheets`), packed into a single
//! non-uniform shelf-packed atlas texture, and used somewhere in the world,
//! grouped into themed zones (a forest, a lake, a village, a mining camp, a
//! monster den, a second forest, a hive-lit tail) rather than scattered
//! everywhere.
//!
//! Controls:
//! - A/D or Left/Right to move, Space to jump.
//! - Left-click and hold on anything to mine it — it cracks in three stages
//!   before breaking, then joins the hotbar along the top of the screen.
//! - Mouse wheel scrolls the hotbar selection.
//! - Right-click places the selected hotbar item at the cursor (snapped to
//!   the terrain grid). Stacks are tracked internally (no on-screen counts);
//!   a stack's icon disappears once it hits zero.
//!
//! Every placed object — terrain tile, tree, building, hive, monster, the
//! lot — is breakable; breaking a terrain tile actually opens a hole
//! (collision reads the same broken-tile set), so digging is real, not just
//! cosmetic.
//!
//! Rendering is the same GPU cull + radix-sort → batch pipeline as the other
//! sprite demos (`SpriteCullPass` → `SpriteBatchPass`) — the terrain alone is
//! ~5,500 instanced quads, inserted once at startup; only the player, the
//! animated critters/items, the in-progress crack overlay, the hotbar icons,
//! and the parallax sky re-upload their instance bytes after that.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use helio_core::RenderGraph;
use helio_pass_radiance_cascades_2d::{
    RadianceCascades2DPass, RadianceCascadesCompositePass, RadianceCascadesConfig,
};
use helio_pass_sprite_batch::{SpriteBatchPass, SpriteHandle, SpriteInstance};
use helio_pass_sprite_cull::SpriteCullPass;
use image::RgbaImage;

mod common;
mod sprite_scene_input;
use sprite_scene_input::SceneInputAdapter;

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

// ── World layout ─────────────────────────────────────────────────────────

const TILE: f32 = 48.0;
const ZOOM: f32 = 1.5; // camera zoom (1.0 = 1 px per world unit, 2.0 = 2× magnification)
const PLAYER_SCALE: f32 = 2.0; // hero frames drawn at 200% native size
const WORLD_COLS: i32 = 240;
const DIRT_ROWS: i32 = 8;
const STONE_ROWS: i32 = 14;
const POOL_CAPACITY: usize = 7500;

const GRAVITY: f32 = -2400.0;
const MOVE_SPEED: f32 = 260.0;
const JUMP_VEL: f32 = 780.0;

const BREAK_STAGE_DURATION: f32 = 0.22;
const BREAK_TOTAL_STAGES: u32 = 3;
const HOTBAR_SLOT_SPACING: f32 = 56.0;
const HOTBAR_ICON_SIZE: f32 = 40.0;
const HOTBAR_MARGIN_TOP: f32 = 46.0;

// ── Lighting (2D radiance cascades) ─────────────────────────────────────
//
// The occupancy grid the lighting pass reads is a flat world-space grid
// (unlike terrain's own per-column-relative storage), sized generously to
// cover every tile the undulating heightfield can ever place: surface_row's
// three summed sines bound it to roughly ±11 tiles, and terrain goes
// `DIRT_ROWS + STONE_ROWS` (22) tiles deep from there.
const OCC_COLS: u32 = WORLD_COLS as u32;
const OCC_ROWS: u32 = 52;
const OCC_ORIGIN: [f32; 2] = [-TILE * 0.5, -38.0 * TILE];

fn occ_cell(pos: [f32; 2]) -> Option<(u32, u32)> {
    let cf = (pos[0] - OCC_ORIGIN[0]) / TILE;
    let rf = (pos[1] - OCC_ORIGIN[1]) / TILE;
    if cf < 0.0 || rf < 0.0 {
        return None;
    }
    let (c, r) = (cf.floor() as u32, rf.floor() as u32);
    if c < OCC_COLS && r < OCC_ROWS {
        Some((c, r))
    } else {
        None
    }
}

fn occ_index(c: u32, r: u32) -> u32 {
    r * OCC_COLS + c
}

/// Mirrors `helio-pass-radiance-cascades-2d`'s `Emitter` WGSL struct layout
/// exactly (32 bytes) — no Cargo-level type dependency, matching how every
/// other pass pair in this codebase shares a byte layout as a protocol
/// rather than a shared Rust type.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuEmitter {
    pos: [f32; 2],
    radius: f32,
    r: f32,
    g: f32,
    b: f32,
    _pad: f32,
    _pad2: f32, // 8 × f32 = 32 bytes, matching shader layout
}

// Emitters: `cabin` windows emit warm amber light.
const LIGHT_EMITTER_NAMES: &[&str] = &["cabin"];

fn emitter_style(name: &str) -> ([f32; 3], f32) {
    match name {
        _ => ([1.0, 0.70, 0.30], 180.0), // cabin: warm amber window glow
    }
}

// Slice names resolved by `slice_sheets` at startup: `/` keys reference an
// animation group's normalized frames ("boar/walk" → the boar's walk sheet),
// plain names reference hand-sliced tiles or single sprites.
//
// Tree base names — layer suffix _0/_1/_2 is appended by place_tree/scatter_trees.
const TREES: &[&str] = &["tree_green_tall", "tree_green_med"];
// Forest A and B ground cover: four bush variants from Tree-Assets.
const FOREST_CLUTTER: &[&str] = &["bush_a", "bush_b", "bush_c", "bush_d"];
const FOREST_CRITTERS: &[&str] = &["snail/walk", "bee/fly"];
// Monster den: dark silhouette trees + mobs.
const DEN_TREES: &[&str] = &["tree_dark_tall", "tree_dark_med", "tree_red_tall"];
const DEN_MONSTERS: &[&str] = &["boar/walk", "bee/fly", "boar/idle"];
// Village: bushes around the cabin.
const VILLAGE_PROPS: &[&str] = &["bush_a", "bush_b", "bush_c"];
// Market/tail: golden and yellow autumn trees + bushes.
const MARKET_TREES: &[&str] = &[
    "tree_golden_tall",
    "tree_golden_med",
    "tree_yellow_tall",
    "tree_yellow_med",
];
const MARKET_CLUTTER: &[&str] = &["bush_c", "bush_d"];

enum Animated {
    None,
    Critter,
    Item,
}

fn surface_row(col: i32) -> i32 {
    let t = col as f32;
    let h = (t * 0.09).sin() * 3.5 + (t * 0.03).sin() * 6.0 + (t * 0.23).sin() * 1.2;
    h.round() as i32
}

fn surface_top_world_y(col: i32) -> f32 {
    surface_row(col) as f32 * TILE
}

/// Ground height at `col` accounting for mined-out terrain: walks down from
/// the surface through consecutive broken tiles, so digging out the row(s)
/// under your feet actually opens a hole you fall into, rather than just
/// changing what's drawn.
fn ground_y_at(col: i32, broken: &HashSet<(i32, i32)>) -> f32 {
    let mut r = 0i32;
    while r <= DIRT_ROWS + STONE_ROWS && broken.contains(&(col, r)) {
        r += 1;
    }
    surface_top_world_y(col) - r as f32 * TILE
}

fn hash01(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(2654435761);
    x ^= x >> 15;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    x as f32 / u32::MAX as f32
}

fn flip_u(uv: [f32; 4]) -> [f32; 4] {
    [uv[2], uv[1], uv[0], uv[3]]
}

fn world_from_screen(
    mouse: (f64, f64),
    window_size: (u32, u32),
    camera_center: [f32; 2],
) -> [f32; 2] {
    let sx = (mouse.0 as f32 - window_size.0 as f32 * 0.5) / ZOOM;
    let sy = (mouse.1 as f32 - window_size.1 as f32 * 0.5) / ZOOM;
    [camera_center[0] + sx, camera_center[1] - sy]
}

/// Point-in-AABB hit test over every breakable object, picking the
/// topmost (highest-depth) match under the cursor.
fn hit_test(objects: &HashMap<SpriteHandle, Breakable>, p: [f32; 2]) -> Option<SpriteHandle> {
    let mut best: Option<(SpriteHandle, f32)> = None;
    for (&handle, b) in objects.iter() {
        let hw = b.size[0] * 0.5;
        let hh = b.size[1] * 0.5;
        if p[0] >= b.pos[0] - hw
            && p[0] <= b.pos[0] + hw
            && p[1] >= b.pos[1] - hh
            && p[1] <= b.pos[1] + hh
            && best.map(|(_, d)| b.depth > d).unwrap_or(true)
        {
            best = Some((handle, b.depth));
        }
    }
    best.map(|(h, _)| h)
}

fn hotbar_slot_world_pos(
    camera_center: [f32; 2],
    window_size: (u32, u32),
    index: usize,
    total: usize,
) -> [f32; 2] {
    let n = total.max(1) as f32;
    let x_offset = (index as f32 - (n - 1.0) * 0.5) * HOTBAR_SLOT_SPACING / ZOOM;
    let y_offset = window_size.1 as f32 * 0.5 / ZOOM - HOTBAR_MARGIN_TOP;
    [camera_center[0] + x_offset, camera_center[1] + y_offset]
}

/// Small deterministic PRNG (PCG-style LCG) — avoids pulling in `rand`.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn next_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
    fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_f32() * (hi - lo) as f32) as i32
    }
    fn range_usize(&mut self, n: usize) -> usize {
        ((self.next_f32() * n as f32) as usize).min(n - 1)
    }
    fn bool(&mut self) -> bool {
        self.next_f32() < 0.5
    }
}

// ── Sprite atlas: load every PNG in assets/sprites/, shelf-pack them (no
// padding on the individual sprites, native pixel dimensions, non-uniform
// packing — not a grid) into one runtime texture, plus a few procedurally
// generated extras (a solid-white swatch, three crack-overlay stages). ──────

#[derive(Clone, Copy)]
struct PackedSprite {
    uv: [f32; 4],
    w: f32,
    h: f32,
}

// Generated by `build.rs` from every PNG in `assets/sprites/` at compile
// time (`include_bytes!` per file) — the sprite set ships inside the binary,
// not read from disk at runtime.
include!(concat!(env!("OUT_DIR"), "/embedded_sprites.rs"));

fn load_all_sprites() -> Vec<(String, RgbaImage)> {
    EMBEDDED_SPRITES
        .iter()
        .map(|(name, bytes)| {
            let img = image::load_from_memory(bytes)
                .unwrap_or_else(|e| panic!("decode embedded sprite {name}: {e}"))
                .to_rgba8();
            (name.to_string(), img)
        })
        .collect()
}

fn point_seg_dist(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        ((px - x0) * dx + (py - y0) * dy) / len2
    } else {
        0.0
    };
    let t = t.clamp(0.0, 1.0);
    let (cx, cy) = (x0 + t * dx, y0 + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// A procedural crack overlay for mining stage `stage` (1..=3) — more, wider
/// cracks radiating from the center at higher stages. Straight alpha, on a
/// transparent background, stretched over whatever it's overlaid on.
fn make_crack_image(stage: u32) -> RgbaImage {
    const SIZE: u32 = 64;
    let mut img = RgbaImage::new(SIZE, SIZE);
    let mut rng = Rng::new(0x0C7A_CC00 + stage as u64);
    let num_lines = 2 + stage * 2;
    let thickness = 1.6 + stage as f32 * 0.5;
    let (cx, cy) = (SIZE as f32 * 0.5, SIZE as f32 * 0.5);
    for _ in 0..num_lines {
        let ang = rng.next_f32() * std::f32::consts::TAU;
        let len = SIZE as f32 * (0.25 + rng.next_f32() * 0.35);
        let (x1, y1) = (cx + ang.cos() * len, cy + ang.sin() * len);
        for py in 0..SIZE {
            for px in 0..SIZE {
                let d = point_seg_dist(px as f32 + 0.5, py as f32 + 0.5, cx, cy, x1, y1);
                if d < thickness {
                    let a = ((1.0 - d / thickness) * 210.0) as u8;
                    let p = img.get_pixel_mut(px, py);
                    if a > p[3] {
                        *p = image::Rgba([15, 12, 10, a]);
                    }
                }
            }
        }
    }
    img
}

/// Shelf-packs `images` into one atlas texture at their native sizes — no
/// padding on the sprites themselves, just a small gap between packed items
/// to avoid sampling bleed at their shared edges.
fn pack_atlas(images: Vec<(String, RgbaImage)>) -> (RgbaImage, HashMap<String, PackedSprite>) {
    const ATLAS_W: u32 = 1536;
    const GAP: u32 = 2;

    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(images[i].1.height()));

    let mut placements: Vec<(usize, u32, u32)> = Vec::new();
    let mut cursor_y = GAP;
    let mut shelf_x = GAP;
    let mut shelf_h = 0u32;
    for &i in &order {
        let (iw, ih) = images[i].1.dimensions();
        if shelf_x + iw + GAP > ATLAS_W {
            cursor_y += shelf_h + GAP;
            shelf_x = GAP;
            shelf_h = 0;
        }
        placements.push((i, shelf_x, cursor_y));
        shelf_x += iw + GAP;
        shelf_h = shelf_h.max(ih);
    }
    let atlas_h = cursor_y + shelf_h + GAP;

    let mut atlas = RgbaImage::new(ATLAS_W, atlas_h);
    let mut uvs = HashMap::new();
    for (i, x, y) in placements {
        let (name, img) = &images[i];
        image::imageops::overlay(&mut atlas, img, x as i64, y as i64);
        let (iw, ih) = img.dimensions();
        uvs.insert(
            name.clone(),
            PackedSprite {
                uv: [
                    x as f32 / ATLAS_W as f32,
                    y as f32 / atlas_h as f32,
                    (x + iw) as f32 / ATLAS_W as f32,
                    (y + ih) as f32 / atlas_h as f32,
                ],
                w: iw as f32,
                h: ih as f32,
            },
        );
    }
    (atlas, uvs)
}

// ── Sheet slicer ─────────────────────────────────────────────────────────
//
// The new pack ships art as multi-frame *sheets*: a hero with
// idle/run/attack/jump/dead strips, boar/bee/snail mobs, a 16px terrain
// tileset, and hand-composed building/hive/rock/tree tiles. `slice_sheets`
// cuts every sheet into individual sprites *before* `pack_atlas` shelves
// them. Frames in a shared animation `group` are normalized to one common
// box (union of every frame's opaque bounds — centered horizontally,
// feet-aligned to the bottom) so the group can cycle frames without jitter
// or ground-sliding, and the player/critters animate by just swapping UVs.

fn crop_rect(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    image::imageops::crop_imm(img, x.min(img.width()), y.min(img.height()), w, h).to_image()
}

/// Returns the tight opaque-content bounds of `img`, or `None` if empty.
fn trim_content(img: &RgbaImage) -> Option<RgbaImage> {
    let (w, h) = img.dimensions();
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 0 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < min_x || max_y < min_y {
        return None;
    }
    Some(crop_rect(
        img,
        min_x,
        min_y,
        max_x - min_x + 1,
        max_y - min_y + 1,
    ))
}

fn avg_color(img: &RgbaImage) -> (u32, u32, u32) {
    let (w, h) = img.dimensions();
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            if p[3] > 0 {
                r += p[0] as u64;
                g += p[1] as u64;
                b += p[2] as u64;
                n += 1;
            }
        }
    }
    if n == 0 {
        return (0, 0, 0);
    }
    ((r / n) as u32, (g / n) as u32, (b / n) as u32)
}
#[derive(Clone, Copy)]
enum SliceSpec {
    /// A regular `cols × rows` grid of `cell_w × cell_h` cells. Empty cells
    /// are dropped; cells are trimmed to their opaque content. With `group`,
    /// all frames across every sheet in the group are re-normalized to one
    /// shared box (see above) and indexed as `{group}_{key}_{i}`.
    Grid {
        cell_w: u32,
        cell_h: u32,
        cols: u32,
        rows: u32,
        group: Option<&'static str>,
        key: &'static str,
    },
    /// Hand-picked sub-rectangles `(x, y, w, h)` in sheet pixels, each its
    /// own sprite, trimmed to content.
    Rects(&'static [(&'static str, (u32, u32, u32, u32))]),
    /// The whole sheet is one sprite.
    Single(&'static str),
    /// Not used by the demo (recolor variants, the raw HUD, `all.png`).
    Skip,
}

/// Cell geometry / crop regions for every sheet in the pack, derived by
/// pixel-gutter analysis of `assets/sprites/` (see the demo's analysis).
fn slice_spec(path: &str) -> SliceSpec {
    use SliceSpec::*;
    match path {
        // ── Hero — normalized to a shared box across every animation. ──────
        "Character/Idle/Idle-Sheet" => Grid {
            cell_w: 64,
            cell_h: 80,
            cols: 4,
            rows: 1,
            group: Some("player"),
            key: "idle",
        },
        "Character/Run/Run-Sheet" => Grid {
            cell_w: 80,
            cell_h: 80,
            cols: 8,
            rows: 1,
            group: Some("player"),
            key: "run",
        },
        "Character/Attack-01/Attack-01-Sheet" => Grid {
            cell_w: 96,
            cell_h: 80,
            cols: 8,
            rows: 1,
            group: Some("player"),
            key: "attack",
        },
        "Character/Jumlp-All/Jump-All-Sheet" => Grid {
            cell_w: 64,
            cell_h: 64,
            cols: 15,
            rows: 1,
            group: Some("player"),
            key: "jump",
        },
        "Character/Jump-Start/Jump-Start-Sheet" => Grid {
            cell_w: 64,
            cell_h: 64,
            cols: 4,
            rows: 1,
            group: Some("player"),
            key: "jump_start",
        },
        "Character/Jump-End/Jump-End-Sheet" => Grid {
            cell_w: 64,
            cell_h: 64,
            cols: 3,
            rows: 1,
            group: Some("player"),
            key: "jump_end",
        },
        "Character/Dead/Dead-Sheet" => Grid {
            cell_w: 80,
            cell_h: 64,
            cols: 8,
            rows: 1,
            group: Some("player"),
            key: "dead",
        },
        // ── Boar. ─────────────────────────────────────────────────────────
        "Mob/Boar/Idle/Idle-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 4,
            rows: 1,
            group: Some("boar"),
            key: "idle",
        },
        "Mob/Boar/Run/Run-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 6,
            rows: 1,
            group: Some("boar"),
            key: "run",
        },
        "Mob/Boar/Walk/Walk-Base-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 6,
            rows: 1,
            group: Some("boar"),
            key: "walk",
        },
        "Mob/Boar/Hit-Vanish/Hit-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 4,
            rows: 1,
            group: Some("boar"),
            key: "hit",
        },
        // ── Bee. ──────────────────────────────────────────────────────────
        "Mob/Small Bee/Attack/Attack-Sheet" => Grid {
            cell_w: 64,
            cell_h: 64,
            cols: 4,
            rows: 1,
            group: Some("bee"),
            key: "attack",
        },
        "Mob/Small Bee/Fly/Fly-Sheet" => Grid {
            cell_w: 64,
            cell_h: 64,
            cols: 4,
            rows: 1,
            group: Some("bee"),
            key: "fly",
        },
        "Mob/Small Bee/Hit/Hit-Sheet" => Grid {
            cell_w: 64,
            cell_h: 64,
            cols: 4,
            rows: 1,
            group: Some("bee"),
            key: "hit",
        },
        // ── Snail. ────────────────────────────────────────────────────────
        "Mob/Snail/walk-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 8,
            rows: 1,
            group: Some("snail"),
            key: "walk",
        },
        "Mob/Snail/Hide-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 8,
            rows: 1,
            group: Some("snail"),
            key: "hide",
        },
        "Mob/Snail/Dead-Sheet" => Grid {
            cell_w: 48,
            cell_h: 32,
            cols: 8,
            rows: 1,
            group: Some("snail"),
            key: "dead",
        },
        // ── 16px terrain tileset (classified by color in `classify_tiles`). ─
        "Assets/Tiles" => Grid {
            cell_w: 16,
            cell_h: 16,
            cols: 25,
            rows: 25,
            group: None,
            key: "",
        },
        // ── Hand-composed tiles. ──────────────────────────────────────────
        // Tree-Assets.png (336×400): right column only — four bush variants.
        // The left cave-dressing cluster is no longer used.
        "Assets/Tree-Assets" => Rects(&[
            ("bush_a", (210, 5, 124, 86)),
            ("bush_b", (210, 101, 124, 86)),
            ("bush_c", (210, 197, 124, 86)),
            ("bush_d", (210, 293, 124, 86)),
        ]),
        // Buildings.png, Hive.png, Interior-01.png, Props-Rocks.png — not used.
        "Assets/Buildings" | "Assets/Hive" | "Assets/Interior-01" | "Assets/Props-Rocks" => Skip,
        // cabin.png (408×201): single standalone cabin with lit windows.
        // Full content fills the sheet — no transparent margins.
        "cabin" => Single("cabin"),
        // Large pine-tree canvases (1344×1200):
        //   Each column (x=0, x=112, x=224) is one depth layer of the same tree.
        //   All three layers must be rendered at the same world XY, composited
        //   back-to-front, to produce one complete tree.
        "Trees/Green-Tree" => Rects(&[
            ("tree_green_tall_0", (0, 0, 107, 368)),
            ("tree_green_tall_1", (112, 0, 107, 368)),
            ("tree_green_tall_2", (224, 0, 107, 368)),
            ("tree_green_med_0", (0, 391, 107, 313)),
            ("tree_green_med_1", (112, 391, 107, 313)),
            ("tree_green_med_2", (224, 391, 107, 313)),
        ]),
        "Trees/Red-Tree" => Rects(&[
            ("tree_red_tall_0", (0, 0, 107, 368)),
            ("tree_red_tall_1", (112, 0, 107, 368)),
            ("tree_red_tall_2", (224, 0, 107, 368)),
            ("tree_red_med_0", (0, 391, 107, 313)),
            ("tree_red_med_1", (112, 391, 107, 313)),
            ("tree_red_med_2", (224, 391, 107, 313)),
        ]),
        "Trees/Dark-Tree" => Rects(&[
            ("tree_dark_tall_0", (0, 0, 107, 368)),
            ("tree_dark_tall_1", (112, 0, 107, 368)),
            ("tree_dark_tall_2", (224, 0, 107, 368)),
            ("tree_dark_med_0", (0, 391, 107, 313)),
            ("tree_dark_med_1", (112, 391, 107, 313)),
            ("tree_dark_med_2", (224, 391, 107, 313)),
        ]),
        "Trees/Golden-Tree" => Rects(&[
            ("tree_golden_tall_0", (0, 0, 107, 368)),
            ("tree_golden_tall_1", (112, 0, 107, 368)),
            ("tree_golden_tall_2", (224, 0, 107, 368)),
            ("tree_golden_med_0", (0, 391, 107, 313)),
            ("tree_golden_med_1", (112, 391, 107, 313)),
            ("tree_golden_med_2", (224, 391, 107, 313)),
        ]),
        "Trees/Yellow-Tree" => Rects(&[
            ("tree_yellow_tall_0", (0, 0, 107, 368)),
            ("tree_yellow_tall_1", (112, 0, 107, 368)),
            ("tree_yellow_tall_2", (224, 0, 107, 368)),
            ("tree_yellow_med_0", (0, 391, 107, 313)),
            ("tree_yellow_med_1", (112, 391, 107, 313)),
            ("tree_yellow_med_2", (224, 391, 107, 313)),
        ]),
        // 896×256 parallax forest silhouette strip — tiled behind the terrain.
        "Trees/Background" => Single("background_trees"),
        "Background/Background" => Single("background"),
        // Recolor variants, the packed `all.png`, and the HUD sheet aren't
        // used by this demo.
        _ => Skip,
    }
}

struct SliceOutput {
    frames: Vec<(String, RgbaImage)>,
    /// `"player/idle" → ["player_idle_0", …]` etc., in sheet order.
    anims: HashMap<String, Vec<String>>,
}

fn slice_sheets(sheets: Vec<(String, RgbaImage)>) -> SliceOutput {
    let mut frames: Vec<(String, RgbaImage)> = Vec::new();
    let mut anims: HashMap<String, Vec<String>> = HashMap::new();
    let mut groups: HashMap<&'static str, Vec<(&'static str, RgbaImage)>> = HashMap::new();

    for (path, img) in sheets {
        match slice_spec(&path) {
            SliceSpec::Skip => {}
            SliceSpec::Single(name) => {
                if let Some(t) = trim_content(&img) {
                    frames.push((name.to_string(), t));
                }
            }
            SliceSpec::Rects(rects) => {
                for &(name, (x, y, w, h)) in rects {
                    let cell = crop_rect(&img, x, y, w, h);
                    if let Some(t) = trim_content(&cell) {
                        frames.push((name.to_string(), t));
                    }
                }
            }
            SliceSpec::Grid {
                cell_w,
                cell_h,
                cols,
                rows,
                group,
                key,
            } => {
                for r in 0..rows {
                    for c in 0..cols {
                        let cell = crop_rect(&img, c * cell_w, r * cell_h, cell_w, cell_h);
                        if let Some(t) = trim_content(&cell) {
                            match group {
                                Some(g) => groups.entry(g).or_default().push((key, t)),
                                None if path == "Assets/Tiles" => {
                                    frames.push((format!("tile_{r}_{c}"), t))
                                }
                                None => frames.push((format!("{path}#{r}_{c}"), t)),
                            }
                        }
                    }
                }
            }
        }
    }

    // Normalize every animation group to a common box.
    for (group, raw) in groups {
        let (box_w, box_h) = raw.iter().fold((0u32, 0u32), |(w, h), (_, im)| {
            (w.max(im.width()), h.max(im.height()))
        });
        let mut order: HashMap<&'static str, Vec<String>> = HashMap::new();
        for (i, (key, im)) in raw.into_iter().enumerate() {
            let name = format!("{group}_{key}_{i}");
            let mut canvas = RgbaImage::from_pixel(box_w, box_h, image::Rgba([0, 0, 0, 0]));
            let x = (box_w - im.width()) / 2;
            let y = box_h - im.height(); // feet aligned to the box bottom
            image::imageops::overlay(&mut canvas, &im, x as i64, y as i64);
            order.entry(key).or_default().push(name.clone());
            frames.push((name, canvas));
        }
        for (key, names) in order {
            anims.insert(format!("{group}/{key}"), names);
        }
    }

    SliceOutput { frames, anims }
}

/// Picks the best terrain tile per class (grass/dirt/stone/water) by
/// nearest-color match and renames those frames to `tile_grass` etc.
fn classify_tiles(frames: &mut Vec<(String, RgbaImage)>) {
    const TARGETS: [(&str, (u32, u32, u32)); 4] = [
        ("tile_grass", (110, 130, 4)),
        ("tile_dirt", (135, 108, 55)),
        ("tile_stone", (112, 124, 132)),
        ("tile_water", (50, 140, 176)),
    ];
    let mut best: [Option<(usize, u64)>; 4] = [None; 4];
    for (i, (name, img)) in frames.iter().enumerate() {
        if !name.starts_with("tile_") || name.contains(' ') {
            continue;
        }
        // Require a reasonably filled tile so the avg color is meaningful.
        let (w, h) = img.dimensions();
        let mut opaque = 0u32;
        for y in 0..h {
            for x in 0..w {
                if img.get_pixel(x, y)[3] > 0 {
                    opaque += 1;
                }
            }
        }
        if opaque < 60 {
            continue;
        }
        let (r, g, b) = avg_color(img);
        for (ti, (_, (tr, tg, tb))) in TARGETS.iter().enumerate() {
            let dr = r as i64 - *tr as i64;
            let dg = g as i64 - *tg as i64;
            let db = b as i64 - *tb as i64;
            let dist = (dr * dr + dg * dg + db * db) as u64;
            if best[ti].map(|(_, d)| dist < d).unwrap_or(true) {
                best[ti] = Some((i, dist));
            }
        }
    }
    for (i, (class, _)) in TARGETS.iter().enumerate() {
        if let Some((fi, _)) = best[i] {
            frames[fi].0 = class.to_string();
        }
    }
}

// ── Breakable world objects ──────────────────────────────────────────────

#[derive(Clone)]
struct Breakable {
    pos: [f32; 2],
    size: [f32; 2],
    depth: f32,
    name: String,
    /// `Some((col, row))` for terrain tiles only — lets breaking one open an
    /// actual hole in the collision heightfield.
    terrain_cell: Option<(i32, i32)>,
}

struct Breaking {
    handle: SpriteHandle,
    target: Breakable,
    start: Instant,
    crack_handle: Option<SpriteHandle>,
    stage: u32,
}

struct HotbarSlot {
    name: String,
    count: u32,
    handle: SpriteHandle,
    uv: [f32; 4],
    w: f32,
    h: f32,
}

fn place_prop(
    scene: &mut SceneInputAdapter,
    atlas: &HashMap<String, PackedSprite>,
    atlas_layer: u32,
    objects: &mut HashMap<SpriteHandle, Breakable>,
    name: &'static str,
    x: f32,
    depth: f32,
    flip: bool,
    y_offset: f32,
) -> PackedSprite {
    let s = atlas[name];
    let col = (x / TILE).round() as i32;
    let top = surface_top_world_y(col);
    let uv = if flip { flip_u(s.uv) } else { s.uv };
    let pos = [x, top + s.h * 0.5 + y_offset];
    let handle = scene.insert_sprite(
        SpriteInstance::new(pos, [s.w, s.h])
            .with_uv_rect(uv)
            .with_depth(depth)
            .with_atlas_layer(atlas_layer),
    );
    objects.insert(
        handle,
        Breakable {
            pos,
            size: [s.w, s.h],
            depth,
            name: name.to_string(),
            terrain_cell: None,
        },
    );
    s
}

/// Lays `names` out left-to-right starting at `start_x`, each spaced by its
/// own real width plus `gap` — used for the mining/market rows so every
/// listed sprite appears exactly once, never overlapping its neighbor.
/// Returns the cursor position after the last item, for chaining rows.
fn lay_row(
    scene: &mut SceneInputAdapter,
    atlas: &HashMap<String, PackedSprite>,
    atlas_layer: u32,
    objects: &mut HashMap<SpriteHandle, Breakable>,
    names: &[&'static str],
    start_x: f32,
    depth: f32,
    gap: f32,
) -> f32 {
    let mut cursor = start_x;
    for &name in names {
        let s = atlas[name];
        cursor += s.w * 0.5;
        place_prop(
            scene,
            atlas,
            atlas_layer,
            objects,
            name,
            cursor,
            depth,
            false,
            0.0,
        );
        cursor += s.w * 0.5 + gap;
    }
    cursor
}

/// Scatters instances of `names` across `[col_start, col_end)`, spaced by a
/// random step in `step`. Used *within* one themed zone (a forest band, a
/// mining band, a monster den) rather than across the whole world, so each
/// category stays visually grouped where it thematically belongs.
#[allow(clippy::too_many_arguments)]
fn scatter_band(
    scene: &mut SceneInputAdapter,
    atlas: &HashMap<String, PackedSprite>,
    atlas_layer: u32,
    objects: &mut HashMap<SpriteHandle, Breakable>,
    rng: &mut Rng,
    critters: &mut Vec<Critter>,
    items: &mut Vec<Item>,
    col_start: i32,
    col_end: i32,
    step: (i32, i32),
    names: &[&'static str],
    animated: Animated,
    depth: f32,
    anims: &HashMap<String, Vec<String>>,
) {
    let mut col = col_start;
    loop {
        col += rng.range_i32(step.0, step.1);
        if col >= col_end {
            break;
        }
        let x = col as f32 * TILE + rng.range_i32(-6, 6) as f32;
        let name = names[rng.range_usize(names.len())];
        let top = surface_top_world_y((x / TILE).round() as i32);
        match animated {
            Animated::None => {
                let s = atlas[name];
                let flip = rng.bool();
                let uv = if flip { flip_u(s.uv) } else { s.uv };
                let pos = [x, top + s.h * 0.5];
                let handle = scene.insert_sprite(
                    SpriteInstance::new(pos, [s.w, s.h])
                        .with_uv_rect(uv)
                        .with_depth(depth)
                        .with_atlas_layer(atlas_layer),
                );
                objects.insert(
                    handle,
                    Breakable {
                        pos,
                        size: [s.w, s.h],
                        depth,
                        name: name.to_string(),
                        terrain_cell: None,
                    },
                );
            }
            Animated::Critter => {
                let frames = &anims[name];
                let s = *atlas
                    .get(&frames[0])
                    .expect("critter frame missing from atlas");
                let base_pos = [x, top + s.h * 0.5];
                let handle = scene.insert_sprite(
                    SpriteInstance::new(base_pos, [s.w, s.h])
                        .with_uv_rect(s.uv)
                        .with_depth(depth)
                        .with_atlas_layer(atlas_layer),
                );
                let name_owned = frames[0].clone();
                objects.insert(
                    handle,
                    Breakable {
                        pos: base_pos,
                        size: [s.w, s.h],
                        depth,
                        name: name_owned,
                        terrain_cell: None,
                    },
                );
                critters.push(Critter {
                    handle,
                    base_pos,
                    phase: rng.next_f32() * std::f32::consts::TAU,
                    frames: frames.clone(),
                    fps: anim_fps(name),
                });
            }
            Animated::Item => {
                let s = atlas[name];
                let base_pos = [x, top + s.h * 0.5 + 10.0];
                let handle = scene.insert_sprite(
                    SpriteInstance::new(base_pos, [s.w, s.h])
                        .with_uv_rect(s.uv)
                        .with_depth(depth)
                        .with_atlas_layer(atlas_layer),
                );
                objects.insert(
                    handle,
                    Breakable {
                        pos: base_pos,
                        size: [s.w, s.h],
                        depth,
                        name: name.to_string(),
                        terrain_cell: None,
                    },
                );
                items.push(Item {
                    handle,
                    base_pos,
                    phase: rng.next_f32() * std::f32::consts::TAU,
                    spin: rng.range_i32(-100, 100) as f32 / 100.0,
                    spr: s,
                });
            }
        }
    }
}

/// Places one complete tree by rendering its three depth layers (suffix _0/_1/_2)
/// at the same world XY position, back-to-front (depth 0.02 apart).
fn place_tree(
    scene: &mut SceneInputAdapter,
    atlas: &HashMap<String, PackedSprite>,
    atlas_layer: u32,
    objects: &mut HashMap<SpriteHandle, Breakable>,
    base: &str,
    x: f32,
    depth: f32,
    flip: bool,
) {
    let col = (x / TILE).round() as i32;
    let top = surface_top_world_y(col);
    for (i, suffix) in ["_0", "_1", "_2"].iter().enumerate() {
        let key = format!("{base}{suffix}");
        let Some(&s) = atlas.get(&key) else { continue };
        let uv = if flip { flip_u(s.uv) } else { s.uv };
        let pos = [x, top + s.h * 0.5];
        let layer_depth = depth + (2 - i) as f32 * 0.01;
        let handle = scene.insert_sprite(
            SpriteInstance::new(pos, [s.w, s.h])
                .with_uv_rect(uv)
                .with_depth(layer_depth)
                .with_atlas_layer(atlas_layer),
        );
        // Only register layer 0 as a breakable so it isn't triple-counted.
        if i == 0 {
            objects.insert(
                handle,
                Breakable {
                    pos,
                    size: [s.w, s.h],
                    depth: layer_depth,
                    name: key,
                    terrain_cell: None,
                },
            );
        }
    }
}

/// Scatters trees from `base_names` across `[col_start, col_end)` using `place_tree`.
#[allow(clippy::too_many_arguments)]
fn scatter_trees(
    scene: &mut SceneInputAdapter,
    atlas: &HashMap<String, PackedSprite>,
    atlas_layer: u32,
    objects: &mut HashMap<SpriteHandle, Breakable>,
    rng: &mut Rng,
    col_start: i32,
    col_end: i32,
    step: (i32, i32),
    base_names: &[&str],
    depth: f32,
) {
    let mut col = col_start;
    loop {
        col += rng.range_i32(step.0, step.1);
        if col >= col_end {
            break;
        }
        let x = col as f32 * TILE + rng.range_i32(-6, 6) as f32;
        let base = base_names[rng.range_usize(base_names.len())];
        let flip = rng.bool();
        place_tree(scene, atlas, atlas_layer, objects, base, x, depth, flip);
    }
}

fn anim_fps(key: &str) -> f32 {
    if key.contains("boar") {
        9.0 // 4/6 frames — weighty trot
    } else if key.contains("bee") {
        16.0 // 4 frames — rapid wing-flap
    } else if key.contains("snail") {
        5.0 // 8 frames — very slow crawl
    } else {
        10.0
    }
}

// ── App scaffolding ──────────────────────────────────────────────────────

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}

struct App {
    state: Option<AppState>,
}
impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

struct Critter {
    handle: SpriteHandle,
    base_pos: [f32; 2],
    phase: f32,
    /// Normalized frames to cycle through (all the same box, feet-aligned).
    frames: Vec<String>,
    fps: f32,
}

struct Item {
    handle: SpriteHandle,
    base_pos: [f32; 2],
    phase: f32,
    spin: f32,
    spr: PackedSprite,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlayerAnim {
    Idle,
    Run,
    Jump,
    Fall,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    graph: RenderGraph,
    scene: SceneInputAdapter,
    dummy_depth_view: wgpu::TextureView,

    atlas: HashMap<String, PackedSprite>,
    atlas_layer: u32,
    crack_uvs: [[f32; 4]; 3],
    /// Animation groups from `slice_sheets`: `"player/idle" → frame keys`.
    anims: HashMap<String, Vec<String>>,

    player_handle: SpriteHandle,
    player_pos: [f32; 2],
    player_vel: [f32; 2],
    player_on_ground: bool,
    player_facing_right: bool,
    /// The normalized player box — every hero frame shares this size, with
    /// feet at the bottom, so collision and animation are stable.
    player_box: [f32; 2],
    player_anim: PlayerAnim,
    player_anim_time: f32,

    bg_handle: SpriteHandle,
    bg_uv: [f32; 4],
    bg_size: [f32; 2],

    camera_center: [f32; 2],
    keys: HashSet<KeyCode>,
    mouse_pos: (f64, f64),

    critters: Vec<Critter>,
    items: Vec<Item>,

    objects: HashMap<SpriteHandle, Breakable>,
    broken_terrain: HashSet<(i32, i32)>,
    breaking: Option<Breaking>,
    hotbar: Vec<HotbarSlot>,
    hotbar_selected: usize,

    occupancy_buf: Arc<wgpu::Buffer>,
    occupancy_words: Vec<u32>,

    start_time: Instant,
    last_frame: Instant,
    fps_frames: u32,
    fps_last_print: Instant,
    window_size: (u32, u32),
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio – Sprite Sandbox/Mining Demo")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("window"),
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            flags: wgpu::InstanceFlags::empty(),
            ..helio_core::wgpu_instance_descriptor()
        });
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .expect("device");
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
                color_space: wgpu::SurfaceColorSpace::Auto,
            },
        );

        let loaded = load_all_sprites();
        log::info!(
            "[sprite_dig_demo] loaded {} embedded sprite files",
            loaded.len()
        );
        let sliced = slice_sheets(loaded);
        let anims = sliced.anims;
        let mut loaded = sliced.frames;
        loaded.push((
            "__white".to_string(),
            RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255])),
        ));
        for stage in 1..=3u32 {
            loaded.push((format!("__crack_{stage}"), make_crack_image(stage)));
        }
        let (atlas_img, atlas) = pack_atlas(loaded);
        log::info!(
            "[sprite_dig_demo] packed atlas: {}x{} ({} sprites)",
            atlas_img.width(),
            atlas_img.height(),
            atlas.len()
        );
        let crack_uvs = [
            atlas["__crack_1"].uv,
            atlas["__crack_2"].uv,
            atlas["__crack_3"].uv,
        ];

        let mut graph = RenderGraph::new(&device, &queue);
        let mut sprite_pass = SpriteBatchPass::new(&device, &queue, format);
        let mut scene = SceneInputAdapter::new(device.clone(), queue.clone());
        sprite_pass.set_clear_color(Some(wgpu::Color {
            r: 0.42,
            g: 0.70,
            b: 0.94,
            a: 1.0,
        }));
        let atlas_layer = sprite_pass.add_atlas_layer(
            &device,
            &queue,
            atlas_img.width(),
            atlas_img.height(),
            atlas_img.as_raw(),
        );

        sprite_pass.reserve(&device, POOL_CAPACITY);
        let mut sprite_cull = SpriteCullPass::new(
            &device,
            &queue,
            sprite_pass.instances_buffer(),
            sprite_pass.alive_buffer(),
            POOL_CAPACITY as u32,
            POOL_CAPACITY as u32,
        );
        sprite_cull.set_view_rect(
            [0.0, 0.0],
            [
                size.width as f32 * 0.5 / ZOOM,
                size.height as f32 * 0.5 / ZOOM,
            ],
        );
        sprite_pass.use_gpu_culling(
            sprite_cull.draw_order_buf.clone(),
            sprite_cull.indirect_buf.clone(),
        );

        // Terrain tiles from `Assets/Tiles.png` by direct cell mapping (16×16 grid):
        // tile_1_1 = rgb(70,83,5)  fully-opaque solid green ground = surface/grass fill
        // tile_3_1 = rgb(28,31,10) fully-opaque dark olive           = dirt layer
        // tile_6_1 = rgb(63,54,34) fully-opaque warm brown — used as base for stone.
        // Multiplied per-tile to produce neutral gray with subtle hue variation,
        // matching the visual style of dirt but in cool stone tones.
        // tile_17_3 = rgb(49,138,175) water — confirmed by pixel scan
        let grass_uv = atlas["tile_1_1"].uv;
        let dirt_uv = atlas["tile_3_1"].uv;
        let stone_uv = atlas["tile_6_1"].uv;
        let water_uv = atlas["tile_17_3"].uv;
        let mut objects: HashMap<SpriteHandle, Breakable> = HashMap::new();
        let mut occupancy_words = vec![0u32; ((OCC_COLS * OCC_ROWS + 31) / 32) as usize];
        let mark_occupied = |pos: [f32; 2], words: &mut [u32]| {
            if let Some((c, r)) = occ_cell(pos) {
                let idx = occ_index(c, r);
                words[(idx / 32) as usize] |= 1 << (idx % 32);
            }
        };

        // ── Terrain: a heightfield of tiled sprite art (the surface tile on
        // the top row, the underground tile for every row underneath — except
        // a short lake stretch where the surface row is water), inserted
        // once. Every tile is breakable and tagged with its (col, row) cell
        // so mining it out actually opens a hole in the collision heightfield
        // *and* the radiance-cascades occupancy grid (real digging, real
        // light bouncing into the hole).
        const LAKE_START: i32 = 42;
        const LAKE_END: i32 = 50;
        const DIRT_LAYERS: i32 = 3;
        for col in 0..WORLD_COLS {
            let top = surface_top_world_y(col);
            let in_lake = col >= LAKE_START && col < LAKE_END;
            let jitter = 0.92 + hash01(col as u32 * 7 + 1) * 0.16;
            let pos = [col as f32 * TILE, top - TILE * 0.5];
            let (surface_uv, surface_name) = if in_lake {
                (water_uv, "tile_17_3")
            } else {
                (grass_uv, "tile_1_1")
            };
            let handle = scene.insert_sprite(
                SpriteInstance::new(pos, [TILE + 1.0, TILE + 1.0])
                    .with_uv_rect(surface_uv)
                    .with_color([jitter, jitter, jitter, 1.0])
                    .with_atlas_layer(atlas_layer),
            );
            objects.insert(
                handle,
                Breakable {
                    pos,
                    size: [TILE, TILE],
                    depth: 0.0,
                    name: surface_name.to_string(),
                    terrain_cell: Some((col, 0)),
                },
            );
            mark_occupied(pos, &mut occupancy_words);
            for r in 1..=(DIRT_ROWS + STONE_ROWS) {
                let is_stone = r > DIRT_LAYERS;
                let uv = if is_stone { stone_uv } else { dirt_uv };
                let name = if is_stone { "tile_6_1" } else { "tile_3_1" };
                let pos = [col as f32 * TILE, top - TILE * 0.5 - r as f32 * TILE];
                // Dirt: same jitter-brightness as before (dark olive).
                // Stone: tile_6_1 base rgb(63,54,34) = normalized (0.247, 0.212, 0.133).
                //   Multiply per-channel to hit target gray ≈ 0.35-0.46 with slight
                //   warm/cool hue shifts per tile — no two stone tiles look identical.
                let color = if is_stone {
                    let base = 0.38 + (hash01(col as u32 * 17 + r as u32 * 53) - 0.5) * 0.10;
                    let hue = (hash01(col as u32 * 41 + r as u32 * 71) - 0.5) * 0.04;
                    [
                        (base + hue) / 0.247_f32,
                        base / 0.212_f32,
                        (base - hue * 0.5) / 0.133_f32,
                        1.0_f32,
                    ]
                } else {
                    let j = 0.9 + hash01(col as u32 * 17 + r as u32 * 53) * 0.2;
                    [j, j, j, 1.0]
                };
                let handle = scene.insert_sprite(
                    SpriteInstance::new(pos, [TILE + 1.0, TILE + 1.0])
                        .with_uv_rect(uv)
                        .with_color(color)
                        .with_atlas_layer(atlas_layer),
                );
                objects.insert(
                    handle,
                    Breakable {
                        pos,
                        size: [TILE, TILE],
                        depth: 0.0,
                        name: name.to_string(),
                        terrain_cell: Some((col, r)),
                    },
                );
                mark_occupied(pos, &mut occupancy_words);
            }
        }

        let mut rng = Rng::new(0xC0FF_EE12_3456_7890);
        let mut critters: Vec<Critter> = Vec::new();
        let mut items: Vec<Item> = Vec::new();

        // ── Zone boundaries (columns) — each themed band gets its own
        // sprites, so the world reads as "a forest, then a lake, then a
        // village, then a mine, then a monster den, then a second forest,
        // then a hive-lit tail" instead of one uniform scatter of everything.
        const SPAWN_END: i32 = 14;
        const FOREST_A_END: i32 = 40;
        const VILLAGE_COL: i32 = 58;
        const MINING_START: i32 = 84;
        const MINING_END: i32 = 110;
        const DEN_END: i32 = 134;
        const HUT_COL: i32 = 148;
        const FOREST_B_END: i32 = 166;
        const MARKET_START: i32 = 168;
        const TAIL_END: i32 = WORLD_COLS - 4;

        macro_rules! scatter {
            ($start:expr, $end:expr, $step:expr, $names:expr, $anim:expr, $depth:expr) => {
                scatter_band(
                    &mut scene,
                    &atlas,
                    atlas_layer,
                    &mut objects,
                    &mut rng,
                    &mut critters,
                    &mut items,
                    $start,
                    $end,
                    $step,
                    $names,
                    $anim,
                    $depth,
                    &anims,
                )
            };
        }

        // ── Spawn: lightly decorated with ground cover.
        scatter!(2, SPAWN_END, (4, 7), FOREST_CLUTTER, Animated::None, 0.2);

        // ── Forest A: green trees spaced generously (they are wide clusters),
        // bushy ground cover, and a little wildlife.
        scatter_trees(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            &mut rng,
            SPAWN_END,
            FOREST_A_END,
            (4, 8),
            TREES,
            0.15,
        );
        scatter!(
            SPAWN_END,
            FOREST_A_END,
            (2, 4),
            FOREST_CLUTTER,
            Animated::None,
            0.2
        );
        scatter!(
            SPAWN_END,
            FOREST_A_END,
            (10, 16),
            FOREST_CRITTERS,
            Animated::Critter,
            0.3
        );

        // ── Village: cabin as the centrepiece, bushes either side.
        let village_x = VILLAGE_COL as f32 * TILE;
        let cabin_spr = atlas["cabin"];
        let building = place_prop(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            "cabin",
            village_x + cabin_spr.w * 1.5,
            0.15,
            false,
            -TILE,
        );
        lay_row(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            VILLAGE_PROPS,
            village_x - building.w * 0.5 - 30.0,
            0.2,
            14.0,
        );

        // ── Mining zone: dark trees and bushes (no structures).
        scatter_trees(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            &mut rng,
            MINING_START,
            MINING_END,
            (4, 8),
            DEN_TREES,
            0.15,
        );
        scatter!(
            MINING_START,
            MINING_END,
            (2, 4),
            FOREST_CLUTTER,
            Animated::None,
            0.2
        );

        // ── Monster den: dark + red trees for atmosphere, mobs.
        scatter_trees(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            &mut rng,
            MINING_END,
            DEN_END,
            (4, 7),
            DEN_TREES,
            0.1,
        );
        scatter!(
            MINING_END,
            DEN_END,
            (5, 8),
            DEN_MONSTERS,
            Animated::Critter,
            0.3
        );

        // ── Forest B: a second, wilder patch of woods with a big green
        // landmark tree centred on the hut column.
        scatter_trees(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            &mut rng,
            DEN_END,
            FOREST_B_END,
            (4, 8),
            TREES,
            0.15,
        );
        scatter!(
            DEN_END,
            FOREST_B_END,
            (2, 4),
            FOREST_CLUTTER,
            Animated::None,
            0.2
        );
        scatter!(
            DEN_END,
            FOREST_B_END,
            (12, 18),
            FOREST_CRITTERS,
            Animated::Critter,
            0.3
        );
        place_tree(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            "tree_green_tall",
            HUT_COL as f32 * TILE,
            0.12,
            false,
        );

        // ── Market / tail: golden and yellow autumn trees + bushes.
        scatter_trees(
            &mut scene,
            &atlas,
            atlas_layer,
            &mut objects,
            &mut rng,
            MARKET_START,
            MARKET_START + 20,
            (3, 6),
            MARKET_TREES,
            0.15,
        );
        scatter!(
            MARKET_START,
            TAIL_END,
            (2, 4),
            MARKET_CLUTTER,
            Animated::None,
            0.2
        );

        // ── Lighting: 2D radiance cascades reading the occupancy grid built
        // above (occluders) plus every placed interior prop that is a light
        // emitter (see `LIGHT_EMITTER_NAMES`), each with a tight pool of light.
        let mut gpu_emitters: Vec<GpuEmitter> = objects
            .values()
            .filter(|b| LIGHT_EMITTER_NAMES.contains(&b.name.as_str()))
            .map(|b| {
                let (color, radius) = emitter_style(&b.name);
                GpuEmitter {
                    pos: b.pos,
                    radius,
                    r: color[0],
                    g: color[1],
                    b: color[2],
                    _pad: 0.0,
                    _pad2: 0.0,
                }
            })
            .collect();
        let real_emitter_count = gpu_emitters.len() as u32;
        let max_emitters = real_emitter_count.max(1);
        gpu_emitters.resize(
            max_emitters as usize,
            GpuEmitter {
                pos: [0.0, 0.0],
                radius: 0.0,
                r: 0.0,
                g: 0.0,
                b: 0.0,
                _pad: 0.0,
                _pad2: 0.0,
            },
        );
        log::info!("[sprite_dig_demo] {real_emitter_count} light emitters");

        let occupancy_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Occupancy Grid"),
            size: (occupancy_words.len() * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        queue.write_buffer(&occupancy_buf, 0, bytemuck::cast_slice(&occupancy_words));
        let emitters_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Light Emitters"),
            size: (gpu_emitters.len() * std::mem::size_of::<GpuEmitter>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        queue.write_buffer(&emitters_buf, 0, bytemuck::cast_slice(&gpu_emitters));

        let mut radiance_pass = RadianceCascades2DPass::new(
            &device,
            &queue,
            RadianceCascadesConfig {
                max_emitters,
                ..Default::default()
            },
            occupancy_buf.clone(),
            (OCC_COLS, OCC_ROWS),
            TILE,
            OCC_ORIGIN,
            emitters_buf,
        );
        radiance_pass.set_emitter_count(real_emitter_count);
        let radiance_composite = RadianceCascadesCompositePass::new(
            &device,
            &queue,
            format,
            radiance_pass.radiance_view(),
            [1.35, 1.25, 1.10], // bright, warm daytime sky ambient
            2.0,
        );

        // ── Player, spawned standing on the surface near the hive marker. ─
        let player_frame = &anims["player/idle"][0];
        let player_spr = atlas[player_frame];
        let spawn_col = 4;
        let player_pos = [
            spawn_col as f32 * TILE,
            surface_top_world_y(spawn_col) + player_spr.h * PLAYER_SCALE * 0.5,
        ];
        let player_handle = scene.insert_sprite(
            SpriteInstance::new(
                player_pos,
                [player_spr.w * PLAYER_SCALE, player_spr.h * PLAYER_SCALE],
            )
            .with_uv_rect(player_spr.uv)
            .with_depth(0.5)
            .with_atlas_layer(atlas_layer),
        );

        // ── Parallax background: the whole-sky `Background/Background.png`,
        // scaled to cover the window and anchored in the sky region above
        // terrain. The native sprite is 480×272; we scale it to window height
        // × 1.1 so it always fills the visible sky regardless of resolution.
        let bg_spr = atlas["background"];
        let bg_scale = (size.height as f32 / bg_spr.h).max(2.0) * 1.1;
        let bg_draw = [bg_spr.w * bg_scale, bg_spr.h * bg_scale];
        let bg_handle = scene.insert_sprite(
            SpriteInstance::new([bg_draw[0] * 0.5, size.height as f32 * 0.5], bg_draw)
                .with_uv_rect(bg_spr.uv)
                .with_depth(-10.0)
                .with_atlas_layer(atlas_layer),
        );
        let bg_uv = bg_spr.uv;

        graph.add_pass(Box::new(sprite_cull));
        graph.add_pass(Box::new(sprite_pass));
        graph.add_pass(Box::new(radiance_pass));
        graph.add_pass(Box::new(radiance_composite));
        graph.lock(size.width.max(1), size.height.max(1));

        scene.sync();
        let dummy_depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dummy Depth (unused by 2D passes)"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dummy_depth_view = dummy_depth.create_view(&wgpu::TextureViewDescriptor::default());

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            graph,
            scene,
            dummy_depth_view,
            atlas,
            atlas_layer,
            crack_uvs,
            player_handle,
            player_pos,
            player_vel: [0.0, 0.0],
            player_on_ground: false,
            player_facing_right: true,
            anims,
            player_box: [player_spr.w * PLAYER_SCALE, player_spr.h * PLAYER_SCALE],
            player_anim: PlayerAnim::Idle,
            player_anim_time: 0.0,
            bg_handle,
            bg_uv,
            bg_size: bg_draw,
            camera_center: player_pos,
            keys: HashSet::new(),
            mouse_pos: (0.0, 0.0),
            critters,
            items,
            objects,
            broken_terrain: HashSet::new(),
            breaking: None,
            hotbar: Vec::new(),
            hotbar_selected: 0,
            occupancy_buf,
            occupancy_words,
            start_time: Instant::now(),
            last_frame: Instant::now(),
            fps_frames: 0,
            fps_last_print: Instant::now(),
            window_size: (size.width.max(1), size.height.max(1)),
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: state.surface_format,
                        width: s.width,
                        height: s.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.graph.set_render_size(s.width, s.height);
                state.window_size = (s.width, s.height);
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(code) = key_event.physical_key {
                    match key_event.state {
                        ElementState::Pressed => {
                            state.keys.insert(code);
                        }
                        ElementState::Released => {
                            state.keys.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.mouse_pos = (position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => match (button, btn_state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    let world =
                        world_from_screen(state.mouse_pos, state.window_size, state.camera_center);
                    if let Some(handle) = hit_test(&state.objects, world) {
                        let target = state.objects[&handle].clone();
                        state.breaking = Some(Breaking {
                            handle,
                            target,
                            start: Instant::now(),
                            crack_handle: None,
                            stage: 0,
                        });
                    }
                }
                (MouseButton::Left, ElementState::Released) => {
                    if let Some(b) = state.breaking.take() {
                        if let Some(ch) = b.crack_handle {
                            state.scene.remove_sprite(ch);
                        }
                    }
                }
                (MouseButton::Right, ElementState::Pressed) => {
                    if !state.hotbar.is_empty() {
                        let world = world_from_screen(
                            state.mouse_pos,
                            state.window_size,
                            state.camera_center,
                        );
                        let sel = state.hotbar_selected.min(state.hotbar.len() - 1);
                        let (name, uv, w, h) = {
                            let s = &state.hotbar[sel];
                            (s.name.clone(), s.uv, s.w, s.h)
                        };
                        let snapped = [(world[0] / TILE).round() * TILE, world[1]];
                        let atlas_layer = state.atlas_layer;
                        let handle = state.scene.insert_sprite(
                            SpriteInstance::new(snapped, [w, h])
                                .with_uv_rect(uv)
                                .with_depth(0.2)
                                .with_atlas_layer(atlas_layer),
                        );
                        state.objects.insert(
                            handle,
                            Breakable {
                                pos: snapped,
                                size: [w, h],
                                depth: 0.2,
                                name,
                                terrain_cell: None,
                            },
                        );
                        state.hotbar[sel].count -= 1;
                        if state.hotbar[sel].count == 0 {
                            let removed = state.hotbar.remove(sel);
                            state.scene.remove_sprite(removed.handle);
                            if state.hotbar_selected >= state.hotbar.len() {
                                state.hotbar_selected = state.hotbar.len().saturating_sub(1);
                            }
                        }
                    }
                }
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if !state.hotbar.is_empty() {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y as f32) * 0.02,
                    };
                    if dy.abs() > 0.01 {
                        let n = state.hotbar.len() as i32;
                        let dir = if dy > 0.0 { -1 } else { 1 };
                        state.hotbar_selected =
                            (state.hotbar_selected as i32 + dir).rem_euclid(n) as usize;
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;
                let time = state.start_time.elapsed().as_secs_f32();
                let atlas_layer = state.atlas_layer;

                // ── Mining: advance the crack overlay, or finalize a break.
                let mut should_finish = false;
                if let Some(breaking) = state.breaking.as_mut() {
                    let elapsed = breaking.start.elapsed().as_secs_f32();
                    let stage =
                        ((elapsed / BREAK_STAGE_DURATION).floor() as u32).min(BREAK_TOTAL_STAGES);
                    if stage != breaking.stage {
                        breaking.stage = stage;
                        if stage >= 1 {
                            let crack_uv = state.crack_uvs[(stage - 1) as usize];
                            let inst =
                                SpriteInstance::new(breaking.target.pos, breaking.target.size)
                                    .with_uv_rect(crack_uv)
                                    .with_depth(breaking.target.depth + 0.01)
                                    .with_atlas_layer(atlas_layer);
                            match breaking.crack_handle {
                                Some(ch) => state.scene.update_sprite(ch, inst),
                                None => {
                                    breaking.crack_handle = Some(state.scene.insert_sprite(inst))
                                }
                            }
                        }
                    }
                    should_finish = stage >= BREAK_TOTAL_STAGES;
                }
                if should_finish {
                    let breaking = state.breaking.take().unwrap();
                    state.scene.remove_sprite(breaking.handle);
                    if let Some(ch) = breaking.crack_handle {
                        state.scene.remove_sprite(ch);
                    }
                    state.objects.remove(&breaking.handle);
                    state.critters.retain(|c| c.handle != breaking.handle);
                    state.items.retain(|it| it.handle != breaking.handle);
                    if let Some(cell) = breaking.target.terrain_cell {
                        state.broken_terrain.insert(cell);
                        // Clear the same tile in the lighting occupancy grid
                        // (a flat world-space grid, not `terrain_cell`'s
                        // column-relative one — see `occ_cell`) so light can
                        // actually pour into the hole just dug.
                        if let Some((c, r)) = occ_cell(breaking.target.pos) {
                            let idx = occ_index(c, r);
                            let word = (idx / 32) as usize;
                            state.occupancy_words[word] &= !(1 << (idx % 32));
                            state.queue.write_buffer(
                                &state.occupancy_buf,
                                (word * 4) as u64,
                                bytemuck::bytes_of(&state.occupancy_words[word]),
                            );
                        }
                    }
                    if let Some(slot) = state
                        .hotbar
                        .iter_mut()
                        .find(|s| s.name == breaking.target.name)
                    {
                        slot.count += 1;
                    } else {
                        let s = state.atlas[&breaking.target.name];
                        let index = state.hotbar.len();
                        let pos = hotbar_slot_world_pos(
                            state.camera_center,
                            state.window_size,
                            index,
                            index + 1,
                        );
                        let handle = state.scene.insert_sprite(
                            SpriteInstance::new(pos, [HOTBAR_ICON_SIZE, HOTBAR_ICON_SIZE])
                                .with_uv_rect(s.uv)
                                .with_depth(0.9)
                                .with_atlas_layer(atlas_layer),
                        );
                        state.hotbar.push(HotbarSlot {
                            name: breaking.target.name,
                            count: 1,
                            handle,
                            uv: s.uv,
                            w: s.w,
                            h: s.h,
                        });
                    }
                }

                // ── Player physics: simple gravity + blocky heightfield
                // collision (snap to the nearest terrain column's surface,
                // accounting for any tiles mined out from under it).
                let mut move_dir = 0.0f32;
                if state.keys.contains(&KeyCode::KeyA) || state.keys.contains(&KeyCode::ArrowLeft) {
                    move_dir -= 1.0;
                }
                if state.keys.contains(&KeyCode::KeyD) || state.keys.contains(&KeyCode::ArrowRight)
                {
                    move_dir += 1.0;
                }
                if move_dir != 0.0 {
                    state.player_facing_right = move_dir > 0.0;
                }
                state.player_vel[0] = move_dir * MOVE_SPEED;
                state.player_vel[1] += GRAVITY * dt;
                if state.keys.contains(&KeyCode::Space) && state.player_on_ground {
                    state.player_vel[1] = JUMP_VEL;
                    state.player_on_ground = false;
                }
                state.player_pos[0] += state.player_vel[0] * dt;
                state.player_pos[1] += state.player_vel[1] * dt;
                state.player_pos[0] =
                    state.player_pos[0].clamp(TILE * 1.0, (WORLD_COLS as f32 - 2.0) * TILE);

                let col = (state.player_pos[0] / TILE).round() as i32;
                let ground_y = ground_y_at(col, &state.broken_terrain) + state.player_box[1] * 0.5;
                if state.player_pos[1] <= ground_y {
                    state.player_pos[1] = ground_y;
                    state.player_vel[1] = 0.0;
                    state.player_on_ground = true;
                } else {
                    state.player_on_ground = false;
                }

                // ── Player animation: idle/run from the character sheet, and
                // a two-pose jump (rising frames / falling frames).
                let new_anim = if !state.player_on_ground {
                    if state.player_vel[1] > 0.0 {
                        PlayerAnim::Jump
                    } else {
                        PlayerAnim::Fall
                    }
                } else if state.player_vel[0].abs() > 1.0 {
                    PlayerAnim::Run
                } else {
                    PlayerAnim::Idle
                };
                if new_anim != state.player_anim {
                    state.player_anim = new_anim;
                    state.player_anim_time = 0.0;
                } else {
                    state.player_anim_time += dt;
                }
                let (group, fps) = match state.player_anim {
                    PlayerAnim::Idle => ("player/idle", 7.0), // 4 frames — gentle sway
                    PlayerAnim::Run => ("player/run", 12.0),  // 8 frames — brisk sprint
                    PlayerAnim::Jump => ("player/jump", 14.0), // 15 frames — snappy arc
                    PlayerAnim::Fall => ("player/jump_end", 10.0), // 3 frames — loop landing
                };
                let frames = &state.anims[group];
                let frame_idx = ((state.player_anim_time * fps) as usize) % frames.len();
                let spr = state.atlas[&frames[frame_idx]];
                let uv = if state.player_facing_right {
                    spr.uv
                } else {
                    flip_u(spr.uv)
                };
                let player_pos = state.player_pos;
                state.scene.update_sprite(
                    state.player_handle,
                    SpriteInstance::new(player_pos, [spr.w * PLAYER_SCALE, spr.h * PLAYER_SCALE])
                        .with_uv_rect(uv)
                        .with_depth(0.5)
                        .with_atlas_layer(atlas_layer),
                );

                for c in &state.critters {
                    let frame_idx = ((time * c.fps) as usize) % c.frames.len();
                    let spr = state.atlas[&c.frames[frame_idx]];
                    let mut pos = c.base_pos;
                    pos[1] += (time * 2.0 + c.phase).sin() * 6.0;
                    state.scene.update_sprite(
                        c.handle,
                        SpriteInstance::new(pos, [spr.w, spr.h])
                            .with_uv_rect(spr.uv)
                            .with_depth(0.3)
                            .with_atlas_layer(atlas_layer),
                    );
                }
                for it in &state.items {
                    let mut pos = it.base_pos;
                    pos[1] += (time * 1.5 + it.phase).sin() * 4.0;
                    state.scene.update_sprite(
                        it.handle,
                        SpriteInstance::new(pos, [it.spr.w, it.spr.h])
                            .with_uv_rect(it.spr.uv)
                            .with_rotation(time * it.spin + it.phase)
                            .with_depth(0.3)
                            .with_atlas_layer(atlas_layer),
                    );
                }

                // ── Camera: smoothly follows the player, biased up a bit so
                // more sky/foreground is visible ahead of the player.
                let target = [state.player_pos[0], state.player_pos[1] + 100.0];
                let smoothing = (dt * 5.0).min(1.0);
                state.camera_center[0] += (target[0] - state.camera_center[0]) * smoothing;
                state.camera_center[1] += (target[1] - state.camera_center[1]) * smoothing;

                // ── Background: sky backdrop scaled to fill the window.
                // Horizontal: very slow drift (5 % of camera speed) that
                // tiles every bg_draw_width, giving a gentle cloud motion
                // without a visible seam.  Vertical: anchored to the upper
                // portion of the visible screen (sky region above terrain).
                let bg_size = state.bg_size;
                let drift_x = state.camera_center[0] * 0.05;
                let bg_pos = [
                    state.camera_center[0] + drift_x.rem_euclid(bg_size[0]) - bg_size[0] * 0.5,
                    // upper ~35 % of the screen (Y-up: add half window height)
                    state.camera_center[1] + state.window_size.1 as f32 * 0.35,
                ];
                state.scene.update_sprite(
                    state.bg_handle,
                    SpriteInstance::new(bg_pos, bg_size)
                        .with_uv_rect(state.bg_uv)
                        .with_depth(-10.0)
                        .with_atlas_layer(atlas_layer),
                );

                // ── Hotbar: screen-locked (re-anchored to the camera every
                // frame), the selected slot tinted — no on-screen counts,
                // just internal bookkeeping per the design.
                let n = state.hotbar.len();
                for (i, slot) in state.hotbar.iter().enumerate() {
                    let pos = hotbar_slot_world_pos(state.camera_center, state.window_size, i, n);
                    let tint = if i == state.hotbar_selected {
                        [1.35, 1.25, 0.55, 1.0]
                    } else {
                        [1.0, 1.0, 1.0, 1.0]
                    };
                    state.scene.update_sprite(
                        slot.handle,
                        SpriteInstance::new(pos, [HOTBAR_ICON_SIZE, HOTBAR_ICON_SIZE])
                            .with_uv_rect(slot.uv)
                            .with_color(tint)
                            .with_depth(0.9)
                            .with_atlas_layer(atlas_layer),
                    );
                }

                let sprite_pass = state
                    .graph
                    .find_pass_mut::<SpriteBatchPass>()
                    .expect("sprite batch pass missing from graph");
                let (win_w, win_h) = state.window_size;
                sprite_pass.set_camera(
                    state.camera_center,
                    Some([win_w as f32 * 0.5 / ZOOM, win_h as f32 * 0.5 / ZOOM]),
                );
                state
                    .graph
                    .find_pass_mut::<SpriteCullPass>()
                    .expect("sprite cull pass missing from graph")
                    .set_view_rect(
                        state.camera_center,
                        [win_w as f32 * 0.5 / ZOOM, win_h as f32 * 0.5 / ZOOM],
                    );
                state
                    .graph
                    .find_pass_mut::<RadianceCascades2DPass>()
                    .expect("radiance cascades pass missing from graph")
                    .set_view(
                        state.camera_center,
                        [win_w as f32 * 0.5, win_h as f32 * 0.5],
                    );

                state.fps_frames += 1;
                if state.fps_last_print.elapsed().as_secs_f32() >= 1.0 {
                    let elapsed = state.fps_last_print.elapsed().as_secs_f32();
                    log::info!(
                        "[sprite_dig_demo] {:.0} fps | pool={} | hotbar_slots={}",
                        state.fps_frames as f32 / elapsed,
                        POOL_CAPACITY,
                        state.hotbar.len(),
                    );
                    state.fps_frames = 0;
                    state.fps_last_print = Instant::now();
                }

                let output = match state.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
                    _ => {
                        state.window.request_redraw();
                        return;
                    }
                };
                let view = output.texture.create_view(&Default::default());
                state.scene.sync();
                if let Err(e) = state
                    .graph
                    .execute(&state.scene, &view, &state.dummy_depth_view)
                {
                    log::error!("graph execute error: {e:?}");
                }
                state.queue.present(output);
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}
