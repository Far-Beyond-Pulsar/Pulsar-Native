//! `VOXEL_CRAFT_STROKE=<op>:<radius m>:<stamps>[,...]` with
//! `VOXEL_CRAFT_SHOTS`: sculpt strokes for measuring what edits cost the
//! frame. Each stroke stamps one brush a frame (`dig`: a ball taken out,
//! `build`: a cube of cobblestone), dragged sideways back and forth over the
//! ground 12 m ahead of the spawn, inside a query focus of 16 m around it (as
//! an editor's sculpt focus), then waits for the world to settle. A line
//! `STROKE` gives the frame times over the stamps and the settle, the
//! world's part of the frames (snapshots, queries, the player: where a
//! query waiting for an edit would show) and the longest `World::stamp`
//! call; the run quits after the last stroke.

use crate::blocks::Block;
use crate::player::Player;
use crate::world::World;
use glam::DVec3;
use helio_pass_voxel_planet::{Brush, BrushOp, BrushShape, Matter};
use std::time::Instant;

/// The query focus kept around a stroke.
const STROKE_FOCUS: u64 = 2;
const STROKE_FOCUS_M: f64 = 16.0;
/// Frames waited for the world to settle after a stroke, at most.
const SETTLE_MAX: u32 = 600;

#[derive(Clone, Copy, Debug)]
struct Stroke {
    dig: bool,
    radius: f64,
    stamps: u32,
}

pub struct Strokes {
    strokes: Vec<Stroke>,
    /// The stroke under way, its stamps so far and the frames since it ended.
    at: usize,
    stamped: u32,
    settling: u32,
    /// Frames waited before the first stroke.
    waited: u32,
    target: Option<(DVec3, DVec3, DVec3)>,
    last: Option<Instant>,
    stamp_ms: Vec<f64>,
    settle_ms: Vec<f64>,
    world_ms: Vec<f64>,
    call_ms: f64,
}

impl Strokes {
    pub fn from_env() -> Option<Self> {
        let spec = std::env::var("VOXEL_CRAFT_STROKE").ok()?;
        let strokes = spec
            .split(',')
            .filter_map(|s| {
                let mut parts = s.split(':');
                let dig = match parts.next()? {
                    "dig" => true,
                    "build" => false,
                    _ => return None,
                };
                Some(Stroke {
                    dig,
                    radius: parts.next()?.parse().ok()?,
                    stamps: parts.next().and_then(|n| n.parse().ok()).unwrap_or(60),
                })
            })
            .collect::<Vec<_>>();
        (!strokes.is_empty()).then_some(Self {
            strokes,
            at: 0,
            stamped: 0,
            settling: 0,
            waited: 0,
            target: None,
            last: None,
            stamp_ms: Vec::new(),
            settle_ms: Vec::new(),
            world_ms: Vec::new(),
            call_ms: 0.0,
        })
    }

    /// One frame of the strokes; `settled`: the view and the world are;
    /// `world_ms`: the world's part of this frame.
    pub fn step(
        &mut self,
        player: &mut Player,
        world: &mut World,
        settled: bool,
        world_ms: f64,
        quit: &mut bool,
    ) {
        let now = Instant::now();
        let frame_ms = self.last.map(|l| (now - l).as_secs_f64() * 1e3);
        self.last = Some(now);
        let Some(&stroke) = self.strokes.get(self.at) else {
            *quit = true;
            return;
        };
        let (ground, right, up) = *self.target.get_or_insert_with(|| {
            let frame = player.frame();
            let planet = world.planet();
            let ground = planet.surface_point_of(
                player.position + frame.forward * 12.0,
                0.0,
                Matter::Ground,
            );
            (ground, frame.right, frame.up)
        });
        world.focus_on(STROKE_FOCUS, ground, STROKE_FOCUS_M);
        if self.waited < 120 || (self.stamped == 0 && !settled && self.waited < 900) {
            self.waited += 1;
            return;
        }
        if self.stamped > 0 {
            self.world_ms.push(world_ms);
        }
        if self.stamped < stroke.stamps {
            if self.stamped > 0 {
                self.stamp_ms.extend(frame_ms);
            }
            let n = f64::from(self.stamped);
            let at = ground + right * (stroke.radius * (n * 0.35).sin());
            let brush = if stroke.dig {
                Brush {
                    center: at.to_array(),
                    radius: stroke.radius,
                    shape: BrushShape::Sphere,
                    op: BrushOp::Remove,
                    material: 0,
                    height: 0.0,
                }
            } else {
                Brush {
                    center: (at + up * (stroke.radius * 0.5)).to_array(),
                    radius: stroke.radius,
                    shape: BrushShape::Cube,
                    op: BrushOp::Add,
                    material: Block::Cobblestone.material(),
                    height: 0.0,
                }
            };
            let started = Instant::now();
            world.stamp(&[brush]);
            self.call_ms = self.call_ms.max(started.elapsed().as_secs_f64() * 1e3);
            self.stamped += 1;
            return;
        }
        if self.settling == 0 {
            self.stamp_ms.extend(frame_ms);
        } else {
            self.settle_ms.extend(frame_ms);
        }
        self.settling += 1;
        if !(settled && self.settling > 30) && self.settling < SETTLE_MAX {
            return;
        }
        let stat = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let at = |q: f64| {
                v.get(((v.len() as f64 - 1.0) * q).round() as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            format!(
                "p50 {:.1} p95 {:.1} max {:.1} ms",
                at(0.5),
                at(0.95),
                at(1.0)
            )
        };
        log::info!(
            "STROKE {} r {} m, {} stamps: frames {}; settle {} frames {}; world {}; longest stamp call {:.1} ms",
            if stroke.dig { "dig" } else { "build" },
            stroke.radius,
            stroke.stamps,
            stat(&mut self.stamp_ms),
            self.settling,
            stat(&mut self.settle_ms),
            stat(&mut self.world_ms),
            self.call_ms,
        );
        self.stamp_ms.clear();
        self.settle_ms.clear();
        self.world_ms.clear();
        self.call_ms = 0.0;
        self.stamped = 0;
        self.settling = 0;
        self.at += 1;
        // The next stroke on fresh ground: a quarter turn.
        let (_, right, up) = self.target.expect("set above");
        let ground =
            world
                .planet()
                .surface_point_of(player.position + right * 12.0, 0.0, Matter::Ground);
        self.target = Some((ground, right.cross(up), up));
    }
}
