//! Streaming PLY reader (ascii, binary little/big endian) that returns only
//! vertex positions and triangulated faces; every other property is skipped.

use std::io::{BufRead, BufReader, Read};

pub struct PlyMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Format {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy, Debug)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(s: &str) -> Result<Ty, String> {
        Ok(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            other => return Err(format!("PLY: unknown type '{other}'")),
        })
    }
    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }
    fn decode(self, b: &[u8], big: bool) -> f64 {
        macro_rules! n {
            ($t:ty, $n:expr) => {{
                let a: [u8; $n] = b[..$n].try_into().unwrap();
                (if big {
                    <$t>::from_be_bytes(a)
                } else {
                    <$t>::from_le_bytes(a)
                }) as f64
            }};
        }
        match self {
            Ty::I8 => b[0] as i8 as f64,
            Ty::U8 => b[0] as f64,
            Ty::I16 => n!(i16, 2),
            Ty::U16 => n!(u16, 2),
            Ty::I32 => n!(i32, 4),
            Ty::U32 => n!(u32, 4),
            Ty::F32 => n!(f32, 4),
            Ty::F64 => n!(f64, 8),
        }
    }
}

#[derive(Debug)]
enum Prop {
    Scalar(String, Ty),
    List(String, Ty, Ty),
}

struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

fn header_line<R: BufRead>(r: &mut R) -> Result<String, String> {
    let mut line = Vec::new();
    r.read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
    if line.is_empty() {
        return Err("PLY: unexpected end of header".into());
    }
    Ok(String::from_utf8_lossy(&line).trim().to_string())
}

pub fn read_ply<R: Read>(reader: R) -> Result<PlyMesh, String> {
    let mut r = BufReader::with_capacity(1 << 20, reader);
    if header_line(&mut r)? != "ply" {
        return Err("not a PLY file".into());
    }
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    loop {
        let line = header_line(&mut r)?;
        let mut t = line.split_whitespace();
        match t.next() {
            Some("format") => {
                format = Some(match t.next() {
                    Some("ascii") => Format::Ascii,
                    Some("binary_little_endian") => Format::Little,
                    Some("binary_big_endian") => Format::Big,
                    other => return Err(format!("PLY: unsupported format {other:?}")),
                })
            }
            Some("element") => {
                let name = t.next().ok_or("PLY: element name")?.to_string();
                let count = t
                    .next()
                    .and_then(|c| c.parse().ok())
                    .ok_or("PLY: element count")?;
                elements.push(Element {
                    name,
                    count,
                    props: Vec::new(),
                });
            }
            Some("property") => {
                let el = elements.last_mut().ok_or("PLY: property before element")?;
                let first = t.next().ok_or("PLY: property")?;
                if first == "list" {
                    let c = Ty::parse(t.next().ok_or("PLY: list")?)?;
                    let i = Ty::parse(t.next().ok_or("PLY: list")?)?;
                    el.props
                        .push(Prop::List(t.next().ok_or("PLY: name")?.into(), c, i));
                } else {
                    let ty = Ty::parse(first)?;
                    el.props
                        .push(Prop::Scalar(t.next().ok_or("PLY: name")?.into(), ty));
                }
            }
            Some("end_header") => break,
            _ => {}
        }
    }
    let format = format.ok_or("PLY: missing format")?;
    let big = format == Format::Big;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for el in &elements {
        match el.name.as_str() {
            "vertex" => read_vertices(&mut r, el, format, big, &mut positions)?,
            "face" => read_faces(&mut r, el, format, big, positions.len(), &mut indices)?,
            _ => skip_element(&mut r, el, format, big)?,
        }
    }
    if positions.is_empty() || indices.is_empty() {
        return Err("PLY: no triangles".into());
    }
    Ok(PlyMesh { positions, indices })
}

fn ascii_line<R: BufRead>(r: &mut R, line: &mut String) -> Result<(), String> {
    line.clear();
    loop {
        if r.read_line(line).map_err(|e| e.to_string())? == 0 {
            return Err("PLY: truncated ascii data".into());
        }
        if !line.trim().is_empty() {
            return Ok(());
        }
        line.clear();
    }
}

fn read_vertices<R: BufRead>(
    r: &mut R,
    el: &Element,
    format: Format,
    big: bool,
    out: &mut Vec<[f32; 3]>,
) -> Result<(), String> {
    let find = |n: &str| {
        el.props
            .iter()
            .position(|p| matches!(p, Prop::Scalar(name, _) if name == n))
    };
    let (xi, yi, zi) = (
        find("x").ok_or("PLY: no x")?,
        find("y").ok_or("PLY: no y")?,
        find("z").ok_or("PLY: no z")?,
    );
    out.reserve(el.count);
    if format == Format::Ascii {
        let mut line = String::new();
        for _ in 0..el.count {
            ascii_line(r, &mut line)?;
            let v: Vec<f64> = line
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if v.len() <= xi.max(yi).max(zi) {
                return Err("PLY: short vertex line".into());
            }
            out.push([v[xi] as f32, v[yi] as f32, v[zi] as f32]);
        }
        return Ok(());
    }
    let mut offsets = Vec::new();
    let mut row = 0;
    for p in &el.props {
        match p {
            Prop::Scalar(_, ty) => {
                offsets.push((row, *ty));
                row += ty.size();
            }
            Prop::List(..) => return Err("PLY: list property in vertex element".into()),
        }
    }
    let mut buf = vec![0u8; row];
    for _ in 0..el.count {
        r.read_exact(&mut buf)
            .map_err(|e| format!("PLY vertices: {e}"))?;
        let g = |i: usize| offsets[i].1.decode(&buf[offsets[i].0..], big) as f32;
        out.push([g(xi), g(yi), g(zi)]);
    }
    Ok(())
}

fn emit(poly: &[u32], vertex_count: usize, out: &mut Vec<u32>) -> Result<(), String> {
    if poly.iter().any(|&i| i as usize >= vertex_count) {
        return Err("PLY: face index out of range".into());
    }
    for k in 1..poly.len().saturating_sub(1) {
        out.extend_from_slice(&[poly[0], poly[k], poly[k + 1]]);
    }
    Ok(())
}

fn read_faces<R: BufRead>(
    r: &mut R,
    el: &Element,
    format: Format,
    big: bool,
    vertex_count: usize,
    out: &mut Vec<u32>,
) -> Result<(), String> {
    let list_pos = el
        .props
        .iter()
        .position(
            |p| matches!(p, Prop::List(n, ..) if n == "vertex_indices" || n == "vertex_index"),
        )
        .ok_or("PLY: no vertex_indices")?;
    out.reserve(el.count * 3);
    let mut poly: Vec<u32> = Vec::new();
    if format == Format::Ascii {
        let mut line = String::new();
        for _ in 0..el.count {
            ascii_line(r, &mut line)?;
            let mut toks = line.split_whitespace();
            for (pi, p) in el.props.iter().enumerate() {
                match p {
                    Prop::Scalar(..) => {
                        toks.next();
                    }
                    Prop::List(..) => {
                        let n: usize = toks
                            .next()
                            .and_then(|t| t.parse().ok())
                            .ok_or("PLY: face count")?;
                        if pi == list_pos {
                            poly.clear();
                        }
                        for _ in 0..n {
                            let i: u32 = toks
                                .next()
                                .and_then(|t| t.parse().ok())
                                .ok_or("PLY: face index")?;
                            if pi == list_pos {
                                poly.push(i);
                            }
                        }
                    }
                }
            }
            emit(&poly, vertex_count, out)?;
        }
        return Ok(());
    }
    let mut b = [0u8; 8];
    for _ in 0..el.count {
        for (pi, p) in el.props.iter().enumerate() {
            match p {
                Prop::Scalar(_, ty) => {
                    r.read_exact(&mut b[..ty.size()])
                        .map_err(|e| format!("PLY faces: {e}"))?;
                }
                Prop::List(_, cty, ity) => {
                    r.read_exact(&mut b[..cty.size()])
                        .map_err(|e| format!("PLY faces: {e}"))?;
                    let n = cty.decode(&b, big) as usize;
                    if pi == list_pos {
                        poly.clear();
                    }
                    for _ in 0..n {
                        r.read_exact(&mut b[..ity.size()])
                            .map_err(|e| format!("PLY faces: {e}"))?;
                        if pi == list_pos {
                            poly.push(ity.decode(&b, big) as u32);
                        }
                    }
                }
            }
        }
        emit(&poly, vertex_count, out)?;
    }
    Ok(())
}

fn skip_element<R: BufRead>(
    r: &mut R,
    el: &Element,
    format: Format,
    big: bool,
) -> Result<(), String> {
    if format == Format::Ascii {
        let mut line = String::new();
        for _ in 0..el.count {
            ascii_line(r, &mut line)?;
        }
        return Ok(());
    }
    let mut b = [0u8; 8];
    let mut sink = vec![0u8; 4096];
    for _ in 0..el.count {
        for p in &el.props {
            match p {
                Prop::Scalar(_, ty) => r
                    .read_exact(&mut b[..ty.size()])
                    .map_err(|e| e.to_string())?,
                Prop::List(_, c, i) => {
                    r.read_exact(&mut b[..c.size()])
                        .map_err(|e| e.to_string())?;
                    let mut left = c.decode(&b, big) as usize * i.size();
                    while left > 0 {
                        let n = left.min(sink.len());
                        r.read_exact(&mut sink[..n]).map_err(|e| e.to_string())?;
                        left -= n;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_header(fmt: &str) -> String {
        format!(
            "ply\nformat {fmt} 1.0\ncomment test\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty uchar confidence\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n"
        )
    }

    const POS: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 2.]];

    #[test]
    fn ascii_quad_is_triangulated() {
        let mut s = quad_header("ascii");
        for p in POS {
            s += &format!("{} {} {} 9\n", p[0], p[1], p[2]);
        }
        s += "4 0 1 2 3\n";
        let m = read_ply(s.as_bytes()).unwrap();
        assert_eq!(m.positions, POS);
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
    }

    fn binary(big: bool) -> Vec<u8> {
        let mut d = quad_header(if big {
            "binary_big_endian"
        } else {
            "binary_little_endian"
        })
        .into_bytes();
        let f = |v: f32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        for p in POS {
            for c in p {
                d.extend(f(c));
            }
            d.push(7);
        }
        d.push(4);
        for i in [0i32, 1, 2, 3] {
            d.extend(if big {
                i.to_be_bytes()
            } else {
                i.to_le_bytes()
            });
        }
        d
    }

    #[test]
    fn binary_both_endians() {
        for big in [false, true] {
            let m = read_ply(&binary(big)[..]).unwrap();
            assert_eq!(m.positions, POS);
            assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
        }
    }

    #[test]
    fn truncated_and_bad_index_fail() {
        let mut d = binary(false);
        d.truncate(d.len() - 3);
        assert!(read_ply(&d[..]).is_err());
        let mut d = binary(false);
        let n = d.len();
        d[n - 4..].copy_from_slice(&99i32.to_le_bytes());
        assert!(read_ply(&d[..]).is_err());
    }
}
