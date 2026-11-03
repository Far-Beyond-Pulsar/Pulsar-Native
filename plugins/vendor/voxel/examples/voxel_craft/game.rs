//! The game: its state and one frame of it.

use crate::blocks::Block;
use crate::config::*;
use crate::gfx::{Gfx, View};
use crate::input::Input;
use crate::player::interaction::Interaction;
use crate::player::Player;
use crate::terrain::{self, Biome};
use crate::world::edits::Edit;
use crate::world::World;
use glam::DVec3;
use helio_pass_voxel_planet::{Cell, Planet};
use std::time::Instant;
use winit::keyboard::KeyCode;

pub struct Game {
    #[cfg(not(target_arch = "wasm32"))]
    rig: crate::xr::rig::Rig,
    pub gfx: Gfx,
    pub world: World,
    pub player: Player,
    pub interaction: Interaction,
    pub input: Input,
    last_frame: Instant,
    started: Instant,
    last_title: Instant,
    frames: u32,
    fps: f64,
    /// Seconds of game time, and its speed relative to the clock.
    time: f64,
    time_speed: f64,
    /// The region the world was last told the player is in.
    /// The sun's path: a great circle through the sky over the spawn.
    sun_axes: (DVec3, DVec3),
    show_help: bool,
    total_frames: u64,
    frame_limit: Option<u64>,
    tour: Option<Tour>,
    pub quit: bool,
}

/// A grassy, level place to start on.
fn find_spawn(planet: &Planet) -> DVec3 {
    let grid = planet.grid();
    let face = grid.faces()[0];
    let n = grid.cells();
    let (ci, cj) = (n / 2, n / 2);
    let mut fallback = None;
    // Rings outward from the middle of the face.
    for ring in 0..400 {
        for step in 0..(8 * ring.max(1)) {
            let t = f64::from(step) / f64::from(8 * ring.max(1)) * std::f64::consts::TAU;
            let (i, j) = (
                ci + (t.cos() * f64::from(ring) * 12.0) as i32,
                cj + (t.sin() * f64::from(ring) * 12.0) as i32,
            );
            let Some(ground) = terrain::ground(planet, face, i, j) else {
                continue;
            };
            let top = planet.column_top(face, i, j, 0);
            let height = f64::from(top) * grid.voxel_size();
            if ground.cell.k + 1 != top || terrain::relief(planet, face, i, j, 3) > 2 {
                continue;
            }
            fallback.get_or_insert((i, j, top));
            if Biome::of(ground.material) == Biome::Temperate && (4.0..40.0).contains(&height) {
                return grid.at_radial(
                    grid.direction(face, f64::from(i) + 0.5, f64::from(j) + 0.5),
                    grid.layer_radius(f64::from(top)) + 0.01,
                );
            }
        }
    }
    let (i, j, top) = fallback.unwrap_or((ci, cj, 8));
    grid.at_radial(
        grid.direction(face, f64::from(i) + 0.5, f64::from(j) + 0.5),
        grid.layer_radius(f64::from(top)) + 0.01,
    )
}

impl Game {
    pub fn new(gfx: impl FnOnce(&Planet) -> Gfx) -> Self {
        let started = Instant::now();
        let planet = Planet::new(terrain::recipe::recipe()).expect("the planet's recipe is valid");
        log::info!("planet built in {:.2} s", started.elapsed().as_secs_f64());
        let spawn = find_spawn(&planet);
        let gfx = gfx(&planet);
        let up = spawn.normalize();
        let e1 = up.any_orthonormal_vector();
        let e2 = up.cross(e1);
        let world = World::new(planet);
        // The world around the spawn, before the first step.
        world.follow(spawn);
        world.settle();
        let mut player = Player::new(spawn);
        player.heading = e2;
        let tour = std::env::var("VOXEL_CRAFT_SHOTS").ok().map(Tour::new);
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            rig: crate::xr::rig::Rig::default(),
            gfx,
            world,
            player,
            interaction: Interaction::default(),
            input: Input::default(),
            last_frame: Instant::now(),
            started,
            last_title: Instant::now(),
            frames: 0,
            fps: 0.0,
            // Mid-morning.
            time: DAY_SECONDS * 0.08,
            // A tour keeps the sun where it is: shots compare.
            time_speed: if tour.is_some() { 0.0 } else { 1.0 },
            sun_axes: (up, e1),
            show_help: true,
            total_frames: 0,
            frame_limit: std::env::var("VOXEL_CRAFT_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok()),
            tour,
            quit: false,
        }
    }

    /// Whether the screenshot tour is running (`VOXEL_CRAFT_SHOTS`): it
    /// poses the camera itself and takes no input.
    pub fn touring(&self) -> bool {
        self.tour.is_some()
    }

    /// The direction to the sun at game time `time`: a great circle that
    /// passes over the spawn at noon.
    fn sun(&self) -> DVec3 {
        let a = self.time / DAY_SECONDS * std::f64::consts::TAU + 0.55;
        let (up, side) = self.sun_axes;
        (up * a.sin() + side * a.cos()).normalize()
    }

    /// Runs one frame.
    pub fn frame(&mut self) {
        // Wait for the compositor before simulation and locate input at this
        // frame's predicted display time, rather than reusing last frame's hands.
        #[cfg(not(target_arch = "wasm32"))]
        let xr_input = if self.gfx.vr_active() {
            match self.gfx.xr_frame_input() {
                Ok(input) => {
                    if input.as_ref().is_none_or(|input| !input.focused) {
                        self.input.release_all();
                    }
                    input
                }
                Err(error) => {
                    log::error!("[XR] input: {error}");
                    self.input.release_all();
                    None
                }
            }
        } else {
            None
        };
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64().min(0.1);
        self.last_frame = now;
        if dt > 0.05 {
            log::info!("HITCH {:.0} ms at frame {}", dt * 1e3, self.total_frames);
        }
        self.time += dt * self.time_speed;

        // Keys that are not movement.
        if self.input.just_pressed(KeyCode::KeyF) {
            self.player.flying = !self.player.flying;
            self.player.velocity = DVec3::ZERO;
        }
        if self.input.just_pressed(KeyCode::Space) && self.player.jump_pressed() {
            self.player.flying = !self.player.flying;
            self.player.velocity = DVec3::ZERO;
        }
        if !self.gfx.vr_active()
            && (self.input.just_pressed(KeyCode::F5) || self.input.just_pressed(KeyCode::KeyV))
        {
            self.player.third_person = !self.player.third_person;
        }
        if self.input.just_pressed(KeyCode::KeyR) {
            self.player.respawn();
            #[cfg(not(target_arch = "wasm32"))]
            self.rig.reset();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.input.just_pressed(KeyCode::KeyC) {
            self.rig.reset();
        }
        if self.input.just_pressed(KeyCode::KeyH) {
            self.show_help = !self.show_help;
        }
        if self.input.just_pressed(KeyCode::KeyT) {
            self.time_speed = if self.time_speed > 1.5 { 1.0 } else { 40.0 };
        }
        if let Some(slot) = self.input.hotbar_key() {
            self.player.hotbar = slot;
        }
        let scroll = self.input.scroll();
        if scroll != 0.0 {
            let n = Block::HOTBAR.len() as i32;
            self.player.hotbar =
                (self.player.hotbar as i32 - scroll.signum() as i32).rem_euclid(n) as usize;
        }

        let mut controls = self.input.controls();
        // The world's part of the frame: snapshots, queries, the player.
        let world_started = Instant::now();
        self.world.update();
        self.world.follow(self.player.position);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(input) = &xr_input {
            self.rig
                .update(input, &mut self.player, &self.world, &mut controls);
        }
        self.player.update(&self.world, &controls, dt);
        #[cfg(not(target_arch = "wasm32"))]
        let vr = xr_input
            .as_ref()
            .map(|input| self.rig.view(&self.player, input));
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(vr) = &vr {
            controls.aim = Some(vr.aim);
        }
        self.interaction
            .update(&mut self.player, &mut self.world, &controls, dt);
        let world_ms = world_started.elapsed().as_secs_f64() * 1e3;

        self.total_frames += 1;
        if let Some(t) = &self.tour {
            if t.trunk && self.total_frames == 700 {
                let here = self.world.cell_at(self.player.position);
                match nearest_wood(&self.world, here, (15.0 / crate::config::VOXEL_M) as i32) {
                    Some((cell, block)) => {
                        log::info!(
                            "TRUNK {:?} at {:?} is {:?}, drawn material {}",
                            block,
                            cell,
                            self.world.block_at(cell),
                            block.material()
                        );
                        let target = self.world.cell_centre(cell);
                        let up = target.normalize();
                        let along = (self.world.cell_centre(Cell::new(
                            cell.face,
                            cell.i,
                            cell.j + 1,
                            cell.k,
                        )) - target)
                            .normalize();
                        for back in 1..=3 {
                            let c = Cell::new(cell.face, cell.i, cell.j - back, cell.k);
                            log::info!("TRUNK in front: {:?} -> {:?}", c, self.world.block_at(c));
                        }
                        self.player.flying = true;
                        self.player.position = target - along * 4.0 - up * 1.12;
                        self.player.heading = along;
                        self.player.pitch = 0.0;
                        self.player.velocity = DVec3::ZERO;
                    }
                    None => log::info!("TRUNK none found"),
                }
            }
        }
        let settled = self.gfx.settled() && self.world.loaded();
        let shot = self
            .tour
            .as_mut()
            .and_then(|t| {
                t.world_ms = world_ms;
                t.step(&mut self.player, &mut self.world, settled, &mut self.quit)
            });
        let (eye, look) = self.player.camera(&self.world);
        #[cfg(not(target_arch = "wasm32"))]
        let (eye, look) = vr.as_ref().map_or((eye, look), |vr| {
            (
                vr.head.w_axis.truncate(),
                vr.head.transform_vector3(DVec3::NEG_Z).normalize(),
            )
        });
        let outline = self.interaction.outline(&self.world);
        let view = View {
            #[cfg(not(target_arch = "wasm32"))]
            vr: vr.as_ref(),
            target: self.interaction.target.map(|target| target.point),
            dt: dt as f32,
            planet: self.world.planet(),
            player: &self.player,
            eye,
            look,
            sun: self.sun(),
            outline: &outline,
            time: self.started.elapsed().as_secs_f64(),
            weather: self.time,
            screenshot: shot.as_deref(),
        };
        self.gfx.draw(&view);
        self.quit |= self.gfx.failed;
        if self
            .frame_limit
            .is_some_and(|limit| self.total_frames >= limit)
        {
            log::info!(
                "completed {} frames ({})",
                self.total_frames,
                if self.gfx.vr_active() {
                    "VR"
                } else {
                    "desktop"
                }
            );
            self.quit = true;
        }
        self.input.end_frame();

        self.frames += 1;
        let since = self.last_title.elapsed().as_secs_f64();
        if since > 0.5 {
            self.fps = f64::from(self.frames) / since;
            self.frames = 0;
            self.last_title = Instant::now();
            self.update_title();
        }
    }

    fn update_title(&self) {
        let grid = self.world.planet().grid();
        let height = grid.height(self.player.position);
        let hotbar: Vec<String> = Block::HOTBAR
            .iter()
            .enumerate()
            .map(|(n, b)| {
                if n == self.player.hotbar {
                    format!("[{} {}]", n + 1, b.name())
                } else {
                    format!("{} {}", n + 1, b.name())
                }
            })
            .collect();
        let hour = (self.time / DAY_SECONDS * 24.0 + 9.0).rem_euclid(24.0);
        let help = if self.show_help && self.gfx.vr_active() {
            "  |  VR: left stick move, right stick turn/hotbar, triggers dig/build, A jump, B fly, X pick, Y recenter"
        } else if self.show_help {
            "  |  WASD move, Space jump (tap twice or F: fly), Ctrl sprint, LMB break, RMB place, MMB pick, wheel/1-9 hotbar, V camera, T time, R respawn, H help, Esc free mouse"
        } else {
            ""
        };
        let target = self
            .interaction
            .target
            .and_then(|t| self.world.block_at(t.cell))
            .map_or(String::new(), |b| format!("  |  looking at {}", b.name()));
        self.gfx.window.set_title(&format!(
            "Voxel Craft  |  {:.0} fps  |  {:02}:{:02}  |  height {:.0} m  |  {}{}{}{}",
            self.fps,
            hour as u32,
            ((hour.fract()) * 60.0) as u32,
            height,
            hotbar.join("  "),
            target,
            if self.player.flying {
                "  |  FLYING"
            } else {
                ""
            },
            help
        ));
    }
}

/// `VOXEL_CRAFT_SHOTS=<dir>`: a scripted tour that poses the camera, waits for
/// the terrain to settle, saves `<dir>/shot_N.png` for each pose and quits.
/// For comparing renders without a person at the keyboard.
struct Tour {
    dir: String,
    pose: usize,
    /// Frames since the pose was set.
    waited: u32,
    /// Look at the nearest tree trunk instead of the posed views.
    trunk: bool,
    /// Consecutive frames saved a pose (`VOXEL_CRAFT_BURST`), and those
    /// saved of this one: still frames, to measure flicker.
    burst: u32,
    shots: u32,
    /// A flight instead of the poses (`VOXEL_CRAFT_FLIGHT`).
    flight: Option<Flight>,
    /// Sculpt strokes instead of the poses (`VOXEL_CRAFT_STROKE`), and this
    /// frame's world milliseconds they log.
    strokes: Option<crate::strokes::Strokes>,
    world_ms: f64,
}

/// `VOXEL_CRAFT_FLIGHT=<frames>` with `VOXEL_CRAFT_SHOTS`: a sideways flight
/// past glowstone, for ghosting (history that trails behind what moves on
/// screen). Glowstone goes up ahead of the spawn: a column on the ground,
/// and a block in the air in front of distant terrain. The camera settles at
/// the start, then moves `VOXEL_CRAFT_FLIGHT_SPEED` m/s (the fly speed) to
/// its right in steps of a 60 Hz frame, saving every frame as
/// `flight_NNN.png`; then it goes back to every eighth of those places and,
/// once still and converged there, saves `still_NNN.png`: what the frame
/// should look like. `tools/ghosting.py` compares them.
struct Flight {
    frames: u32,
    /// Metres a frame, and along which direction from where.
    step: f64,
    start: DVec3,
    along: DVec3,
    /// Frames flown so far; then the stills shot so far.
    flown: u32,
    stills: u32,
}

/// Flight frames between still references.
const FLIGHT_STILL_EVERY: u32 = 8;

/// (turn about up in degrees, pitch, third person, metres above the start).
const POSES: &[(f64, f64, bool, f64)] = &[
    (0.0, -0.08, false, 0.0),
    (90.0, 0.0, false, 0.0),
    (90.0, 0.55, false, 0.0),
    (90.0, -0.75, false, 0.0),
    (0.0, -0.2, true, 0.0),
    (90.0, -0.35, true, 0.0),
    (0.0, -0.55, false, 45.0),
    (180.0, 0.05, false, 0.0),
    // 450 m up, looking out: forests to the horizon (their far canopies).
    (30.0, -0.22, false, 400.0),
];

impl Tour {
    fn new(dir: String) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let burst = std::env::var("VOXEL_CRAFT_BURST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1u32)
            .max(1);
        let flight = std::env::var("VOXEL_CRAFT_FLIGHT")
            .ok()
            .and_then(|v| v.parse().ok())
            .map(|frames: u32| Flight {
                frames: frames.max(1),
                step: std::env::var("VOXEL_CRAFT_FLIGHT_SPEED")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(FLY_SPEED)
                    / 60.0,
                start: DVec3::ZERO,
                along: DVec3::ZERO,
                flown: 0,
                stills: 0,
            });
        Self {
            dir,
            pose: 0,
            waited: u32::MAX,
            trunk: std::env::var_os("VOXEL_CRAFT_TRUNK").is_some(),
            burst,
            shots: 0,
            flight,
            strokes: crate::strokes::Strokes::from_env(),
            world_ms: 0.0,
        }
    }

    /// The flight's frame: places the glowstone and the camera first, then
    /// flies, then shoots the stills; the path to save this frame to.
    fn fly(
        &mut self,
        player: &mut Player,
        world: &mut World,
        settled: bool,
        quit: &mut bool,
    ) -> Option<String> {
        let frames = |name: &str, default: u32| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let flight = self.flight.as_mut()?;
        if self.waited == u32::MAX {
            let frame = player.frame();
            let (f, r, u) = (frame.forward, frame.right, frame.up);
            flight.start = player.position + u * 1.0;
            flight.along = r;
            // The ground under a point (past the resident world: the
            // snapshot's, once), and blocks stacked on it.
            let mut edits = Vec::new();
            for (ahead, side, height) in [(7.0, -1.0, 3), (7.0, 5.0, 2), (12.0, 9.0, 1)] {
                let above = flight.start + f * ahead + r * side + u * 6.0;
                if let Some(hit) = world.planet().raycast(above, -u, 30.0) {
                    let base = world.cell_centre(hit.previous);
                    for k in 0..height {
                        edits.push(Edit::Build(
                            world.cell_at(base + u * (BLOCK_M * f64::from(k))),
                            Block::Glowstone,
                        ));
                    }
                }
            }
            edits.push(Edit::Build(
                world.cell_at(flight.start + f * 9.0 + r * 3.0 + u * 1.5),
                Block::Glowstone,
            ));
            world.edit(&edits);
            player.flying = true;
            player.third_person = false;
            player.pitch = -0.1;
            player.position = flight.start;
            player.velocity = DVec3::ZERO;
            self.waited = 0;
        }
        self.waited += 1;
        let at = |n: u32| flight.start + flight.along * (flight.step * f64::from(n));
        if flight.flown < flight.frames {
            let minimum = frames("VOXEL_CRAFT_WAIT", 120);
            if !((settled && self.waited >= minimum)
                || self.waited >= minimum.max(frames("VOXEL_CRAFT_WAIT_MAX", 900)))
            {
                return None;
            }
            player.position = at(flight.flown);
            flight.flown += 1;
            return Some(format!("{}/flight_{:03}.png", self.dir, flight.flown - 1));
        }
        // The stills: each a fresh pose that converges.
        let n = flight.stills * FLIGHT_STILL_EVERY;
        if n >= flight.frames {
            *quit = true;
            self.flight = None;
            self.pose = usize::MAX / 2;
            return None;
        }
        if player.position != at(n) {
            player.position = at(n);
            self.waited = 0;
            return None;
        }
        if self.waited < frames("VOXEL_CRAFT_STILL_WAIT", 90) {
            return None;
        }
        flight.stills += 1;
        Some(format!("{}/still_{:03}.png", self.dir, n))
    }

    /// Advances the tour by a frame; the path to save this frame to, if it
    /// should be.
    fn step(
        &mut self,
        player: &mut Player,
        world: &mut World,
        settled: bool,
        quit: &mut bool,
    ) -> Option<String> {
        if self.pose >= usize::MAX / 2 {
            return None;
        }
        if self.flight.is_some() {
            return self.fly(player, world, settled, quit);
        }
        if let Some(strokes) = &mut self.strokes {
            strokes.step(player, world, settled, self.world_ms, quit);
            return None;
        }
        if self.waited == u32::MAX {
            self.set_pose(player);
        }
        self.waited += 1;
        // A pose is shot once the terrain has settled and the temporal
        // anti-aliasing has had frames to converge (or after a long wait).
        let frames = |name: &str, default: u32| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let minimum = frames("VOXEL_CRAFT_WAIT", 120);
        let ready = (settled && self.waited >= minimum)
            || self.waited >= minimum.max(frames("VOXEL_CRAFT_WAIT_MAX", 900));
        if !ready {
            return None;
        }
        let path = if self.burst == 1 {
            format!("{}/shot_{:02}.png", self.dir, self.pose)
        } else {
            format!("{}/shot_{:02}_{:03}.png", self.dir, self.pose, self.shots)
        };
        log::info!(
            "shot {} at {} frames, settled {settled}",
            self.pose,
            self.waited
        );
        self.shots += 1;
        if self.shots < self.burst {
            return Some(path);
        }
        self.shots = 0;
        self.pose += 1;
        let limit = std::env::var("VOXEL_CRAFT_POSES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(POSES.len());
        if self.pose >= limit.min(POSES.len()) {
            *quit = true;
            self.pose = usize::MAX / 2;
        } else {
            // The next pose from the next frame: this one is shot as posed.
            self.waited = u32::MAX;
        }
        Some(path)
    }

    fn set_pose(&mut self, player: &mut Player) {
        let (turn, pitch, third, lift) = POSES[self.pose];
        let frame = player.frame();
        let (s, c) = turn.to_radians().sin_cos();
        player.heading = frame.forward * c + frame.right * s;
        player.pitch = pitch;
        player.third_person = third;
        player.flying = lift > 0.0;
        if lift > 0.0 && self.pose > 0 {
            player.position += frame.up * lift;
        }
        player.velocity = DVec3::ZERO;
        self.waited = 0;
    }
}

/// The nearest wood block within `reach` blocks of `from`, as (cell, block):
/// a search past the resident world, of the snapshot.
fn nearest_wood(world: &World, from: Cell, reach: i32) -> Option<(Cell, Block)> {
    let logs = [
        Block::OakLog,
        Block::BirchLog,
        Block::SpruceLog,
        Block::JungleLog,
    ];
    let planet = world.planet();
    let mut best: Option<(i32, Cell, Block)> = None;
    for di in -reach..=reach {
        for dj in -reach..=reach {
            for dk in 0..16 {
                let cell = Cell::new(from.face, from.i + di, from.j + dj, from.k + dk);
                let block = planet
                    .solid(cell)
                    .then(|| Block::from_material(planet.material(cell)))
                    .flatten();
                if let Some(block) = block.filter(|b| logs.contains(b)) {
                    let d = di * di + dj * dj + dk * dk;
                    if best.is_none_or(|(bd, _, _)| d < bd) {
                        best = Some((d, cell, block));
                    }
                }
            }
        }
    }
    best.map(|(_, c, b)| (c, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player left standing at the spawn stays exactly where it stands
    /// (hosts plan their views for an eye, and a view whose eye moves by a
    /// rounding step never settles).
    #[test]
    fn a_player_at_the_spawn_stands_still() {
        let planet = Planet::new(terrain::recipe::recipe()).unwrap();
        let spawn = find_spawn(&planet);
        let world = World::new(planet);
        // The world around the spawn, before the first step.
        world.follow(spawn);
        world.settle();
        let mut player = Player::new(spawn);
        let controls = crate::input::Controls::default();
        let mut rest = Vec::new();
        for step in 0..240 {
            player.update(&world, &controls, 1.0 / 60.0);
            if step >= 60 {
                rest.push((player.position, player.on_ground));
            }
        }
        assert!(
            rest.iter().all(|r| r.1 && r.0 == rest[0].0),
            "moved {:e} m",
            rest.iter()
                .map(|r| (r.0 - rest[0].0).length())
                .fold(0.0, f64::max)
        );
    }
}
