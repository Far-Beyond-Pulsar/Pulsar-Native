//! `.cube` LUT file parser (Adobe, DaVinci Resolve, Unreal Engine format).
//!
//! Supports 1D and 3D LUTs per the Cube LUT specification v1 and v2.
//! Returns flat RGBA16F texel data ready for GPU upload.

use std::fs;
use std::path::Path;

/// Errors that can occur during LUT parsing.
#[derive(Debug)]
pub enum LutError {
    Io(std::io::Error),
    Parse(String),
}

impl From<std::io::Error> for LutError {
    fn from(e: std::io::Error) -> Self {
        LutError::Io(e)
    }
}

/// A parsed 3D LUT ready for GPU upload.
#[derive(Debug, Clone)]
pub struct CubeLut {
    /// Size of each dimension (e.g. 33 for a 33³ LUT).
    pub size: u32,
    /// Domain metadata (optional): minimum input values.
    pub domain_min: [f32; 3],
    /// Domain metadata (optional): maximum input values.
    pub domain_max: [f32; 3],
    /// Flat texel data in RGBA16F format (half-floats).
    /// Length = size³ × 4.
    pub data: Vec<u16>,
}

impl CubeLut {
    /// Parse a `.cube` file from a byte slice.
    pub fn parse(bytes: &[u8]) -> Result<Self, LutError> {
        let s = std::str::from_utf8(bytes)
            .map_err(|e| LutError::Parse(format!("File is not valid UTF-8: {}", e)))?;

        let mut size: Option<u32> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut data_type_1d = false;
        let mut values: Vec<[f32; 3]> = Vec::new();

        for line in s.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let lower = trimmed.to_lowercase();

            if lower.starts_with("title ") {
                // Ignore title
                continue;
            }

            if lower.starts_with("lut_1d_size ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() < 2 {
                    return Err(LutError::Parse("Invalid LUT_1D_SIZE".into()));
                }
                size = Some(
                    parts[1]
                        .parse::<u32>()
                        .map_err(|_| LutError::Parse("Invalid LUT_1D_SIZE value".into()))?,
                );
                data_type_1d = true;
                continue;
            }

            if lower.starts_with("lut_3d_size ") || lower.starts_with("lut_3d_size\t") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() < 2 {
                    return Err(LutError::Parse("Invalid LUT_3D_SIZE".into()));
                }
                size = Some(
                    parts[1]
                        .parse::<u32>()
                        .map_err(|_| LutError::Parse("Invalid LUT_3D_SIZE value".into()))?,
                );
                data_type_1d = false;
                continue;
            }

            if lower.starts_with("domain_min ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 4 {
                    domain_min[0] = parts[1].parse().unwrap_or(0.0);
                    domain_min[1] = parts[2].parse().unwrap_or(0.0);
                    domain_min[2] = parts[3].parse().unwrap_or(0.0);
                }
                continue;
            }

            if lower.starts_with("domain_max ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 4 {
                    domain_max[0] = parts[1].parse().unwrap_or(1.0);
                    domain_max[1] = parts[2].parse().unwrap_or(1.0);
                    domain_max[2] = parts[3].parse().unwrap_or(1.0);
                }
                continue;
            }

            // Try to parse as data line (3 floats)
            if let Ok(v) = parse_rgb_line(trimmed) {
                values.push(v);
            }
        }

        let size = size.ok_or_else(|| LutError::Parse("No LUT_SIZE declaration found".into()))?;

        let expected_count = if data_type_1d {
            size as usize
        } else {
            (size * size * size) as usize
        };

        if values.len() != expected_count {
            return Err(LutError::Parse(format!(
                "Expected {} values but found {}",
                expected_count,
                values.len()
            )));
        }

        // Convert to RGBA16F half-float format
        let mut data = Vec::with_capacity(expected_count * 4);
        for v in &values {
            data.push(f32_to_f16(v[0]));
            data.push(f32_to_f16(v[1]));
            data.push(f32_to_f16(v[2]));
            data.push(f32_to_f16(1.0)); // alpha = 1
        }

        Ok(CubeLut {
            size,
            domain_min,
            domain_max,
            data,
        })
    }

    /// Load a `.cube` file from disk.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, LutError> {
        let bytes = fs::read(path.as_ref())?;
        Self::parse(&bytes)
    }
}

fn parse_rgb_line(s: &str) -> Result<[f32; 3], ()> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(());
    }
    // Check that all parts look like floats
    let r = parts[0].parse::<f32>().map_err(|_| ())?;
    let g = parts[1].parse::<f32>().map_err(|_| ())?;
    let b = parts[2].parse::<f32>().map_err(|_| ())?;
    Ok([r, g, b])
}

/// Convert an f32 to IEEE 754 binary16 (half-float).
fn f32_to_f16(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = (bits >> 16) & 0x8000u32;
    let orig_exp = ((bits >> 23) & 0xff) as i32;
    let exp = orig_exp - 127 + 15;
    let mant = bits & 0x007fffff;

    if orig_exp == 0 {
        // Zero or denormal
        let m = mant | 0x00800000;
        let shift = 113 - orig_exp; // 127 - 15 + 1 = 113
        let r = m >> shift.min(31);
        return (sign | r) as u16;
    }

    if orig_exp == 0xff {
        // Infinity or NaN
        if mant != 0 {
            return (sign | 0x7e00u32 | (mant >> 13)) as u16;
        }
        return (sign | 0x7c00u32) as u16;
    }

    if exp <= 0 {
        return sign as u16;
    }
    if exp >= 31 {
        return (sign | 0x7c00u32) as u16;
    }

    (sign | ((exp as u32) << 10) | (mant >> 13)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_2x2x2_lut() {
        let cube_data = b"TITLE \"Identity 2x2x2\"\nLUT_3D_SIZE 2\n0.0 0.0 0.0\n0.5 0.0 0.0\n0.0 0.5 0.0\n0.5 0.5 0.0\n0.0 0.0 0.5\n0.5 0.0 0.5\n0.0 0.5 0.5\n0.5 0.5 0.5\n";
        let lut = CubeLut::parse(cube_data).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.data.len(), 8 * 4);
        // First entry should be (0,0,0,1)
        assert_eq!(f32_to_f16(0.0), lut.data[0]);
        assert_eq!(f32_to_f16(0.0), lut.data[1]);
        assert_eq!(f32_to_f16(0.0), lut.data[2]);
        assert_eq!(f32_to_f16(1.0), lut.data[3]);
    }

    #[test]
    fn test_33_cube_from_file() {
        // Test with an inline small LUT
        let cube_data = b"LUT_3D_SIZE 2\n0.0 0.0 0.0\n1.0 0.0 0.0\n0.0 1.0 0.0\n1.0 1.0 0.0\n0.0 0.0 1.0\n1.0 0.0 1.0\n0.0 1.0 1.0\n1.0 1.0 1.0\n";
        let lut = CubeLut::parse(cube_data).unwrap();
        assert_eq!(lut.size, 2);
        // Last entry should be (1,1,1,1)
        assert_eq!(f32_to_f16(1.0), lut.data[28]);
        assert_eq!(f32_to_f16(1.0), lut.data[29]);
        assert_eq!(f32_to_f16(1.0), lut.data[30]);
        assert_eq!(f32_to_f16(1.0), lut.data[31]);
    }
}
