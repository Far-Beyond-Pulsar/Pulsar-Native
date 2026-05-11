use std::time::{Duration, Instant};

/// Snapshot of game time passed to every tick.
#[derive(Clone, Copy, Debug)]
pub struct GameTime {
    /// Wall-clock time since the world was created.
    pub elapsed: Duration,
    /// Time since the previous tick (capped at `max_delta` to prevent spiral-of-death).
    pub delta: Duration,
    /// Current fixed-tick counter (always 0 for variable-timestep ticks).
    pub tick: u64,
}

impl GameTime {
    #[inline]
    pub fn delta_secs(&self) -> f32 {
        self.delta.as_secs_f32()
    }

    #[inline]
    pub fn delta_secs_f64(&self) -> f64 {
        self.delta.as_secs_f64()
    }

    #[inline]
    pub fn elapsed_secs(&self) -> f32 {
        self.elapsed.as_secs_f32()
    }
}

/// Tracks wall-clock time and computes per-tick deltas.
pub(crate) struct Clock {
    start: Instant,
    last_tick: Instant,
    pub max_delta: Duration,
    pub tick_counter: u64,
}

impl Clock {
    pub fn new(max_delta: Duration) -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last_tick: now,
            max_delta,
            tick_counter: 0,
        }
    }

    pub fn tick(&mut self) -> GameTime {
        let now = Instant::now();
        let raw_delta = now.duration_since(self.last_tick);
        let delta = raw_delta.min(self.max_delta);
        let elapsed = now.duration_since(self.start);
        self.last_tick = now;
        let tick = self.tick_counter;
        self.tick_counter += 1;
        GameTime { elapsed, delta, tick }
    }

    pub fn reset(&mut self) {
        let now = Instant::now();
        self.start = now;
        self.last_tick = now;
        self.tick_counter = 0;
    }
}
