use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct GameTime {
    pub elapsed: Duration,
    pub delta: Duration,
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

pub struct Clock {
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
        GameTime {
            elapsed,
            delta,
            tick,
        }
    }

    pub fn reset(&mut self) {
        let now = Instant::now();
        self.start = now;
        self.last_tick = now;
        self.tick_counter = 0;
    }
}
