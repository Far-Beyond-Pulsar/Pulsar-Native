//! Minimal MD5 (RFC 1321). Poly Haven publishes an MD5 for every file it
//! serves; this checks downloads against it without another dependency. It is
//! an integrity check against truncated or corrupted downloads, not a security
//! boundary (the transport is HTTPS and sizes are pinned too).

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn k(i: usize) -> u32 {
    // floor(2^32 * abs(sin(i + 1))), the RFC 1321 constants.
    ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32
}

pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    filled: usize,
    length: u64,
    table: [u32; 64],
}

impl Md5 {
    pub fn new() -> Self {
        let mut table = [0u32; 64];
        for (i, t) in table.iter_mut().enumerate() {
            *t = k(i);
        }
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buffer: [0; 64],
            filled: 0,
            length: 0,
            table,
        }
    }

    fn block(&mut self, chunk: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(self.table[i])
                .wrapping_add(m[g])
                .rotate_left(S[i]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                let chunk = self.buffer;
                self.block(&chunk);
                self.filled = 0;
            }
        }
        while data.len() >= 64 {
            let chunk: [u8; 64] = data[..64].try_into().unwrap();
            self.block(&chunk);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.filled = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let bits = self.length.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        let used = (self.length as usize + 1) % 64;
        pad.extend(std::iter::repeat(0u8).take((56 + 64 - used) % 64));
        pad.extend_from_slice(&bits.to_le_bytes());
        self.update(&pad);
        debug_assert_eq!(self.filled, 0);
        let mut out = [0u8; 16];
        for (i, s) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}

pub fn hex(data: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1321_test_vectors() {
        assert_eq!(hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(
            hex(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn chunked_updates_match_one_shot() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 31 % 251) as u8).collect();
        let mut h = Md5::new();
        for piece in data.chunks(77) {
            h.update(piece);
        }
        let chunked: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(chunked, hex(&data));
    }
}
