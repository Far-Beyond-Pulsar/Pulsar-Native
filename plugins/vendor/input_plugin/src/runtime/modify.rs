//! Applying modifiers to a value.

use crate::model::behavior::{magnitude, Curve, Modifier, Value3};

/// A modifier chain and the state some of its modifiers keep between frames.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    mods: Vec<Modifier>,
    /// Where each smoothing modifier has got to.
    smoothed: Vec<Value3>,
}

impl Chain {
    pub fn new(mods: Vec<Modifier>) -> Self {
        let smoothed = vec![[0.0; 3]; mods.len()];
        Self { mods, smoothed }
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    /// Run `value` through every modifier in order.
    pub fn apply(&mut self, mut value: Value3, dt: f32) -> Value3 {
        for (modifier, state) in self.mods.iter().zip(self.smoothed.iter_mut()) {
            value = apply_one(modifier, value, dt, state);
        }
        value
    }
}

fn map(value: Value3, f: impl Fn(usize, f32) -> f32) -> Value3 {
    [f(0, value[0]), f(1, value[1]), f(2, value[2])]
}

fn apply_one(modifier: &Modifier, value: Value3, dt: f32, smoothed: &mut Value3) -> Value3 {
    match *modifier {
        Modifier::DeadZone { lower, upper, radial } => {
            let upper = upper.max(lower + f32::EPSILON);
            let rescale = |m: f32| ((m.min(upper) - lower) / (upper - lower)).clamp(0.0, 1.0);
            if radial {
                let m = magnitude(value);
                if m <= lower {
                    [0.0; 3]
                } else {
                    let k = rescale(m) / m;
                    map(value, |_, v| v * k)
                }
            } else {
                map(value, |_, v| {
                    let m = v.abs();
                    if m <= lower {
                        0.0
                    } else {
                        rescale(m) * v.signum()
                    }
                })
            }
        }
        Modifier::Scale { x, y, z } => [value[0] * x, value[1] * y, value[2] * z],
        Modifier::Negate { x, y, z } => [if x { -value[0] } else { value[0] }, if y { -value[1] } else { value[1] }, if z { -value[2] } else { value[2] }],
        Modifier::Swizzle { order } => {
            let o = order.order();
            [value[o[0]], value[o[1]], value[o[2]]]
        }
        Modifier::Smooth { speed } => {
            // Exponential approach: independent of the frame rate.
            let k = 1.0 - (-speed.max(0.0) * dt).exp();
            *smoothed = map(*smoothed, |i, s| s + (value[i] - s) * k);
            *smoothed
        }
        Modifier::ResponseCurve { curve, exponent } => map(value, |_, v| {
            let m = v.abs();
            let shaped = match curve {
                Curve::Exponential => m.powf(exponent.max(0.0)),
                Curve::Quadratic => m * m,
                Curve::Cubic => m * m * m,
                Curve::SmoothStep => {
                    let t = m.clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                }
            };
            shaped * v.signum()
        }),
        Modifier::Clamp { min, max } => map(value, |_, v| v.clamp(min.min(max), max.max(min))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::behavior::Swizzle;

    fn run(m: Modifier, v: Value3) -> Value3 {
        Chain::new(vec![m]).apply(v, 0.016)
    }

    fn near(a: Value3, b: Value3) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-4, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_radial_dead_zone_drops_drift_and_rescales_the_rest() {
        let dz = Modifier::DeadZone { lower: 0.2, upper: 1.0, radial: true };
        near(run(dz.clone(), [0.1, 0.1, 0.0]), [0.0; 3]);
        near(run(dz.clone(), [1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        near(run(dz.clone(), [0.6, 0.0, 0.0]), [0.5, 0.0, 0.0]);
        let diagonal = run(dz, [0.7, 0.7, 0.0]);
        assert!(diagonal[0] > 0.0 && (diagonal[0] - diagonal[1]).abs() < 1e-5, "direction is kept");
    }

    #[test]
    fn an_axial_dead_zone_judges_each_axis_alone() {
        let dz = Modifier::DeadZone { lower: 0.2, upper: 1.0, radial: false };
        near(run(dz, [0.1, -0.6, 0.0]), [0.0, -0.5, 0.0]);
    }

    #[test]
    fn scale_negate_and_clamp_work_per_component() {
        near(run(Modifier::Scale { x: 2.0, y: 0.5, z: 1.0 }, [1.0, 1.0, 1.0]), [2.0, 0.5, 1.0]);
        near(run(Modifier::Negate { x: true, y: false, z: true }, [1.0, 1.0, 1.0]), [-1.0, 1.0, -1.0]);
        near(run(Modifier::Clamp { min: -0.5, max: 0.5 }, [2.0, -2.0, 0.1]), [0.5, -0.5, 0.1]);
    }

    #[test]
    fn swizzle_sends_a_keys_x_to_y() {
        near(run(Modifier::Swizzle { order: Swizzle::Yxz }, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        near(run(Modifier::Swizzle { order: Swizzle::Zyx }, [1.0, 2.0, 3.0]), [3.0, 2.0, 1.0]);
    }

    #[test]
    fn modifiers_run_in_order() {
        let mut chain = Chain::new(vec![Modifier::Swizzle { order: Swizzle::Yxz }, Modifier::Negate { x: false, y: true, z: false }]);
        near(chain.apply([1.0, 0.0, 0.0], 0.016), [0.0, -1.0, 0.0]);
    }

    #[test]
    fn response_curves_keep_the_sign_and_the_ends() {
        let q = Modifier::ResponseCurve { curve: Curve::Quadratic, exponent: 2.0 };
        near(run(q.clone(), [-0.5, 1.0, 0.0]), [-0.25, 1.0, 0.0]);
        let e = Modifier::ResponseCurve { curve: Curve::Exponential, exponent: 3.0 };
        near(run(e, [0.5, 0.0, 0.0]), [0.125, 0.0, 0.0]);
        let s = Modifier::ResponseCurve { curve: Curve::SmoothStep, exponent: 0.0 };
        near(run(s, [0.5, 0.0, 0.0]), [0.5, 0.0, 0.0]);
    }

    #[test]
    fn smoothing_approaches_the_target_at_a_rate_that_ignores_frame_rate() {
        let m = Modifier::Smooth { speed: 10.0 };
        let mut fast = Chain::new(vec![m.clone()]);
        let mut slow = Chain::new(vec![m]);
        let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
        for _ in 0..100 {
            a = fast.apply([1.0, 0.0, 0.0], 0.01);
        }
        for _ in 0..10 {
            b = slow.apply([1.0, 0.0, 0.0], 0.1);
        }
        assert!((a[0] - b[0]).abs() < 1e-3, "after one second both agree: {a:?} {b:?}");
        assert!(a[0] > 0.9 && a[0] < 1.0);
    }
}
