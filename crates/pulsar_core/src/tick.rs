use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum TickMode {
    Fixed { dt: Duration },
    Variable { max_delta: Duration },
}

impl Default for TickMode {
    fn default() -> Self {
        TickMode::Fixed {
            dt: Duration::from_secs_f64(1.0 / 60.0),
        }
    }
}
