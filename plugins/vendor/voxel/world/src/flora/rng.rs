//! A small deterministic random generator (xoshiro256** seeded by
//! SplitMix64): every tree is a pure function of its seed, on any platform.

#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut state = seed;
        Self {
            s: [
                splitmix(&mut state),
                splitmix(&mut state),
                splitmix(&mut state),
                splitmix(&mut state),
            ],
        }
    }

    pub fn u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [a, b].
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f64()
    }

    /// Uniform integer in [a, b] (both inclusive).
    pub fn int(&mut self, a: i32, b: i32) -> i32 {
        if b <= a {
            return a;
        }
        a + (self.u64() % (b - a + 1) as u64) as i32
    }

    /// One of `items`.
    pub fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.u64() as usize % items.len()]
    }
}
