//! Opt-in native integration diagnostic. No camera control in normal sessions.
use glam::{DVec3, Vec3};
use std::time::Instant;

const PROTOCOL: &str = "continuous_ground_v2";
const STRESS_PROTOCOL: &str = "continuous_surface_stress_v1";
const STRESS_MOVING_SECONDS: f64 = 300.0;
const STRESS_SETTLE_SECONDS: f64 = 10.0;

pub(super) struct NativeVoxelFlight {
    armed: bool,
    stress: bool,
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
            stress: std::env::var("PULSAR_VOXEL_NATIVE_FLIGHT_STRESS").is_ok_and(|v| v == "1"),
            armed_at: now, flight: None, last_report: now,
        }
    }

    pub fn force_frames(&self) -> bool { self.armed || self.flight.is_some() }
    pub fn running(&self) -> bool { self.flight.is_some() }

    pub fn advance(&mut self, now: Instant, ready: bool, interrupted: bool,
        eye: DVec3, clearance: Option<f64>, forward: Vec3, pitch: f32,
        mut surface_point: impl FnMut(DVec3, f64) -> Option<DVec3>) -> Option<Pose> {
        let protocol = if self.stress { STRESS_PROTOCOL } else { PROTOCOL };
        let duration = if self.stress { STRESS_MOVING_SECONDS + STRESS_SETTLE_SECONDS } else { 27.0 };
        let timeout = if self.stress { duration + 90.0 } else { 90.0 };
        if interrupted || now.duration_since(self.armed_at).as_secs_f64() > timeout {
            if self.force_frames() { tracing::info!("VOXEL_NATIVE_FLIGHT cancelled protocol={protocol}"); }
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
                tracing::info!("VOXEL_NATIVE_FLIGHT started protocol={protocol}");
            }
        }
        let flight = self.flight.as_ref()?;
        let t = now.duration_since(flight.started).as_secs_f64();
        if t > duration {
            self.flight = None;
            tracing::info!("VOXEL_NATIVE_FLIGHT complete protocol={protocol}");
            return None;
        }
        let (height, distance, down, stage) = if self.stress {
            stress_route(t, flight.clearance, flight.pitch)
        } else {
            route(t, flight.clearance, flight.pitch)
        };
        if now.duration_since(self.last_report).as_secs_f64() >= 0.5 {
            self.last_report = now;
            tracing::info!("VOXEL_NATIVE_FLIGHT time={t:.3} stage={stage} protocol={protocol}");
        }
        let direction = if self.stress {
            let angle = distance / (flight.eye.length() - flight.clearance);
            flight.eye.normalize() * angle.cos() + flight.tangent * angle.sin()
        } else {
            (flight.eye + flight.tangent * distance).normalize()
        };
        let eye = if self.stress || t >= 15.0 {
            // Follow the canonical surface at the changing direction. A
            // descent relative to the starting terrain can enter a distant
            // mountain and stop early at the production ground clamp.
            let Some(point) = surface_point(direction, height).filter(|point| point.is_finite()) else {
                self.flight = None;
                tracing::info!("VOXEL_NATIVE_FLIGHT cancelled protocol={protocol} reason=surface_unavailable");
                return None;
            };
            point
        } else {
            direction * (flight.eye.length() - flight.clearance + height)
        };
        Some(Pose { eye, pitch: down })
    }
}

// Five continuous moving cycles exercise surface turnover and repeated
// orbit/descent pressure. The final ten seconds expose reclamation after motion.
fn stress_route(t: f64, start: f64, pitch: f32) -> (f64, f64, f32, &'static str) {
    let moving_t = t.clamp(0.0, STRESS_MOVING_SECONDS);
    let cycles = (moving_t / 60.0).floor();
    let phase = moving_t % 60.0;
    let cycle_distance = 21.0 * 20_000.0 + 6.0 * (20_000.0 + 3_000.0) * 0.5 + 33.0 * 3_000.0;
    let partial_distance = if phase < 21.0 {
        phase * 20_000.0
    } else if phase < 27.0 {
        let arrival = phase - 21.0;
        21.0 * 20_000.0 + arrival * 20_000.0 - 0.5 * (17_000.0 / 6.0) * arrival * arrival
    } else {
        21.0 * 20_000.0 + 6.0 * 11_500.0 + (phase - 27.0) * 3_000.0
    };
    let distance = cycles * cycle_distance + partial_distance;
    if t >= STRESS_MOVING_SECONDS {
        (start.max(1.0), distance, pitch, "settle")
    } else if phase < 27.0 {
        let (height, _, down, stage) = route(phase, start, pitch);
        (height, distance, down, stage)
    } else {
        (start.max(1.0), distance, pitch, "surface")
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
        (blend(cruise, start, (t - 21.0) / 6.0), (t - 15.0) * 3_000.0, pitch, "arrival")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere_surface(direction: DVec3, clearance: f64) -> Option<DVec3> {
        Some(direction.normalize() * (6_371_700.0 + clearance))
    }
    #[test]
    fn diagnostic_waits_for_residency_and_cancels_without_rearming() {
        let now = Instant::now();
        let mut driver = NativeVoxelFlight {
            armed: true, stress: false, armed_at: now, flight: None, last_report: now,
        };
        let eye = DVec3::Y * 6_371_730.0;
        assert!(driver.advance(now, false, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_none());
        let pose = driver.advance(now, true, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).unwrap();
        assert!(pose.eye.distance(eye) < 1.0e-8);
        assert!(driver.advance(now, true, true, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_none());
        assert!(!driver.force_frames());
        assert!(driver.advance(now, true, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_none());
    }

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

    #[test]
    fn stress_route_is_continuous_across_cycles_and_stops_after_five_minutes() {
        for cycle in 0..5 {
            for phase in [6.0, 9.0, 15.0, 21.0, 27.0, 60.0] {
                let t = cycle as f64 * 60.0 + phase;
                let a = stress_route(t - 1.0e-8, 30.0, -0.15);
                let b = stress_route(t, 30.0, -0.15);
                assert!((a.0 - b.0).abs() < 0.02 && (a.1 - b.1).abs() < 0.001);
                assert!((a.2 - b.2).abs() < 1.0e-5);
            }
            let surface_t = cycle as f64 * 60.0 + 30.0;
            assert!((stress_route(surface_t + 1.0, 30.0, -0.15).1
                - stress_route(surface_t, 30.0, -0.15).1 - 3_000.0).abs() < 1.0e-8);
            assert!((stress_route(surface_t - 20.0 + 1.0, 30.0, -0.15).1
                - stress_route(surface_t - 20.0, 30.0, -0.15).1 - 20_000.0).abs() < 1.0e-8);
        }
        let stopped = stress_route(300.0, 30.0, -0.15);
        assert_eq!(stopped.0, 30.0);
        assert_eq!(stopped.1, 2_940_000.0);
        assert_eq!(stopped, stress_route(310.0, 30.0, -0.15));
    }

    #[test]
    fn stress_driver_stays_active_past_normal_timeout_and_has_bounded_settle() {
        let now = Instant::now();
        let eye = DVec3::Y * 6_371_730.0;
        let mut driver = NativeVoxelFlight {
            armed: true, stress: true, armed_at: now, flight: None, last_report: now,
        };
        assert!(driver.advance(now, true, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_some());
        let mut previous: Option<Pose> = None;
        for second in [94, 95, 299, 300, 310] {
            let pose = driver.advance(now + std::time::Duration::from_secs(second),
                false, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).unwrap();
            if second == 95 {
                let angle = previous.as_ref().unwrap().eye.normalize().angle_between(pose.eye.normalize());
                assert!((angle * 6_371_700.0 - 3_000.0).abs() < 1.0e-4);
            }
            if second == 310 {
                assert!(pose.eye.distance(previous.as_ref().unwrap().eye) < 1.0e-8);
            }
            previous = Some(pose);
        }
        assert!(driver.advance(now + std::time::Duration::from_millis(310_001),
            false, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_none());
        assert!(!driver.force_frames());
        // The stress switch cannot arm a normal session by itself.
        assert!(driver.advance(now, true, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_none());
    }


    #[test]
    fn arrival_keeps_moving_with_requested_clearance_over_uneven_ground() {
        let now = Instant::now();
        let ground_radius = |direction: DVec3| {
            6_371_700.0 + 400.0 + 250.0 * (direction.normalize().z * 1000.0).sin()
        };
        let surface = |direction: DVec3, clearance: f64| {
            Some(direction.normalize() * (ground_radius(direction) + clearance))
        };
        let eye = DVec3::Y * (ground_radius(DVec3::Y) + 30.0);
        let mut driver = NativeVoxelFlight {
            armed: true, stress: false, armed_at: now, flight: None, last_report: now,
        };
        let start = driver.advance(now, true, false, eye, Some(30.0), -Vec3::Z, -0.15, surface).unwrap();
        assert!(start.eye.distance(eye) < 1e-8);
        let mut previous: Option<Pose> = None;
        for second in 21..=27 {
            let pose = driver.advance(now + std::time::Duration::from_secs(second),
                false, false, eye, Some(30.0), -Vec3::Z, -0.15, surface).unwrap();
            let actual_clearance = pose.eye.length() - ground_radius(pose.eye);
            let requested_clearance = 1000.0_f64 * (30.0_f64 / 1000.0).powf((second - 21) as f64 / 6.0);
            assert!((actual_clearance - requested_clearance).abs() < 1e-7,
                "the new local ground must determine clearance at second{second}");
            if let Some(previous) = previous {
                let surface_travel = pose.eye.normalize().distance(previous.eye.normalize()) * 6_371_700.0;
                assert!(surface_travel > 2990.0 && surface_travel < 3010.0,
                    "arrival must keep translating rather than ground-clamping: {surface_travel}");
            }
            previous = Some(pose);
        }
        assert!(driver.advance(now + std::time::Duration::from_millis(27_001),
            false, false, eye, Some(30.0), -Vec3::Z, -0.15, surface).is_none());
        assert!(!driver.force_frames());
    }

    #[test]
    fn unavailable_canonical_surface_cancels_the_diagnostic() {
        let now = Instant::now();
        let eye = DVec3::Y * 6_371_730.0;
        let mut driver = NativeVoxelFlight {
            armed: true, stress: false, armed_at: now, flight: None, last_report: now,
        };
        assert!(driver.advance(now, true, false, eye, Some(30.0), -Vec3::Z, -0.15, sphere_surface).is_some());
        assert!(driver.advance(now + std::time::Duration::from_secs(15),
            false, false, eye, Some(30.0), -Vec3::Z, -0.15, |_, _| None).is_none());
        assert!(!driver.force_frames());
    }
}
