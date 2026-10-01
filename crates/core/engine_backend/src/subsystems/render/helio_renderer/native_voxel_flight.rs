//! Opt-in native integration diagnostic. No camera control in normal sessions.
use glam::{DVec3, Vec3};
use std::time::Instant;

pub(super) struct NativeVoxelFlight {
    armed: bool,
    armed_at: Instant,
    flight: Option<Flight>,
    last_report: Instant,
}

struct Flight {
    started: Instant,
    eye: DVec3,
    clearance: f64,
    tangent: DVec3,
    pitch: f32,
}

pub(super) struct Pose {
    pub eye: DVec3,
    pub pitch: f32,
}

impl NativeVoxelFlight {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            armed: std::env::var("PULSAR_VOXEL_NATIVE_FLIGHT").is_ok_and(|v| v == "1"),
            armed_at: now, flight: None, last_report: now,
        }
    }

    pub fn force_frames(&self) -> bool { self.armed || self.flight.is_some() }
    pub fn running(&self) -> bool { self.flight.is_some() }

    pub fn advance(&mut self, now: Instant, ready: bool, interrupted: bool,
        eye: DVec3, clearance: Option<f64>, forward: Vec3, pitch: f32) -> Option<Pose> {
        if interrupted || now.duration_since(self.armed_at).as_secs_f64() > 90.0 {
            if self.force_frames() { tracing::info!("VOXEL_NATIVE_FLIGHT cancelled"); }
            self.armed = false;
            self.flight = None;
            return None;
        }
        if self.armed && ready {
            if let Some(h) = clearance.filter(|h| h.is_finite() && *h > 0.0) {
                let up = eye.normalize();
                let f = forward.as_dvec3();
                let tangent = (f - up * f.dot(up)).try_normalize()?;
                self.flight = Some(Flight { started: now, eye, clearance: h, tangent, pitch });
                self.armed = false;
                tracing::info!("VOXEL_NATIVE_FLIGHT started");
            }
        }
        let flight = self.flight.as_ref()?;
        let t = now.duration_since(flight.started).as_secs_f64();
        if t > 27.0 {
            self.flight = None;
            tracing::info!("VOXEL_NATIVE_FLIGHT complete");
            return None;
        }
        let (height, distance, down, stage) = route(t, flight.clearance, flight.pitch);
        if now.duration_since(self.last_report).as_secs_f64() >= 0.5 {
            self.last_report = now;
            tracing::info!("VOXEL_NATIVE_FLIGHT time={t:.3} stage={stage}");
        }
        Some(Pose {
            eye: (flight.eye + flight.tangent * distance).normalize()
                * (flight.eye.length() - flight.clearance + height),
            pitch: down,
        })
    }
}

fn route(t: f64, start: f64, pitch: f32) -> (f64, f64, f32, &'static str) {
    let start = start.max(1.0);
    let orbit = start.max(300_000.0);
    let cruise = start.max(1_000.0);
    let blend = |a: f64, b: f64, v: f64| a * (b / a).powf(v.clamp(0.0, 1.0));
    if t < 6.0 {
        let v = t / 6.0;
        (blend(start, orbit, v), 0.0, pitch + (-1.2 - pitch) * v as f32, "ascent")
    } else if t < 9.0 {
        (orbit, 0.0, -1.2, "orbit")
    } else if t < 15.0 {
        let v = (t - 9.0) / 6.0;
        (blend(orbit, cruise, v), 0.0, -1.2 + (pitch + 1.2) * v as f32, "descent")
    } else if t < 21.0 {
        (cruise, (t - 15.0) * 3_000.0, pitch, "cruise")
    } else {
        (blend(cruise, start, (t - 21.0) / 6.0), 18_000.0, pitch, "arrival")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_is_continuous_and_returns_to_starting_clearance() {
        for t in [6.0, 9.0, 15.0, 21.0] {
            let a = route(t - 1.0e-8, 30.0, -0.15);
            let b = route(t, 30.0, -0.15);
            assert!((a.0 - b.0).abs() < 0.02 && (a.1 - b.1).abs() < 0.001);
            assert!((a.2 - b.2).abs() < 1.0e-5);
        }
        assert!((route(27.0, 30.0, -0.15).0 - 30.0).abs() < 1.0e-9);
    }
}
