//! IES LM-63-2002 light profile parser.
//!
//! Parses the IESNA standard format used by lighting manufacturers to describe
//! the angular intensity distribution of real-world luminaires. Produces a
//! 256×256 `R8Unorm` texture suitable for GPU sampling.

use std::fs;
use std::path::Path;

#[derive(Debug)]
pub enum IesError {
    Io(std::io::Error),
    Parse(String),
}

impl From<std::io::Error> for IesError {
    fn from(e: std::io::Error) -> Self {
        IesError::Io(e)
    }
}

/// A parsed IES light profile, ready for GPU upload as a 2D texture.
#[derive(Debug, Clone)]
pub struct IesProfile {
    /// Generated texture data: 256×256 R8Unorm (1 byte per texel).
    pub texture_data: Vec<u8>,
    /// Texture width (always 256).
    pub width: u32,
    /// Texture height (always 256).
    pub height: u32,
    /// Total lumens (from metadata, 0 if unavailable).
    pub lumens: f32,
    /// Candela multiplier.
    pub candela_mult: f32,
}

impl IesProfile {
    /// Parse an IES file and generate a 256×256 intensity texture.
    pub fn parse(bytes: &[u8]) -> Result<Self, IesError> {
        let s = std::str::from_utf8(bytes)
            .map_err(|e| IesError::Parse(format!("Not valid UTF-8: {}", e)))?;

        let lines: Vec<&str> = s.lines().collect();
        if lines.is_empty() {
            return Err(IesError::Parse("Empty file".into()));
        }

        // First line must start with IESNA
        if !lines[0].trim().starts_with("IESNA") {
            return Err(IesError::Parse("Missing IESNA header".into()));
        }

        let mut line_idx = 1;

        // Skip metadata lines (start with '[')
        while line_idx < lines.len() && lines[line_idx].trim().starts_with('[') {
            line_idx += 1;
        }

        // Skip TILT keyword block
        if line_idx < lines.len() && lines[line_idx].trim().to_uppercase() == "TILT=NONE" {
            line_idx += 1;
        } else if line_idx < lines.len()
            && lines[line_idx].trim().to_uppercase().starts_with("TILT=")
        {
            line_idx += 1;
            // TILT=INCLUDE or TILT=FILENAME — skip until next keyword or data
            while line_idx < lines.len() && !lines[line_idx].contains('=') {
                line_idx += 1;
            }
        }

        // Parse numeric data: number of lamps, lumens per lamp, candela multiplier,
        // number of vertical angles, number of horizontal angles,
        // photometric type (1=C, 2=B, 3=A), units (1=feet, 2=meters),
        // width/length/height, ballast factor, future use, input watts
        let mut nums = Vec::new();
        while line_idx < lines.len() {
            for token in lines[line_idx].split_whitespace() {
                if let Ok(v) = token.parse::<f32>() {
                    nums.push(v);
                }
            }
            if nums.len() >= 18 {
                // We have enough to start reading angles
                break;
            }
            line_idx += 1;
        }

        if nums.len() < 18 {
            return Err(IesError::Parse("Not enough numeric data".into()));
        }

        let n_lamps = nums[0] as u32;
        let lumens_per_lamp = nums[1];
        let candela_mult = nums[2];
        let n_v_angles = nums[3] as usize;
        let n_h_angles = nums[4] as usize;
        let _photometric_type = nums[5] as u32;
        let lumens = n_lamps as f32 * lumens_per_lamp;

        if n_v_angles == 0 || n_h_angles == 0 {
            return Err(IesError::Parse("Zero angle count".into()));
        }

        // Read vertical angles
        let mut v_angles = Vec::with_capacity(n_v_angles);
        while v_angles.len() < n_v_angles && line_idx < lines.len() {
            for token in lines[line_idx].split_whitespace() {
                if v_angles.len() < n_v_angles {
                    if let Ok(v) = token.parse::<f32>() {
                        v_angles.push(v);
                    }
                } else {
                    break;
                }
            }
            if v_angles.len() >= n_v_angles {
                line_idx += 1;
                break;
            }
            line_idx += 1;
        }

        // Read horizontal angles
        let mut h_angles = Vec::with_capacity(n_h_angles);
        while h_angles.len() < n_h_angles && line_idx < lines.len() {
            for token in lines[line_idx].split_whitespace() {
                if h_angles.len() < n_h_angles {
                    if let Ok(v) = token.parse::<f32>() {
                        h_angles.push(v);
                    }
                } else {
                    break;
                }
            }
            if h_angles.len() >= n_h_angles {
                line_idx += 1;
                break;
            }
            line_idx += 1;
        }

        // Read candela values (n_h_angles × n_v_angles)
        let expected = n_h_angles * n_v_angles;
        let mut candela = Vec::with_capacity(expected);
        while candela.len() < expected && line_idx < lines.len() {
            for token in lines[line_idx].split_whitespace() {
                if candela.len() < expected {
                    if let Ok(v) = token.parse::<f32>() {
                        candela.push(v);
                    }
                } else {
                    break;
                }
            }
            line_idx += 1;
        }

        if candela.len() != expected {
            return Err(IesError::Parse(format!(
                "Expected {} candela values, got {}",
                expected,
                candela.len()
            )));
        }

        // Generate 256×256 texture
        let tex = Self::generate_texture(
            &v_angles,
            &h_angles,
            &candela,
            n_h_angles,
            n_v_angles,
            candela_mult,
        );

        Ok(IesProfile {
            texture_data: tex,
            width: 256,
            height: 256,
            lumens,
            candela_mult,
        })
    }

    fn generate_texture(
        v_angles: &[f32],
        h_angles: &[f32],
        candela: &[f32],
        n_h: usize,
        n_v: usize,
        candela_mult: f32,
    ) -> Vec<u8> {
        let w = 256usize;
        let h = 256usize;
        let mut tex = vec![0u8; w * h];

        // Find max candela for normalisation
        let max_cdl = candela.iter().copied().fold(0.0f32, f32::max).max(1.0);

        for py in 0..h {
            for px in 0..w {
                // Map pixel to angle: U = horizontal (0-360°), V = vertical (0-180°)
                let h_angle = (px as f32 / w as f32) * 360.0;
                let v_angle = (py as f32 / h as f32) * 180.0;

                // Bilinear interpolate in the candela grid
                let intensity =
                    Self::interpolate_ies(h_angle, v_angle, h_angles, v_angles, candela, n_h, n_v)
                        * candela_mult;

                // Normalize to [0, 1] and quantize to u8
                let normalized = (intensity / max_cdl).clamp(0.0, 1.0);
                tex[py * w + px] = (normalized * 255.0) as u8;
            }
        }

        tex
    }

    fn interpolate_ies(
        h_angle: f32,
        v_angle: f32,
        h_angles: &[f32],
        v_angles: &[f32],
        candela: &[f32],
        n_h: usize,
        n_v: usize,
    ) -> f32 {
        // Wrap horizontal angle
        let h_angle = h_angle % 360.0;

        // Find horizontal segment
        let hi = Self::find_angle_index(h_angle, h_angles);
        let hi0 = hi.0.min(n_h - 1);
        let hi1 = hi.1.min(n_h - 1);
        let ht = hi.2;

        // Find vertical segment
        let vi = Self::find_angle_index(v_angle, v_angles);
        let vi0 = vi.0.min(n_v - 1);
        let vi1 = vi.1.min(n_v - 1);
        let vt = vi.2;

        // Bilinear interpolate
        let c00 = candela[hi0 * n_v + vi0];
        let c01 = candela[hi0 * n_v + vi1];
        let c10 = candela[hi1 * n_v + vi0];
        let c11 = candela[hi1 * n_v + vi1];

        let c0 = c00 * (1.0 - vt) + c01 * vt;
        let c1 = c10 * (1.0 - vt) + c11 * vt;
        c0 * (1.0 - ht) + c1 * ht
    }

    /// Binary search for the segment containing `angle`.
    /// Returns (lower_idx, upper_idx, fractional_t).
    fn find_angle_index(angle: f32, angles: &[f32]) -> (usize, usize, f32) {
        if angles.is_empty() {
            return (0, 0, 0.0);
        }
        if angle <= angles[0] {
            return (0, if angles.len() > 1 { 1 } else { 0 }, 0.0);
        }
        let last = angles.len() - 1;
        if angle >= angles[last] {
            return (last, last, 0.0);
        }

        let mut lo = 0usize;
        let mut hi = last;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if angles[mid] <= angle {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let range = angles[hi] - angles[lo];
        let t = if range > 0.0 {
            ((angle - angles[lo]) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (lo, hi, t)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, IesError> {
        let bytes = fs::read(path.as_ref())?;
        Self::parse(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_ies() {
        let data = b"IESNA:LM-63-2002\r\n[TEST] Minimal\r\nTILT=NONE\r\n1 1000.0 1.0 5 1 1 1 0 0 0 0 0 0 0 0 0 0 0\r\n0 15 30 45 90\r\n0\r\n100 200 300 400 500\r\n";
        let profile = IesProfile::parse(data).unwrap();
        assert_eq!(profile.width, 256);
        assert_eq!(profile.height, 256);
        assert!((profile.lumens - 1000.0).abs() < 0.1);
        assert_eq!(profile.texture_data.len(), 256 * 256);
    }

    #[test]
    fn test_find_angle() {
        let angles = vec![0.0, 15.0, 30.0, 45.0, 90.0];
        let (lo, hi, t) = IesProfile::find_angle_index(10.0, &angles);
        assert_eq!(lo, 0);
        assert_eq!(hi, 1);
        assert!((t - 10.0 / 15.0).abs() < 0.01);
    }
}
