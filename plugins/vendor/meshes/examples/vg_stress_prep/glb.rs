//! Minimal binary glTF (GLB) writer for one indexed triangle mesh with
//! positions and normals, streamed to disk so a 28M-triangle scan never needs
//! a second in-memory copy.

use std::io::Write;

fn pad4(n: usize) -> usize {
    (4 - n % 4) % 4
}

fn write_f32x3<W: Write>(w: &mut W, data: &[[f32; 3]]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1 << 20);
    for chunk in data.chunks(1 << 16) {
        buf.clear();
        for v in chunk {
            for c in v {
                buf.extend_from_slice(&c.to_le_bytes());
            }
        }
        w.write_all(&buf)?;
    }
    Ok(())
}

fn write_u32<W: Write>(w: &mut W, data: &[u32]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1 << 20);
    for chunk in data.chunks(1 << 18) {
        buf.clear();
        for v in chunk {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&buf)?;
    }
    Ok(())
}

/// Write `positions`/`normals`/`indices` as a single-mesh GLB whose only
/// material is named `material`.
pub fn write_glb<W: Write>(
    w: &mut W,
    name: &str,
    material: &str,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    indices: &[u32],
) -> Result<(), String> {
    if positions.len() != normals.len() || indices.len() % 3 != 0 || positions.is_empty() {
        return Err("glb: inconsistent mesh".into());
    }
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let vbytes = positions.len() * 12;
    let ibytes = indices.len() * 4;
    let bin_len = vbytes * 2 + ibytes;
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"helio vg_stress provisioning\"}},",
            "\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"name\":\"{n}\",\"mesh\":0}}],",
            "\"materials\":[{{\"name\":\"{m}\",\"pbrMetallicRoughness\":{{\"baseColorFactor\":[0.85,0.82,0.75,1.0],\"metallicFactor\":0.0,\"roughnessFactor\":0.6}}}}],",
            "\"meshes\":[{{\"name\":\"{n}\",\"primitives\":[{{\"attributes\":{{\"POSITION\":0,\"NORMAL\":1}},\"indices\":2,\"material\":0,\"mode\":4}}]}}],",
            "\"accessors\":[",
            "{{\"bufferView\":0,\"componentType\":5126,\"count\":{vc},\"type\":\"VEC3\",\"min\":[{l0:e},{l1:e},{l2:e}],\"max\":[{h0:e},{h1:e},{h2:e}]}},",
            "{{\"bufferView\":1,\"componentType\":5126,\"count\":{vc},\"type\":\"VEC3\"}},",
            "{{\"bufferView\":2,\"componentType\":5125,\"count\":{ic},\"type\":\"SCALAR\"}}],",
            "\"bufferViews\":[",
            "{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{vb},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{vb},\"byteLength\":{vb},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{v2},\"byteLength\":{ib},\"target\":34963}}],",
            "\"buffers\":[{{\"byteLength\":{bl}}}]}}"
        ),
        n = name,
        m = material,
        vc = positions.len(),
        ic = indices.len(),
        vb = vbytes,
        v2 = vbytes * 2,
        ib = ibytes,
        bl = bin_len,
        l0 = lo[0],
        l1 = lo[1],
        l2 = lo[2],
        h0 = hi[0],
        h1 = hi[1],
        h2 = hi[2],
    );
    let json_pad = pad4(json.len());
    let bin_pad = pad4(bin_len);
    let json_chunk = json.len() + json_pad;
    let total = 12u64 + 8 + json_chunk as u64 + 8 + (bin_len + bin_pad) as u64;
    if total > u32::MAX as u64 {
        return Err("glb: mesh exceeds the 4 GiB GLB limit".into());
    }
    let io = |e: std::io::Error| e.to_string();
    w.write_all(b"glTF").map_err(io)?;
    w.write_all(&2u32.to_le_bytes()).map_err(io)?;
    w.write_all(&(total as u32).to_le_bytes()).map_err(io)?;
    w.write_all(&(json_chunk as u32).to_le_bytes())
        .map_err(io)?;
    w.write_all(b"JSON").map_err(io)?;
    w.write_all(json.as_bytes()).map_err(io)?;
    w.write_all(&vec![b' '; json_pad]).map_err(io)?;
    w.write_all(&((bin_len + bin_pad) as u32).to_le_bytes())
        .map_err(io)?;
    w.write_all(b"BIN\0").map_err(io)?;
    write_f32x3(w, positions).map_err(io)?;
    write_f32x3(w, normals).map_err(io)?;
    write_u32(w, indices).map_err(io)?;
    w.write_all(&vec![0u8; bin_pad]).map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glb_header_and_chunks_are_consistent() {
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let n = [[0.0, 0.0, 1.0]; 3];
        let mut out = Vec::new();
        write_glb(&mut out, "tri", "tri_stone", &p, &n, &[0, 1, 2]).unwrap();
        assert_eq!(&out[..4], b"glTF");
        assert_eq!(
            u32::from_le_bytes(out[8..12].try_into().unwrap()) as usize,
            out.len()
        );
        let jl = u32::from_le_bytes(out[12..16].try_into().unwrap()) as usize;
        assert_eq!(&out[16..20], b"JSON");
        let json: serde_json::Value = serde_json::from_slice(&out[20..20 + jl]).unwrap();
        assert_eq!(json["accessors"][0]["count"], 3);
        assert_eq!(json["materials"][0]["name"], "tri_stone");
        assert_eq!(json["buffers"][0]["byteLength"], 3 * 12 * 2 + 12);
        let bl = 20 + jl;
        assert_eq!(&out[bl + 4..bl + 8], b"BIN\0");
        assert_eq!(out.len() % 4, 0);
    }
}
