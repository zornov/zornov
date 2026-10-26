// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! More model formats for [`crate::import`]: Stanford PLY (ASCII and binary)
//! and Autodesk FBX (binary, versions 6100 to 7700: geometry, UVs, and each
//! mesh placed by its node's translation, rotation and scale). Formats Studio
//! cannot take apart yet are named in [`unsupported`] so the message can say
//! what to export instead.

use crate::{Mesh, V3, ops};
use std::collections::HashMap;

/// Model formats other programs write that are not read here, with the way
/// out for each.
pub fn unsupported(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "usdz" | "usd" | "usda" | "usdc" => "USD / USDZ is not read yet; export it as .glb (Reality Converter and Apple's usdzconvert can)",
        "dae" => "COLLADA is not read yet; export it as .glb, .obj or .fbx",
        "3ds" | "blend" | "max" | "ma" | "mb" | "c4d" | "skp" | "abc" | "3mf" | "step" | "stp" | "iges" | "igs" | "off" | "x3d" | "wrl" | "vrml" | "lwo" | "fbx_ascii" => "that program's own format is not read; export it as .glb, .obj, .fbx, .stl or .ply",
        _ => return None,
    })
}

/// Whether `ext` is a 3D model format (read or not), for telling a model
/// from other files.
pub fn is_model_ext(ext: &str) -> bool {
    matches!(ext, "glb" | "gltf" | "obj" | "stl" | "ply" | "fbx" | "smesh") || unsupported(ext).is_some()
}

/// The model format `bytes` start like, as an extension, whatever the file
/// is called.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"glTF") {
        Some("glb")
    } else if bytes.starts_with(b"Kaydara FBX Binary") {
        Some("fbx")
    } else if bytes.starts_with(b"ply\n") || bytes.starts_with(b"ply\r\n") {
        Some("ply")
    } else if bytes.starts_with(b"SMESH") {
        None
    } else if bytes.len() > 84 && bytes.starts_with(b"solid") && bytes.windows(7).take(400).any(|w| w == b"facet n") {
        Some("stl")
    } else {
        None
    }
}

// ---------------------------------------------------------------- PLY ----

#[derive(Clone, Copy, PartialEq, Debug)]
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
    fn parse(s: &str) -> Option<Ty> {
        Some(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return None,
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
}

struct Prop {
    name: String,
    ty: Ty,
    /// The type of the count, for a list.
    list: Option<Ty>,
}

struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

/// A reader over the body: ASCII words, or binary values in either order.
struct Body<'a> {
    data: &'a [u8],
    at: usize,
    ascii: bool,
    big: bool,
}

impl Body<'_> {
    fn next(&mut self, ty: Ty) -> Result<f64, String> {
        const CUT: &str = "the PLY file ends before its data does";
        if self.ascii {
            let rest = &self.data[self.at..];
            let start = rest.iter().position(|b| !b.is_ascii_whitespace()).ok_or(CUT)?;
            let len = rest[start..].iter().position(|b| b.is_ascii_whitespace()).unwrap_or(rest.len() - start);
            self.at += start + len;
            let word = std::str::from_utf8(&rest[start..start + len]).map_err(|_| "the PLY file has bytes that are not text where text was expected")?;
            return word.parse::<f64>().map_err(|_| format!("the PLY file has \"{word}\" where a number was expected"));
        }
        let n = ty.size();
        let b = self.data.get(self.at..self.at + n).ok_or(CUT)?;
        self.at += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(b);
        if self.big {
            a[..n].reverse();
        }
        Ok(match ty {
            Ty::I8 => a[0] as i8 as f64,
            Ty::U8 => a[0] as f64,
            Ty::I16 => i16::from_le_bytes([a[0], a[1]]) as f64,
            Ty::U16 => u16::from_le_bytes([a[0], a[1]]) as f64,
            Ty::I32 => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::U32 => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::F32 => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::F64 => f64::from_le_bytes(a),
        })
    }
}

/// A Stanford PLY file: vertices (with normals, UVs and colours when it has
/// them) and polygon faces.
pub fn ply(bytes: &[u8]) -> Result<(Mesh, Vec<String>), String> {
    let end = bytes.windows(10).position(|w| w == b"end_header").ok_or("not a readable PLY file: no end_header line")?;
    let after = end + 10;
    let nl = bytes[after..].iter().position(|&b| b == b'\n').ok_or("not a readable PLY file: the header does not end")?;
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let mut format = "";
    let mut elements: Vec<Element> = Vec::new();
    for line in head.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["format", f, _] => format = f,
            ["element", name, n] => elements.push(Element { name: (*name).into(), count: n.parse().map_err(|_| format!("bad element count \"{n}\" in the PLY header"))?, props: Vec::new() }),
            ["property", "list", c, t, name] => {
                let e = elements.last_mut().ok_or("a PLY property comes before any element")?;
                e.props.push(Prop { name: (*name).into(), ty: Ty::parse(t).ok_or_else(|| format!("unknown PLY type \"{t}\""))?, list: Some(Ty::parse(c).ok_or_else(|| format!("unknown PLY type \"{c}\""))?) });
            }
            ["property", t, name] => {
                let e = elements.last_mut().ok_or("a PLY property comes before any element")?;
                e.props.push(Prop { name: (*name).into(), ty: Ty::parse(t).ok_or_else(|| format!("unknown PLY type \"{t}\""))?, list: None });
            }
            _ => {}
        }
    }
    let (ascii, big) = match format {
        "ascii" => (true, false),
        "binary_little_endian" => (false, false),
        "binary_big_endian" => (false, true),
        other => return Err(format!("PLY format \"{other}\" is not one of ascii, binary_little_endian, binary_big_endian")),
    };
    let mut body = Body { data: bytes, at: after + nl + 1, ascii, big };
    let mut mesh = Mesh::default();
    let mut notes = Vec::new();
    let (mut have_normals, mut have_uv) = (false, false);
    let mut colour = [0.0f64; 3];
    let mut coloured = 0usize;
    for el in &elements {
        match el.name.as_str() {
            "vertex" => {
                let at = |n: &[&str]| el.props.iter().position(|p| p.list.is_none() && n.contains(&p.name.as_str()));
                let (ix, iy, iz) = (at(&["x"]), at(&["y"]), at(&["z"]));
                let (Some(ix), Some(iy), Some(iz)) = (ix, iy, iz) else { return Err("the PLY vertices have no x, y, z".into()) };
                let nrm = (at(&["nx"]), at(&["ny"]), at(&["nz"]));
                let uv = (at(&["u", "s", "texture_u", "texture_s"]), at(&["v", "t", "texture_v", "texture_t"]));
                let rgb = (at(&["red", "r"]), at(&["green", "g"]), at(&["blue", "b"]));
                have_normals = nrm.0.is_some() && nrm.1.is_some() && nrm.2.is_some();
                have_uv = uv.0.is_some() && uv.1.is_some();
                let mut vals = vec![0f64; el.props.len()];
                for _ in 0..el.count {
                    for (i, p) in el.props.iter().enumerate() {
                        match p.list {
                            None => vals[i] = body.next(p.ty)?,
                            Some(ct) => {
                                let n = body.next(ct)? as usize;
                                for _ in 0..n {
                                    body.next(p.ty)?;
                                }
                            }
                        }
                    }
                    mesh.positions.push(from_y_up([vals[ix] as f32, vals[iy] as f32, vals[iz] as f32]));
                    if let (Some(a), Some(b), Some(c)) = nrm {
                        mesh.normals.push(from_y_up([vals[a] as f32, vals[b] as f32, vals[c] as f32]));
                    }
                    if let (Some(a), Some(b)) = uv {
                        mesh.uvs.push([vals[a] as f32, 1.0 - vals[b] as f32]);
                    }
                    if let (Some(a), Some(b), Some(c)) = rgb {
                        let scale = if el.props[a].ty == Ty::U8 { 255.0 } else { 1.0 };
                        for (k, i) in [a, b, c].into_iter().enumerate() {
                            colour[k] += vals[i] / scale;
                        }
                        coloured += 1;
                    }
                }
            }
            "face" => {
                for _ in 0..el.count {
                    for p in &el.props {
                        match p.list {
                            None => {
                                body.next(p.ty)?;
                            }
                            Some(ct) => {
                                let n = body.next(ct)? as usize;
                                let mut idx = Vec::with_capacity(n);
                                for _ in 0..n {
                                    idx.push(body.next(p.ty)? as i64);
                                }
                                if matches!(p.name.as_str(), "vertex_indices" | "vertex_index") {
                                    for k in 1..idx.len().saturating_sub(1) {
                                        for c in [idx[0], idx[k], idx[k + 1]] {
                                            if c < 0 || c as usize >= mesh.positions.len() {
                                                return Err(format!("a PLY face points at vertex {c}, but there are {}", mesh.positions.len()));
                                            }
                                            mesh.indices.push(c as u32);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                // Any other element (edges, materials...) is read past.
                for _ in 0..el.count {
                    for p in &el.props {
                        match p.list {
                            None => {
                                body.next(p.ty)?;
                            }
                            Some(ct) => {
                                let n = body.next(ct)? as usize;
                                for _ in 0..n {
                                    body.next(p.ty)?;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if mesh.indices.is_empty() {
        return Err(if mesh.positions.is_empty() { "the PLY file has no vertices".into() } else { format!("the PLY file is a point cloud ({} points, no faces), not a surface; make a mesh from it first (MeshLab can)", mesh.positions.len()) });
    }
    mesh.single_group();
    if coloured > 0 {
        let c = colour.map(|v| (v / coloured as f64) as f32);
        mesh.materials[0].base_colour = [c[0], c[1], c[2], 1.0];
        notes.push("per-vertex colours were averaged into one colour".into());
    }
    if !have_uv {
        mesh.uvs = vec![[0.0; 2]; mesh.positions.len()];
    }
    if !have_normals {
        mesh.normals = vec![[0.0; 3]; mesh.positions.len()];
        ops::recompute_normals(&mut mesh, 40.0);
    }
    Ok((mesh, notes))
}

fn from_y_up(p: V3) -> V3 {
    [p[0], -p[1], -p[2]]
}

fn from_z_up(p: V3) -> V3 {
    [p[0], -p[2], p[1]]
}

// ---------------------------------------------------------------- FBX ----

#[derive(Debug, Clone)]
enum P {
    Int(i64),
    Float(f64),
    Floats(Vec<f64>),
    Ints(Vec<i64>),
    Text(String),
    Raw,
}

#[derive(Debug, Default)]
struct Node {
    name: String,
    props: Vec<P>,
    kids: Vec<Node>,
}

impl Node {
    fn kid(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.name == name)
    }
    fn text(&self, i: usize) -> &str {
        match self.props.get(i) {
            Some(P::Text(s)) => s,
            _ => "",
        }
    }
    fn int(&self, i: usize) -> i64 {
        match self.props.get(i) {
            Some(P::Int(v)) => *v,
            Some(P::Float(v)) => *v as i64,
            _ => 0,
        }
    }
    fn floats(&self) -> Vec<f64> {
        match self.props.first() {
            Some(P::Floats(v)) => v.clone(),
            Some(P::Ints(v)) => v.iter().map(|&x| x as f64).collect(),
            _ => Vec::new(),
        }
    }
    fn ints(&self) -> Vec<i64> {
        match self.props.first() {
            Some(P::Ints(v)) => v.clone(),
            Some(P::Floats(v)) => v.iter().map(|&x| x as i64).collect(),
            _ => Vec::new(),
        }
    }
}

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
    wide: bool,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.b.get(self.at..self.at.checked_add(n).ok_or("the FBX file is damaged")?).ok_or("the FBX file ends before its data does")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn word(&mut self) -> Result<u64, String> {
        if self.wide { Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8"))) } else { Ok(self.u32()? as u64) }
    }
}

fn fbx_array(r: &mut Rd, kind: u8) -> Result<P, String> {
    let n = r.u32()? as usize;
    let enc = r.u32()?;
    let clen = r.u32()? as usize;
    let w = match kind {
        b'd' | b'l' => 8,
        b'f' | b'i' => 4,
        _ => 1,
    };
    let raw = r.take(clen)?;
    let data: Vec<u8> = if enc == 1 { studio_media::deflate::zlib_decompress(raw, n.saturating_mul(w).saturating_add(64)).map_err(|e| format!("an FBX array does not unpack: {e:?}"))? } else { raw.to_vec() };
    if data.len() < n * w {
        return Err("an FBX array is shorter than it says".into());
    }
    Ok(match kind {
        b'd' => P::Floats(data.chunks_exact(8).take(n).map(|c| f64::from_le_bytes(c.try_into().expect("8"))).collect()),
        b'f' => P::Floats(data.chunks_exact(4).take(n).map(|c| f32::from_le_bytes(c.try_into().expect("4")) as f64).collect()),
        b'l' => P::Ints(data.chunks_exact(8).take(n).map(|c| i64::from_le_bytes(c.try_into().expect("8"))).collect()),
        b'i' => P::Ints(data.chunks_exact(4).take(n).map(|c| i32::from_le_bytes(c.try_into().expect("4")) as i64).collect()),
        _ => P::Ints(data.iter().take(n).map(|&b| b as i64).collect()),
    })
}

fn fbx_node(r: &mut Rd, depth: usize) -> Result<Option<Node>, String> {
    if depth > 64 {
        return Err("the FBX file nests too deeply".into());
    }
    let end = r.word()? as usize;
    let nprops = r.word()? as usize;
    let _plen = r.word()?;
    let nlen = r.take(1)?[0] as usize;
    if end == 0 && nprops == 0 && nlen == 0 {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(r.take(nlen)?).into_owned();
    let mut node = Node { name, ..Node::default() };
    for _ in 0..nprops {
        let t = r.take(1)?[0];
        node.props.push(match t {
            b'Y' => P::Int(i16::from_le_bytes(r.take(2)?.try_into().expect("2")) as i64),
            b'C' => P::Int(r.take(1)?[0] as i64),
            b'I' => P::Int(i32::from_le_bytes(r.take(4)?.try_into().expect("4")) as i64),
            b'L' => P::Int(i64::from_le_bytes(r.take(8)?.try_into().expect("8"))),
            b'F' => P::Float(f32::from_le_bytes(r.take(4)?.try_into().expect("4")) as f64),
            b'D' => P::Float(f64::from_le_bytes(r.take(8)?.try_into().expect("8"))),
            b'S' => {
                let n = r.u32()? as usize;
                P::Text(String::from_utf8_lossy(r.take(n)?).into_owned())
            }
            b'R' => {
                let n = r.u32()? as usize;
                r.take(n)?;
                P::Raw
            }
            b'f' | b'd' | b'l' | b'i' | b'b' => fbx_array(r, t)?,
            other => return Err(format!("the FBX file has an unknown value type 0x{other:02x}")),
        });
    }
    while r.at < end {
        match fbx_node(r, depth + 1)? {
            Some(k) => node.kids.push(k),
            None => break,
        }
    }
    if end >= r.at && end <= r.b.len() {
        r.at = end;
    }
    Ok(Some(node))
}

type M4 = [[f64; 4]; 4];

fn identity() -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1.0 } else { 0.0 }))
}

fn mul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

fn rot(order: [usize; 3], deg: [f64; 3]) -> M4 {
    let one = |axis: usize, d: f64| -> M4 {
        let (s, c) = d.to_radians().sin_cos();
        let mut m = identity();
        let (i, j) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        m[i][i] = c;
        m[i][j] = -s;
        m[j][i] = s;
        m[j][j] = c;
        m
    };
    // Applied first to last along `order`: a point is turned by the first axis first.
    let mut m = identity();
    for &a in &order {
        m = mul(&one(a, deg[a]), &m);
    }
    m
}

/// A node's own placement (row vectors on the right: `p' = M * p`).
fn local_matrix(model: &Node) -> M4 {
    let mut t = [0.0; 3];
    let mut r = [0.0; 3];
    let mut pre = [0.0; 3];
    let mut s = [1.0; 3];
    let mut order = 0i64;
    if let Some(p70) = model.kid("Properties70") {
        for p in p70.kids.iter().filter(|k| k.name == "P") {
            let v = |i: usize| match p.props.get(4 + i) {
                Some(P::Float(f)) => *f,
                Some(P::Int(n)) => *n as f64,
                _ => 0.0,
            };
            let vec3 = [v(0), v(1), v(2)];
            match p.text(0) {
                "Lcl Translation" => t = vec3,
                "Lcl Rotation" => r = vec3,
                "PreRotation" => pre = vec3,
                "Lcl Scaling" => s = vec3,
                "RotationOrder" => order = v(0) as i64,
                _ => {}
            }
        }
    }
    let orders: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 2, 0], [1, 0, 2], [2, 0, 1], [2, 1, 0]];
    let ord = orders[(order.clamp(0, 5)) as usize];
    let mut sm = identity();
    for i in 0..3 {
        sm[i][i] = s[i];
    }
    let mut m = mul(&rot(ord, r), &sm);
    m = mul(&rot([0, 1, 2], pre), &m);
    for i in 0..3 {
        m[i][3] = t[i];
    }
    m
}

/// A binary FBX file: every mesh, each placed by its node chain.
pub fn fbx(bytes: &[u8]) -> Result<(Mesh, Vec<String>), String> {
    if !bytes.starts_with(b"Kaydara FBX Binary") {
        return Err(if bytes.starts_with(b"; FBX") { "this is an ASCII FBX file, which is not read; export it as binary FBX, or as .glb".into() } else { "not a readable FBX file".into() });
    }
    let version = u32::from_le_bytes(bytes.get(23..27).ok_or("the FBX file is cut short")?.try_into().expect("4"));
    let mut r = Rd { b: bytes, at: 27, wide: version >= 7500 };
    let mut top: Vec<Node> = Vec::new();
    while r.at + if r.wide { 25 } else { 13 } <= bytes.len() {
        match fbx_node(&mut r, 0)? {
            Some(n) => top.push(n),
            None => break,
        }
    }
    let objects = top.iter().find(|n| n.name == "Objects").ok_or("the FBX file has no objects")?;
    let up_z = top.iter().find(|n| n.name == "GlobalSettings").and_then(|g| g.kid("Properties70")).is_some_and(|p| p.kids.iter().any(|k| k.name == "P" && k.text(0) == "UpAxis" && matches!(k.props.get(4), Some(P::Int(2)))));
    // Parent of each object (child id -> parent id).
    let mut parent: HashMap<i64, i64> = HashMap::new();
    if let Some(c) = top.iter().find(|n| n.name == "Connections") {
        for k in c.kids.iter().filter(|k| k.name == "C" && k.text(0) == "OO") {
            parent.insert(k.int(1), k.int(2));
        }
    }
    let models: HashMap<i64, &Node> = objects.kids.iter().filter(|k| k.name == "Model").map(|k| (k.int(0), k)).collect();
    let world = |mut id: i64| -> M4 {
        let mut m = identity();
        let mut guard = 0;
        while let Some(model) = models.get(&id) {
            m = mul(&local_matrix(model), &m);
            match parent.get(&id) {
                Some(&p) => id = p,
                None => break,
            }
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        m
    };
    let mut mesh = Mesh::default();
    let mut notes = Vec::new();
    let mut parts = 0;
    for g in objects.kids.iter().filter(|k| k.name == "Geometry" && k.text(2) == "Mesh") {
        let verts = g.kid("Vertices").map(Node::floats).unwrap_or_default();
        let poly = g.kid("PolygonVertexIndex").map(Node::ints).unwrap_or_default();
        if verts.len() < 9 || poly.len() < 3 {
            continue;
        }
        let m = world(parent.get(&g.int(0)).copied().unwrap_or(-1));
        let place = |v: usize| -> V3 {
            let (x, y, z) = (verts[v * 3], verts[v * 3 + 1], verts[v * 3 + 2]);
            let q: [f64; 3] = std::array::from_fn(|i| m[i][0] * x + m[i][1] * y + m[i][2] * z + m[i][3]);
            let q = [q[0] as f32, q[1] as f32, q[2] as f32];
            if up_z { from_z_up(q) } else { from_y_up(q) }
        };
        // UVs: per corner, or per control point.
        let layer = g.kid("LayerElementUV");
        let uv = layer.and_then(|l| l.kid("UV")).map(Node::floats).unwrap_or_default();
        let uv_index = layer.and_then(|l| l.kid("UVIndex")).map(Node::ints).unwrap_or_default();
        let by_corner = layer.and_then(|l| l.kid("MappingInformationType")).is_some_and(|k| k.text(0) == "ByPolygonVertex");
        let uv_at = |corner: usize, ctrl: usize| -> [f32; 2] {
            let slot = if by_corner { corner } else { ctrl };
            let i = if uv_index.is_empty() { slot } else { uv_index.get(slot).map_or(usize::MAX, |&v| v.max(0) as usize) };
            match (uv.get(i * 2), uv.get(i * 2 + 1)) {
                (Some(&u), Some(&v)) => [u as f32, 1.0 - v as f32],
                _ => [0.0; 2],
            }
        };
        let base = mesh.positions.len() as u32;
        let mut seen: HashMap<(usize, [u32; 2]), u32> = HashMap::new();
        let mut poly_corners: Vec<u32> = Vec::new();
        for (corner, &raw) in poly.iter().enumerate() {
            let ctrl = if raw < 0 { !raw } else { raw } as usize;
            if ctrl * 3 + 2 >= verts.len() {
                return Err(format!("an FBX face points at vertex {ctrl}, but the mesh has {}", verts.len() / 3));
            }
            let t = uv_at(corner, ctrl);
            let key = (ctrl, [t[0].to_bits(), t[1].to_bits()]);
            let vi = *seen.entry(key).or_insert_with(|| {
                mesh.positions.push(place(ctrl));
                mesh.uvs.push(t);
                mesh.positions.len() as u32 - 1
            });
            poly_corners.push(vi);
            if raw < 0 {
                for k in 1..poly_corners.len().saturating_sub(1) {
                    mesh.indices.extend_from_slice(&[poly_corners[0], poly_corners[k], poly_corners[k + 1]]);
                }
                poly_corners.clear();
            }
        }
        if mesh.positions.len() as u32 > base {
            parts += 1;
        }
    }
    if mesh.indices.is_empty() {
        return Err("the FBX file has no polygon meshes".into());
    }
    mesh.normals = vec![[0.0; 3]; mesh.positions.len()];
    mesh.single_group();
    ops::recompute_normals(&mut mesh, 40.0);
    notes.push(format!("FBX read as geometry and UVs from {parts} mesh(es); its materials, textures, skeleton and animation are not read"));
    Ok((mesh, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii_ply() -> Vec<u8> {
        b"ply\nformat ascii 1.0\ncomment made by a test\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255 0 0\n1 0 0 255 0 0\n1 1 0 255 0 0\n0 1 0 255 0 0\n4 0 1 2 3\n".to_vec()
    }

    #[test]
    fn ply_ascii_quad_becomes_two_triangles() {
        let (m, notes) = ply(&ascii_ply()).unwrap();
        assert_eq!((m.positions.len(), m.triangles()), (4, 2));
        assert!((m.materials[0].base_colour[0] - 1.0).abs() < 1e-6 && m.materials[0].base_colour[1] < 1e-6);
        assert_eq!(notes.len(), 1);
        assert_eq!(sniff(&ascii_ply()), Some("ply"));
    }

    #[test]
    fn ply_binary_both_orders() {
        for big in [false, true] {
            let mut b = format!("ply\nformat binary_{}_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar uint vertex_indices\nend_header\n", if big { "big" } else { "little" }).into_bytes();
            for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
                b.extend_from_slice(&if big { v.to_be_bytes() } else { v.to_le_bytes() });
            }
            b.push(3);
            for i in [0u32, 1, 2] {
                b.extend_from_slice(&if big { i.to_be_bytes() } else { i.to_le_bytes() });
            }
            let (m, _) = ply(&b).unwrap();
            assert_eq!(m.triangles(), 1, "big={big}");
            assert!(m.positions.iter().any(|p| p[0] == 1.0));
        }
    }

    #[test]
    fn ply_point_cloud_and_cut_file_are_explained() {
        let cloud = b"ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 1 1\n";
        assert!(ply(cloud).unwrap_err().contains("point cloud"));
        let cut = &ascii_ply()[..ascii_ply().len() - 12];
        assert!(ply(cut).unwrap_err().contains("ends before"), "{:?}", ply(cut).err());
    }

    /// Writes a minimal binary FBX (version 7400) with one triangle mesh under
    /// a model translated by (10, 0, 0) and scaled by 2.
    pub fn tiny_fbx() -> Vec<u8> {
        fn node(name: &str, props: Vec<Vec<u8>>, kids: Vec<Vec<u8>>, base: usize) -> Vec<u8> {
            let plist: Vec<u8> = props.concat();
            let head = 13 + name.len();
            let mut kid_bytes = Vec::new();
            for k in &kids {
                kid_bytes.extend_from_slice(k);
            }
            let has_kids = !kids.is_empty();
            let total = head + plist.len() + kid_bytes.len() + if has_kids { 13 } else { 0 };
            let mut out = Vec::new();
            out.extend_from_slice(&((base + total) as u32).to_le_bytes());
            out.extend_from_slice(&(props.len() as u32).to_le_bytes());
            out.extend_from_slice(&(plist.len() as u32).to_le_bytes());
            out.push(name.len() as u8);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&plist);
            out.extend_from_slice(&kid_bytes);
            if has_kids {
                out.extend_from_slice(&[0; 13]);
            }
            out
        }
        // Offsets inside a record are absolute, so children are built after
        // their place is known: build bottom-up with a two-pass trick (sizes
        // do not depend on the base, only the stored end offsets do).
        fn build(name: &str, props: &[Vec<u8>], kids: &dyn Fn(usize) -> Vec<Vec<u8>>, base: usize) -> Vec<u8> {
            let plist_len: usize = props.iter().map(Vec::len).sum();
            let inner_base = base + 13 + name.len() + plist_len;
            let k = kids(inner_base);
            node(name, props.to_vec(), k, base)
        }
        let s = |t: &str| {
            let mut v = vec![b'S'];
            v.extend_from_slice(&(t.len() as u32).to_le_bytes());
            v.extend_from_slice(t.as_bytes());
            v
        };
        let l = |n: i64| {
            let mut v = vec![b'L'];
            v.extend_from_slice(&n.to_le_bytes());
            v
        };
        let d = |x: f64| {
            let mut v = vec![b'D'];
            v.extend_from_slice(&x.to_le_bytes());
            v
        };
        let darr = |xs: &[f64]| {
            let mut v = vec![b'd'];
            v.extend_from_slice(&(xs.len() as u32).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&((xs.len() * 8) as u32).to_le_bytes());
            for x in xs {
                v.extend_from_slice(&x.to_le_bytes());
            }
            v
        };
        let iarr = |xs: &[i32]| {
            let mut v = vec![b'i'];
            v.extend_from_slice(&(xs.len() as u32).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&((xs.len() * 4) as u32).to_le_bytes());
            for x in xs {
                v.extend_from_slice(&x.to_le_bytes());
            }
            v
        };
        let mut file = b"Kaydara FBX Binary  \0\x1a\0".to_vec();
        file.extend_from_slice(&7400u32.to_le_bytes());
        let objects = build("Objects", &[], &|b| {
            let geo = build("Geometry", &[l(100), s("tri\0\x01Geometry"), s("Mesh")], &|b2| {
                let v = build("Vertices", &[darr(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0])], &|_| vec![], b2);
                let p = build("PolygonVertexIndex", &[iarr(&[0, 1, -3])], &|_| vec![], b2 + v.len());
                vec![v, p]
            }, b);
            let model = build("Model", &[l(200), s("m\0\x01Model"), s("Mesh")], &|b2| {
                let p70 = build("Properties70", &[], &|b3| {
                    let t = build("P", &[s("Lcl Translation"), s("Lcl Translation"), s(""), s("A"), d(10.0), d(0.0), d(0.0)], &|_| vec![], b3);
                    let sc = build("P", &[s("Lcl Scaling"), s("Lcl Scaling"), s(""), s("A"), d(2.0), d(2.0), d(2.0)], &|_| vec![], b3 + t.len());
                    vec![t, sc]
                }, b2);
                vec![p70]
            }, b + geo.len());
            vec![geo, model]
        }, file.len());
        file.extend_from_slice(&objects);
        let conns = build("Connections", &[], &|b| vec![build("C", &[s("OO"), l(100), l(200)], &|_| vec![], b), ], file.len());
        file.extend_from_slice(&conns);
        file.extend_from_slice(&[0; 13]);
        file
    }

    #[test]
    fn fbx_binary_mesh_is_placed_by_its_model() {
        let (m, notes) = fbx(&tiny_fbx()).unwrap();
        assert_eq!(m.triangles(), 1);
        // x: 0..1 scaled by 2 then moved by 10 -> 10..12.
        let (lo, hi) = m.bounds();
        assert!((lo[0] - 10.0).abs() < 1e-4 && (hi[0] - 12.0).abs() < 1e-4, "{lo:?} {hi:?}");
        assert!(notes[0].contains("1 mesh"));
        assert_eq!(sniff(&tiny_fbx()), Some("fbx"));
    }

    #[test]
    fn fbx_that_is_ascii_or_cut_is_explained() {
        assert!(fbx(b"; FBX 7.3.0 project file\n").unwrap_err().contains("ASCII"));
        let f = tiny_fbx();
        assert!(fbx(&f[..f.len() / 2]).is_err());
    }

    #[test]
    fn unsupported_formats_say_what_to_export() {
        assert!(unsupported("usdz").unwrap().contains(".glb"));
        assert!(is_model_ext("blend") && is_model_ext("fbx") && !is_model_ext("mp4"));
    }
}
