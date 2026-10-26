// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Interchange: assets out to USD (`.usda`, `.usdz` for AR), FBX (binary
//! 7.4, what game engines and other 3D packages take), OBJ with its quads
//! and n-gons, STL (printing), PLY and glTF; and FBX, USD text and PLY in.
//! Each part goes out as its own mesh with its faces as they are (quads
//! stay quads), its materials and their textures.
//!
//! Studio's world has y down and z away; the files' worlds have y up, so
//! points turn half a turn about x on the way out and back on the way in,
//! and texture v runs up.

use crate::asset::{Asset, OutPart};
use crate::poly::Poly;
use std::fmt::Write as _;
use std::path::Path;
use studio_mesh::{Mesh, V3};

/// Studio → y-up.
fn out_p(p: V3) -> V3 {
    [p[0], -p[1], -p[2]]
}

fn png(t: &studio_mesh::Texture) -> Vec<u8> {
    studio_media::png::encode(&studio_media::RgbaImage { width: t.width, height: t.height, pixels: t.rgba.clone() })
}

fn safe(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if s.is_empty() || s.as_bytes()[0].is_ascii_digit() {
        s.insert(0, '_');
    }
    s
}

fn srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Writes an asset (and its drawn triangles) to `path` by its extension.
pub fn write(mesh: &Mesh, asset: &Asset, path: &Path) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let w = |bytes: &[u8], p: &Path| std::fs::write(p, bytes).map_err(|e| format!("{}: {e}", p.display()));
    match ext.as_str() {
        "glb" => w(&studio_mesh::export::glb(mesh), path),
        "stl" => w(&stl(mesh), path),
        "ply" => w(ply(mesh).as_bytes(), path),
        "obj" => {
            let parts = asset.output_parts(false)?;
            let mtl = path.with_extension("mtl");
            let mtl_name = mtl.file_name().and_then(|n| n.to_str()).unwrap_or("model.mtl").to_owned();
            let (o, m, pics) = obj(&parts, &mtl_name);
            w(o.as_bytes(), path)?;
            w(m.as_bytes(), &mtl)?;
            for (file, bytes) in pics {
                w(&bytes, &path.with_file_name(file))?;
            }
            Ok(())
        }
        "usda" | "usd" => {
            let parts = asset.output_parts(false)?;
            let (text, pics) = usda(&parts, asset, "textures/");
            w(text.as_bytes(), path)?;
            if !pics.is_empty() {
                let dir = path.with_file_name("textures");
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                for (file, bytes) in pics {
                    w(&bytes, &dir.join(file))?;
                }
            }
            Ok(())
        }
        "usdz" => {
            let parts = asset.output_parts(false)?;
            let (text, pics) = usda(&parts, asset, "textures/");
            let mut files: Vec<(String, Vec<u8>)> = vec![("model.usda".into(), text.into_bytes())];
            files.extend(pics.into_iter().map(|(n, b)| (format!("textures/{n}"), b)));
            w(&zip_stored(&files, 64), path)
        }
        "fbx" => {
            let parts = asset.output_parts(false)?;
            w(&fbx(&parts, asset), path)
        }
        _ => Err("export to .glb, .fbx, .usdz, .usda, .obj, .stl, .ply or .smodel".into()),
    }
}

/// Binary STL (y up).
pub fn stl(m: &Mesh) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    b[..21].copy_from_slice(b"Zornov 3D model, STL ");
    b.extend_from_slice(&(m.triangles() as u32).to_le_bytes());
    for t in 0..m.triangles() {
        let n = out_p(m.face_normal(t));
        for x in n {
            b.extend_from_slice(&x.to_le_bytes());
        }
        for v in m.tri(t) {
            for x in out_p(m.positions[v]) {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0, 0]);
    }
    b
}

/// ASCII PLY with normals, texture coordinates and colours.
pub fn ply(m: &Mesh) -> String {
    let mut s = String::new();
    let mats = m.tri_materials();
    let mut vcol = vec![[200u8, 200, 200]; m.positions.len()];
    for t in 0..m.triangles() {
        let c = m.materials.get(mats[t] as usize).map(|x| x.base_colour).unwrap_or([0.8; 4]);
        for v in m.tri(t) {
            vcol[v] = [(srgb(c[0]) * 255.0) as u8, (srgb(c[1]) * 255.0) as u8, (srgb(c[2]) * 255.0) as u8];
        }
    }
    let _ = writeln!(s, "ply\nformat ascii 1.0\ncomment Zornov\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\nproperty float s\nproperty float t\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face {}\nproperty list uchar int vertex_indices\nend_header", m.positions.len(), m.triangles());
    for (i, p) in m.positions.iter().enumerate() {
        let (p, n, uv) = (out_p(*p), out_p(m.normals.get(i).copied().unwrap_or([0.0, -1.0, 0.0])), m.uvs.get(i).copied().unwrap_or([0.0, 0.0]));
        let c = vcol[i];
        let _ = writeln!(s, "{} {} {} {} {} {} {} {} {} {} {}", p[0], p[1], p[2], n[0], n[1], n[2], uv[0], 1.0 - uv[1], c[0], c[1], c[2]);
    }
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t);
        let _ = writeln!(s, "3 {a} {b} {c}");
    }
    s
}

/// OBJ + MTL with every part as a group and faces as they are.
pub fn obj(parts: &[OutPart], mtl_name: &str) -> (String, String, Vec<(String, Vec<u8>)>) {
    let mut o = format!("# Zornov 3D asset\nmtllib {mtl_name}\n");
    let mut mtl = String::from("# Zornov materials\n");
    let mut pics = Vec::new();
    let (mut vbase, mut tbase, mut nbase) = (1usize, 1usize, 1usize);
    for p in parts {
        let poly = &p.poly;
        let normals = poly.corner_normals(p.smooth_angle, p.mode);
        let _ = writeln!(o, "o {}", safe(&p.name));
        for v in &poly.positions {
            let q = out_p(*v);
            let _ = writeln!(o, "v {} {} {}", q[0], q[1], q[2]);
        }
        let uvs = poly.has_uvs();
        if uvs {
            for u in &poly.uvs {
                let _ = writeln!(o, "vt {} {}", u[0], 1.0 - u[1]);
            }
        }
        for n in &normals {
            let q = out_p(*n);
            let _ = writeln!(o, "vn {} {} {}", q[0], q[1], q[2]);
        }
        let mut last = u32::MAX;
        for f in 0..poly.faces() {
            let mi = poly.face_material[f];
            if mi != last {
                let _ = writeln!(o, "usemtl {}_{}", safe(&p.name), safe(&p.materials.get(mi as usize).map(|m| m.name.clone()).unwrap_or_default()));
                last = mi;
            }
            o.push('f');
            for k in poly.face_range(f) {
                let v = poly.corners[k] as usize + vbase;
                if uvs {
                    let _ = write!(o, " {v}/{}/{}", k + tbase, k + nbase);
                } else {
                    let _ = write!(o, " {v}//{}", k + nbase);
                }
            }
            o.push('\n');
        }
        for m in &p.materials {
            let name = format!("{}_{}", safe(&p.name), safe(&m.name));
            let c = m.base_colour;
            let _ = writeln!(mtl, "newmtl {name}\nKd {} {} {}\nd {}\nPr {}\nPm {}\nNs {}", srgb(c[0]), srgb(c[1]), srgb(c[2]), c[3], m.roughness, m.metallic, ((1.0 - m.roughness) * 1000.0).max(1.0));
            if let Some(t) = &m.texture {
                let file = format!("{name}.png");
                let _ = writeln!(mtl, "map_Kd {file}");
                pics.push((file, png(t)));
            }
        }
        vbase += poly.positions.len();
        tbase += if uvs { poly.uvs.len() } else { 0 };
        nbase += normals.len();
    }
    (o, mtl, pics)
}

/// USD text: an Xform of meshes with preview-surface materials.
pub fn usda(parts: &[OutPart], asset: &Asset, tex_dir: &str) -> (String, Vec<(String, Vec<u8>)>) {
    let mut s = String::new();
    let mut pics = Vec::new();
    let root = safe(&asset.name);
    let _ = writeln!(s, "#usda 1.0\n(\n    defaultPrim = \"{root}\"\n    metersPerUnit = {}\n    upAxis = \"Y\"\n    doc = \"Zornov 3D asset\"\n)\n\ndef Xform \"{root}\" (\n    kind = \"component\"\n)\n{{", 1.0 / asset.px_per_metre.max(1e-6));
    let list = |v: &mut String, items: &mut dyn Iterator<Item = String>| {
        let mut first = true;
        for it in items {
            if !first {
                v.push_str(", ");
            }
            v.push_str(&it);
            first = false;
        }
    };
    for p in parts {
        let poly = &p.poly;
        let pname = safe(&p.name);
        let normals = poly.corner_normals(p.smooth_angle, p.mode);
        let _ = writeln!(s, "    def Mesh \"{pname}\"\n    {{");
        let mut counts = String::new();
        list(&mut counts, &mut (0..poly.faces()).map(|f| poly.face(f).len().to_string()));
        let mut idx = String::new();
        list(&mut idx, &mut poly.corners.iter().map(|c| c.to_string()));
        let mut pts = String::new();
        list(&mut pts, &mut poly.positions.iter().map(|v| {
            let q = out_p(*v);
            format!("({}, {}, {})", q[0], q[1], q[2])
        }));
        let mut nrm = String::new();
        list(&mut nrm, &mut normals.iter().map(|v| {
            let q = out_p(*v);
            format!("({}, {}, {})", q[0], q[1], q[2])
        }));
        let (lo, hi) = poly.bounds();
        let (lo, hi) = (out_p(lo), out_p(hi));
        let _ = writeln!(s, "        uniform token subdivisionScheme = \"none\"\n        int[] faceVertexCounts = [{counts}]\n        int[] faceVertexIndices = [{idx}]\n        point3f[] points = [{pts}]\n        normal3f[] normals = [{nrm}] (\n            interpolation = \"faceVarying\"\n        )\n        float3[] extent = [({}, {}, {}), ({}, {}, {})]", lo[0].min(hi[0]), lo[1].min(hi[1]), lo[2].min(hi[2]), lo[0].max(hi[0]), lo[1].max(hi[1]), lo[2].max(hi[2]));
        if poly.has_uvs() {
            let mut st = String::new();
            list(&mut st, &mut poly.uvs.iter().map(|u| format!("({}, {})", u[0], 1.0 - u[1])));
            let _ = writeln!(s, "        texCoord2f[] primvars:st = [{st}] (\n            interpolation = \"faceVarying\"\n        )");
        }
        // One subset per material slot.
        let slots = poly.face_material.iter().copied().max().map_or(1, |m| m as usize + 1);
        if slots == 1 {
            let _ = writeln!(s, "        rel material:binding = </{root}/Materials/{pname}_0>");
        } else {
            for slot in 0..slots {
                let faces: Vec<String> = (0..poly.faces()).filter(|&f| poly.face_material[f] as usize == slot).map(|f| f.to_string()).collect();
                if faces.is_empty() {
                    continue;
                }
                let _ = writeln!(s, "        def GeomSubset \"slot{slot}\"\n        {{\n            uniform token elementType = \"face\"\n            uniform token familyName = \"materialBind\"\n            int[] indices = [{}]\n            rel material:binding = </{root}/Materials/{pname}_{slot}>\n        }}", faces.join(", "));
            }
        }
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    def Scope \"Materials\"\n    {{");
    for p in parts {
        let pname = safe(&p.name);
        for (slot, m) in p.materials.iter().enumerate() {
            let mname = format!("{pname}_{slot}");
            let c = m.base_colour;
            let _ = writeln!(s, "        def Material \"{mname}\"\n        {{\n            token outputs:surface.connect = </{root}/Materials/{mname}/Surface.outputs:surface>\n            def Shader \"Surface\"\n            {{\n                uniform token info:id = \"UsdPreviewSurface\"");
            if m.texture.is_some() {
                let _ = writeln!(s, "                color3f inputs:diffuseColor.connect = </{root}/Materials/{mname}/Colour.outputs:rgb>");
            } else {
                let _ = writeln!(s, "                color3f inputs:diffuseColor = ({}, {}, {})", c[0], c[1], c[2]);
            }
            let _ = writeln!(s, "                float inputs:roughness = {}\n                float inputs:metallic = {}\n                float inputs:opacity = {}\n                color3f inputs:emissiveColor = ({}, {}, {})\n                token outputs:surface\n            }}", m.roughness, m.metallic, c[3], m.emissive[0], m.emissive[1], m.emissive[2]);
            if let Some(t) = &m.texture {
                let file = format!("{mname}.png");
                let _ = writeln!(s, "            def Shader \"Reader\"\n            {{\n                uniform token info:id = \"UsdPrimvarReader_float2\"\n                string inputs:varname = \"st\"\n                float2 outputs:result\n            }}\n            def Shader \"Colour\"\n            {{\n                uniform token info:id = \"UsdUVTexture\"\n                asset inputs:file = @{tex_dir}{file}@\n                float2 inputs:st.connect = </{root}/Materials/{mname}/Reader.outputs:result>\n                token inputs:sourceColorSpace = \"sRGB\"\n                float3 outputs:rgb\n            }}");
                pics.push((file, png(t)));
            }
            let _ = writeln!(s, "        }}");
        }
    }
    let _ = writeln!(s, "    }}\n}}");
    (s, pics)
}

fn crc32(data: &[u8]) -> u32 {
    studio_media::png::crc32(&[data])
}

/// A zip with stored (uncompressed) files, each one's data aligned to
/// `align` bytes (USDZ needs 64).
pub fn zip_stored(files: &[(String, Vec<u8>)], align: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        // Local header with padding in the extra field.
        let header_len = 30 + name.len();
        let pad = (align - (out.len() + header_len + 4) % align) % align;
        let extra_len = if pad > 0 || align > 1 { 4 + pad } else { 0 };
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(extra_len as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        if extra_len > 0 {
            out.extend_from_slice(&0x1986u16.to_le_bytes());
            out.extend_from_slice(&(pad as u16).to_le_bytes());
            out.extend(std::iter::repeat_n(0u8, pad));
        }
        out.extend_from_slice(data);
        central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

// ------------------------------------------------------------------ FBX

/// A property of an FBX node.
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    I16(i16),
    Bool(bool),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Str(String),
    Raw(Vec<u8>),
    I32s(Vec<i32>),
    I64s(Vec<i64>),
    F32s(Vec<f32>),
    F64s(Vec<f64>),
    Bools(Vec<bool>),
}

/// An FBX node: a name, properties and children.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Node {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: &str, props: Vec<Prop>) -> Node {
        Node { name: name.into(), props, children: Vec::new() }
    }
    pub fn with(mut self, c: Node) -> Node {
        self.children.push(c);
        self
    }
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn s(v: &str) -> Prop {
    Prop::Str(v.into())
}

/// A Properties70 "P" entry.
fn p70(name: &str, ty: &str, ty2: &str, flags: &str, vals: Vec<Prop>) -> Node {
    let mut props = vec![s(name), s(ty), s(ty2), s(flags)];
    props.extend(vals);
    Node::new("P", props)
}

fn write_prop(b: &mut Vec<u8>, p: &Prop) {
    let arr = |b: &mut Vec<u8>, code: u8, n: usize, data: Vec<u8>| {
        b.push(code);
        b.extend_from_slice(&(n as u32).to_le_bytes());
        // Big arrays go compressed (encoding 1, zlib).
        if data.len() > 128 {
            let z = studio_media::deflate::zlib_compress(&data);
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&(z.len() as u32).to_le_bytes());
            b.extend_from_slice(&z);
        } else {
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&(data.len() as u32).to_le_bytes());
            b.extend_from_slice(&data);
        }
    };
    match p {
        Prop::I16(v) => {
            b.push(b'Y');
            b.extend_from_slice(&v.to_le_bytes());
        }
        Prop::Bool(v) => {
            b.push(b'C');
            b.push(if *v { b'T' } else { b'F' });
        }
        Prop::I32(v) => {
            b.push(b'I');
            b.extend_from_slice(&v.to_le_bytes());
        }
        Prop::I64(v) => {
            b.push(b'L');
            b.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F32(v) => {
            b.push(b'F');
            b.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F64(v) => {
            b.push(b'D');
            b.extend_from_slice(&v.to_le_bytes());
        }
        Prop::Str(v) => {
            b.push(b'S');
            b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            b.extend_from_slice(v.as_bytes());
        }
        Prop::Raw(v) => {
            b.push(b'R');
            b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            b.extend_from_slice(v);
        }
        Prop::I32s(v) => arr(b, b'i', v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
        Prop::I64s(v) => arr(b, b'l', v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
        Prop::F32s(v) => arr(b, b'f', v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
        Prop::F64s(v) => arr(b, b'd', v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
        Prop::Bools(v) => arr(b, b'b', v.len(), v.iter().map(|x| *x as u8).collect()),
    }
}

fn write_node(b: &mut Vec<u8>, n: &Node) {
    let start = b.len();
    b.extend_from_slice(&[0u8; 12]);
    b.push(n.name.len() as u8);
    b.extend_from_slice(n.name.as_bytes());
    let props_start = b.len();
    for p in &n.props {
        write_prop(b, p);
    }
    let props_len = b.len() - props_start;
    if !n.children.is_empty() {
        for c in &n.children {
            write_node(b, c);
        }
        b.extend_from_slice(&[0u8; 13]);
    }
    let end = b.len() as u32;
    b[start..start + 4].copy_from_slice(&end.to_le_bytes());
    b[start + 4..start + 8].copy_from_slice(&(n.props.len() as u32).to_le_bytes());
    b[start + 8..start + 12].copy_from_slice(&(props_len as u32).to_le_bytes());
}

const FBX_MAGIC: &[u8] = b"Kaydara FBX Binary  \x00\x1a\x00";

/// FBX nodes as a binary 7.4 file.
pub fn fbx_bytes(nodes: &[Node]) -> Vec<u8> {
    let mut b = FBX_MAGIC.to_vec();
    b.extend_from_slice(&7400u32.to_le_bytes());
    for n in nodes {
        write_node(&mut b, n);
    }
    b.extend_from_slice(&[0u8; 13]);
    // Footer: id, padding to 16, version, zeros, magic.
    b.extend_from_slice(&[0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e]);
    let pad = (16 - b.len() % 16) % 16;
    b.extend(std::iter::repeat_n(0u8, if pad == 0 { 16 } else { pad }));
    b.extend_from_slice(&7400u32.to_le_bytes());
    b.extend(std::iter::repeat_n(0u8, 120));
    b.extend_from_slice(&[0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b]);
    b
}

/// The asset as a binary FBX: one Model + Geometry per part, a material
/// per slot, textures embedded.
pub fn fbx(parts: &[OutPart], asset: &Asset) -> Vec<u8> {
    let mut objects = Node::new("Objects", vec![]);
    let mut conns = Node::new("Connections", vec![]);
    let mut next_id: i64 = 1_000_000;
    let mut id = || {
        next_id += 1;
        next_id
    };
    let unit_cm = 100.0 / asset.px_per_metre.max(1e-6) as f64;
    for p in parts {
        let poly = &p.poly;
        let (gid, mid) = (id(), id());
        let pts: Vec<f64> = poly.positions.iter().flat_map(|v| out_p(*v).map(|x| x as f64)).collect();
        let mut pvi: Vec<i32> = Vec::with_capacity(poly.corners.len());
        for f in 0..poly.faces() {
            let c = poly.face(f);
            for (k, &v) in c.iter().enumerate() {
                pvi.push(if k + 1 == c.len() { -(v as i32) - 1 } else { v as i32 });
            }
        }
        let normals = poly.corner_normals(p.smooth_angle, p.mode);
        let nrm: Vec<f64> = normals.iter().flat_map(|v| out_p(*v).map(|x| x as f64)).collect();
        let mut geom = Node::new("Geometry", vec![Prop::I64(gid), s(&format!("{}\x00\x01Geometry", p.name)), s("Mesh")])
            .with(Node::new("Vertices", vec![Prop::F64s(pts)]))
            .with(Node::new("PolygonVertexIndex", vec![Prop::I32s(pvi)]))
            .with(Node::new("GeometryVersion", vec![Prop::I32(124)]))
            .with(Node::new("LayerElementNormal", vec![Prop::I32(0)]).with(Node::new("Version", vec![Prop::I32(101)])).with(Node::new("Name", vec![s("")])).with(Node::new("MappingInformationType", vec![s("ByPolygonVertex")])).with(Node::new("ReferenceInformationType", vec![s("Direct")])).with(Node::new("Normals", vec![Prop::F64s(nrm)])));
        let mut layer = Node::new("Layer", vec![Prop::I32(0)]).with(Node::new("Version", vec![Prop::I32(100)])).with(Node::new("LayerElement", vec![]).with(Node::new("Type", vec![s("LayerElementNormal")])).with(Node::new("TypedIndex", vec![Prop::I32(0)])));
        if poly.has_uvs() {
            let uv: Vec<f64> = poly.uvs.iter().flat_map(|u| [u[0] as f64, 1.0 - u[1] as f64]).collect();
            let idx: Vec<i32> = (0..poly.uvs.len() as i32).collect();
            geom.children.push(Node::new("LayerElementUV", vec![Prop::I32(0)]).with(Node::new("Version", vec![Prop::I32(101)])).with(Node::new("Name", vec![s("UVMap")])).with(Node::new("MappingInformationType", vec![s("ByPolygonVertex")])).with(Node::new("ReferenceInformationType", vec![s("IndexToDirect")])).with(Node::new("UV", vec![Prop::F64s(uv)])).with(Node::new("UVIndex", vec![Prop::I32s(idx)])));
            layer.children.push(Node::new("LayerElement", vec![]).with(Node::new("Type", vec![s("LayerElementUV")])).with(Node::new("TypedIndex", vec![Prop::I32(0)])));
        }
        let mats: Vec<i32> = poly.face_material.iter().map(|m| *m as i32).collect();
        geom.children.push(Node::new("LayerElementMaterial", vec![Prop::I32(0)]).with(Node::new("Version", vec![Prop::I32(101)])).with(Node::new("Name", vec![s("")])).with(Node::new("MappingInformationType", vec![s("ByPolygon")])).with(Node::new("ReferenceInformationType", vec![s("IndexToDirect")])).with(Node::new("Materials", vec![Prop::I32s(mats)])));
        layer.children.push(Node::new("LayerElement", vec![]).with(Node::new("Type", vec![s("LayerElementMaterial")])).with(Node::new("TypedIndex", vec![Prop::I32(0)])));
        // Creases on the edges, for packages that subdivide.
        geom.children.push(layer);
        objects.children.push(geom);
        let props = Node::new("Properties70", vec![]).with(p70("Lcl Scaling", "Lcl Scaling", "", "A", vec![Prop::F64(unit_cm), Prop::F64(unit_cm), Prop::F64(unit_cm)]));
        objects.children.push(Node::new("Model", vec![Prop::I64(mid), s(&format!("{}\x00\x01Model", p.name)), s("Mesh")]).with(Node::new("Version", vec![Prop::I32(232)])).with(props).with(Node::new("Shading", vec![Prop::Bool(true)])).with(Node::new("Culling", vec![s("CullingOff")])));
        conns.children.push(Node::new("C", vec![s("OO"), Prop::I64(gid), Prop::I64(mid)]));
        conns.children.push(Node::new("C", vec![s("OO"), Prop::I64(mid), Prop::I64(0)]));
        for m in &p.materials {
            let matid = id();
            let c = m.base_colour;
            let props = Node::new("Properties70", vec![])
                .with(p70("DiffuseColor", "Color", "", "A", vec![Prop::F64(c[0] as f64), Prop::F64(c[1] as f64), Prop::F64(c[2] as f64)]))
                .with(p70("EmissiveColor", "Color", "", "A", vec![Prop::F64(m.emissive[0] as f64), Prop::F64(m.emissive[1] as f64), Prop::F64(m.emissive[2] as f64)]))
                .with(p70("Opacity", "double", "Number", "", vec![Prop::F64(c[3] as f64)]))
                .with(p70("Shininess", "double", "Number", "", vec![Prop::F64(((1.0 - m.roughness) * 100.0) as f64)]))
                .with(p70("ReflectionFactor", "double", "Number", "", vec![Prop::F64(m.metallic as f64)]));
            objects.children.push(Node::new("Material", vec![Prop::I64(matid), s(&format!("{}\x00\x01Material", m.name)), s("")]).with(Node::new("Version", vec![Prop::I32(102)])).with(Node::new("ShadingModel", vec![s("phong")])).with(Node::new("MultiLayer", vec![Prop::I32(0)])).with(props));
            conns.children.push(Node::new("C", vec![s("OO"), Prop::I64(matid), Prop::I64(mid)]));
            if let Some(t) = &m.texture {
                let (tid, vid) = (id(), id());
                let file = format!("{}_{}.png", safe(&p.name), safe(&m.name));
                objects.children.push(Node::new("Video", vec![Prop::I64(vid), s(&format!("{file}\x00\x01Video")), s("Clip")]).with(Node::new("Type", vec![s("Clip")])).with(Node::new("FileName", vec![s(&file)])).with(Node::new("RelativeFilename", vec![s(&file)])).with(Node::new("Content", vec![Prop::Raw(png(t))])));
                objects.children.push(Node::new("Texture", vec![Prop::I64(tid), s(&format!("{file}\x00\x01Texture")), s("")]).with(Node::new("Type", vec![s("TextureVideoClip")])).with(Node::new("Version", vec![Prop::I32(202)])).with(Node::new("TextureName", vec![s(&format!("{file}\x00\x01Texture"))])).with(Node::new("FileName", vec![s(&file)])).with(Node::new("RelativeFilename", vec![s(&file)])));
                conns.children.push(Node::new("C", vec![s("OO"), Prop::I64(vid), Prop::I64(tid)]));
                conns.children.push(Node::new("C", vec![s("OP"), Prop::I64(tid), Prop::I64(matid), s("DiffuseColor")]));
            }
        }
    }
    let header = Node::new("FBXHeaderExtension", vec![]).with(Node::new("FBXHeaderVersion", vec![Prop::I32(1003)])).with(Node::new("FBXVersion", vec![Prop::I32(7400)])).with(Node::new("Creator", vec![s("Zornov")]));
    let settings = Node::new("GlobalSettings", vec![]).with(Node::new("Version", vec![Prop::I32(1000)])).with(
        Node::new("Properties70", vec![])
            .with(p70("UpAxis", "int", "Integer", "", vec![Prop::I32(1)]))
            .with(p70("UpAxisSign", "int", "Integer", "", vec![Prop::I32(1)]))
            .with(p70("FrontAxis", "int", "Integer", "", vec![Prop::I32(2)]))
            .with(p70("FrontAxisSign", "int", "Integer", "", vec![Prop::I32(1)]))
            .with(p70("CoordAxis", "int", "Integer", "", vec![Prop::I32(0)]))
            .with(p70("CoordAxisSign", "int", "Integer", "", vec![Prop::I32(1)]))
            .with(p70("UnitScaleFactor", "double", "Number", "", vec![Prop::F64(1.0)])),
    );
    let defs = Node::new("Definitions", vec![]).with(Node::new("Version", vec![Prop::I32(100)]));
    fbx_bytes(&[header, Node::new("FileId", vec![Prop::Raw(vec![0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b, 0xfc, 0xf1])]), Node::new("CreationTime", vec![s("1970-01-01 10:00:00:000")]), Node::new("Creator", vec![s("Zornov")]), settings, defs, objects, conns])
}

// ------------------------------------------------------------- reading

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("the FBX file ends early")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let s = self.take(8)?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }
}

fn read_array<T>(r: &mut Rd, size: usize, conv: impl Fn(&[u8]) -> T) -> Result<Vec<T>, String> {
    let n = r.u32()? as usize;
    let enc = r.u32()?;
    let len = r.u32()? as usize;
    let raw = r.take(len)?.to_vec();
    let data = if enc == 1 { studio_media::deflate::zlib_decompress(&raw, n * size + 16).map_err(|e| format!("FBX array: {e:?}"))? } else { raw };
    if data.len() < n * size {
        return Err("the FBX file has a short array".into());
    }
    Ok(data.chunks_exact(size).take(n).map(conv).collect())
}

fn read_prop(r: &mut Rd) -> Result<Prop, String> {
    let code = r.take(1)?[0];
    Ok(match code {
        b'Y' => {
            let s = r.take(2)?;
            Prop::I16(i16::from_le_bytes([s[0], s[1]]))
        }
        b'C' => {
            let c = r.take(1)?[0];
            Prop::Bool(c == b'T' || c == 1)
        }
        b'I' => Prop::I32(r.u32()? as i32),
        b'F' => Prop::F32(f32::from_bits(r.u32()?)),
        b'D' => Prop::F64(f64::from_bits(r.u64()?)),
        b'L' => Prop::I64(r.u64()? as i64),
        b'S' => {
            let n = r.u32()? as usize;
            Prop::Str(String::from_utf8_lossy(r.take(n)?).into_owned())
        }
        b'R' => {
            let n = r.u32()? as usize;
            Prop::Raw(r.take(n)?.to_vec())
        }
        b'f' => Prop::F32s(read_array(r, 4, |c| f32::from_le_bytes(c.try_into().unwrap()))?),
        b'd' => Prop::F64s(read_array(r, 8, |c| f64::from_le_bytes(c.try_into().unwrap()))?),
        b'i' => Prop::I32s(read_array(r, 4, |c| i32::from_le_bytes(c.try_into().unwrap()))?),
        b'l' => Prop::I64s(read_array(r, 8, |c| i64::from_le_bytes(c.try_into().unwrap()))?),
        b'b' => Prop::Bools(read_array(r, 1, |c| c[0] != 0)?),
        other => return Err(format!("the FBX file has an unknown property type {}", other as char)),
    })
}

fn read_node(r: &mut Rd, wide: bool, depth: usize) -> Result<Option<Node>, String> {
    if depth > 64 {
        return Err("the FBX file nests too deeply".into());
    }
    let (end, nprops, _plen) = if wide { (r.u64()? as usize, r.u64()? as usize, r.u64()? as usize) } else { (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize) };
    let nlen = r.take(1)?[0] as usize;
    if end == 0 {
        return Ok(None);
    }
    if end > r.b.len() {
        return Err("the FBX file is damaged (a node runs past the end)".into());
    }
    let name = String::from_utf8_lossy(r.take(nlen)?).into_owned();
    let mut n = Node { name, props: Vec::with_capacity(nprops.min(64)), children: Vec::new() };
    for _ in 0..nprops {
        n.props.push(read_prop(r)?);
    }
    while r.at < end {
        match read_node(r, wide, depth + 1)? {
            Some(c) => n.children.push(c),
            None => break,
        }
    }
    r.at = end;
    Ok(Some(n))
}

/// The top nodes of a binary FBX file.
pub fn read_fbx(b: &[u8]) -> Result<Vec<Node>, String> {
    if !b.starts_with(&FBX_MAGIC[..20]) {
        return Err("not a binary FBX file (ASCII FBX: export it as binary)".into());
    }
    let mut r = Rd { b, at: 23 };
    let version = r.u32()?;
    let wide = version >= 7500;
    let mut out = Vec::new();
    while r.at < b.len() {
        match read_node(&mut r, wide, 0)? {
            Some(n) => out.push(n),
            None => break,
        }
    }
    Ok(out)
}

fn f64s(p: &Prop) -> Vec<f64> {
    match p {
        Prop::F64s(v) => v.clone(),
        Prop::F32s(v) => v.iter().map(|x| *x as f64).collect(),
        _ => Vec::new(),
    }
}

fn i32s(p: &Prop) -> Vec<i32> {
    match p {
        Prop::I32s(v) => v.clone(),
        Prop::I64s(v) => v.iter().map(|x| *x as i32).collect(),
        _ => Vec::new(),
    }
}

/// Meshes in an FBX file as polygon meshes (one per geometry), in
/// Studio's frame, scaled so the largest side is `fit` px.
pub fn import_fbx(b: &[u8], fit: f32) -> Result<Vec<(String, Poly)>, String> {
    let nodes = read_fbx(b)?;
    let objects = nodes.iter().find(|n| n.name == "Objects").ok_or("the FBX file has no objects")?;
    let mut out = Vec::new();
    for g in objects.all("Geometry") {
        if g.props.get(2) != Some(&Prop::Str("Mesh".into())) {
            continue;
        }
        let name = match g.props.get(1) {
            Some(Prop::Str(s)) => s.split('\0').next().unwrap_or("Mesh").to_owned(),
            _ => "Mesh".into(),
        };
        let pts = g.child("Vertices").and_then(|v| v.props.first()).map(f64s).unwrap_or_default();
        let idx = g.child("PolygonVertexIndex").and_then(|v| v.props.first()).map(i32s).unwrap_or_default();
        let mut p = Poly::new();
        for c in pts.chunks_exact(3) {
            p.positions.push([c[0] as f32, -(c[1] as f32), -(c[2] as f32)]);
        }
        let uv_node = g.child("LayerElementUV");
        let uv_data = uv_node.and_then(|u| u.child("UV")).and_then(|v| v.props.first()).map(f64s).unwrap_or_default();
        let uv_index = uv_node.and_then(|u| u.child("UVIndex")).and_then(|v| v.props.first()).map(i32s);
        let by_vertex = uv_node.and_then(|u| u.child("MappingInformationType")).is_some_and(|m| matches!(m.props.first(), Some(Prop::Str(s)) if s == "ByVertice" || s == "ByVertex"));
        let mats = g.child("LayerElementMaterial").and_then(|m| m.child("Materials")).and_then(|v| v.props.first()).map(i32s).unwrap_or_default();
        let mut face: Vec<u32> = Vec::new();
        let mut fuv: Vec<[f32; 2]> = Vec::new();
        let have_uv = !uv_data.is_empty();
        for (k, &raw) in idx.iter().enumerate() {
            let last = raw < 0;
            let v = if last { (-raw - 1) as u32 } else { raw as u32 };
            if v as usize >= p.positions.len() {
                return Err("the FBX file's faces point past its vertices".into());
            }
            face.push(v);
            if have_uv {
                let ui = if by_vertex { v as usize } else { uv_index.as_ref().and_then(|ix| ix.get(k)).map_or(k, |&x| x as usize) };
                let u = uv_data.get(2 * ui).copied().unwrap_or(0.0) as f32;
                let w = uv_data.get(2 * ui + 1).copied().unwrap_or(0.0) as f32;
                fuv.push([u, 1.0 - w]);
            }
            if last {
                let fi = p.faces();
                let m = if mats.len() == 1 { mats[0] } else { mats.get(fi).copied().unwrap_or(0) };
                // Mirrored y and z keep the winding.
                p.add_face_like(&face, have_uv.then_some(&fuv[..]), m.max(0) as u32, 0, true);
                face.clear();
                fuv.clear();
            }
        }
        if p.faces() == 0 {
            continue;
        }
        p.ensure_materials();
        out.push((name, p));
    }
    if out.is_empty() {
        return Err("the FBX file has no meshes".into());
    }
    fit_all(&mut out, fit);
    Ok(out)
}

/// Scales parts together so the largest side is `fit` px, centred.
fn fit_all(parts: &mut [(String, Poly)], fit: f32) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for (_, p) in parts.iter() {
        let (a, b) = p.bounds();
        for i in 0..3 {
            lo[i] = lo[i].min(a[i]);
            hi[i] = hi[i].max(b[i]);
        }
    }
    let big = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f32, f32::max).max(1e-9);
    let k = if fit > 0.0 { fit / big } else { 1.0 };
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    for (_, p) in parts.iter_mut() {
        p.map_positions(|v| [(v[0] - c[0]) * k, (v[1] - c[1]) * k, (v[2] - c[2]) * k]);
    }
}

/// Meshes in a USD text file (each `def Mesh` with its points, counts,
/// indices and `primvars:st`).
pub fn import_usda(text: &str, fit: f32) -> Result<Vec<(String, Poly)>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("def Mesh \"") {
        rest = &rest[i + 10..];
        let name = rest.split('"').next().unwrap_or("Mesh").to_owned();
        let body_end = rest.find("\n    def ").or_else(|| rest.find("\ndef ")).unwrap_or(rest.len());
        let body = &rest[..body_end];
        let array = |key: &str| -> Option<&str> {
            let k = body.find(key)?;
            let after = &body[k..];
            let eq = after.find('=')?;
            let open = after[eq..].find('[')? + eq;
            let close = after[open..].find(']')? + open;
            Some(&after[open + 1..close])
        };
        let nums = |s: &str| -> Vec<f64> { s.split(|c: char| c == ',' || c == '(' || c == ')' || c.is_whitespace()).filter(|x| !x.is_empty()).filter_map(|x| x.parse::<f64>().ok()).collect() };
        let counts: Vec<usize> = array("faceVertexCounts").map(nums).unwrap_or_default().into_iter().map(|x| x as usize).collect();
        let idx: Vec<u32> = array("faceVertexIndices").map(nums).unwrap_or_default().into_iter().map(|x| x as u32).collect();
        let pts = array("point3f[] points").map(nums).unwrap_or_default();
        let st = array("primvars:st").map(nums).unwrap_or_default();
        let mut p = Poly::new();
        for c in pts.chunks_exact(3) {
            p.positions.push([c[0] as f32, -(c[1] as f32), -(c[2] as f32)]);
        }
        let face_varying = st.len() / 2 == idx.len() && !st.is_empty();
        let mut k = 0;
        for &n in &counts {
            if k + n > idx.len() {
                return Err(format!("USD mesh \"{name}\": its faces run past its indices"));
            }
            let f = &idx[k..k + n];
            if f.iter().any(|&v| v as usize >= p.positions.len()) {
                return Err(format!("USD mesh \"{name}\": a face uses a missing point"));
            }
            let uv: Vec<[f32; 2]> = if face_varying { (k..k + n).map(|c| [st[2 * c] as f32, 1.0 - st[2 * c + 1] as f32]).collect() } else { Vec::new() };
            p.add_face_like(f, face_varying.then_some(&uv[..]), 0, 0, true);
            k += n;
        }
        if p.faces() > 0 {
            p.ensure_materials();
            out.push((name, p));
        }
        rest = &rest[body_end..];
    }
    if out.is_empty() {
        return Err("no meshes in that USD file (binary .usdc isn't read; export as .usda)".into());
    }
    fit_all(&mut out, fit);
    Ok(out)
}

/// A USDZ's text layer (the first .usda inside), if it has one.
pub fn usdz_text(b: &[u8]) -> Result<String, String> {
    let mut at = 0;
    while at + 30 <= b.len() && b[at..at + 4] == [0x50, 0x4b, 0x03, 0x04] {
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let method = u16_at(at + 8);
        let size = u32_at(at + 18);
        let (nlen, xlen) = (u16_at(at + 26), u16_at(at + 28));
        let name = String::from_utf8_lossy(b.get(at + 30..at + 30 + nlen).ok_or("damaged zip")?).into_owned();
        let data_at = at + 30 + nlen + xlen;
        let data = b.get(data_at..data_at + size).ok_or("damaged zip")?;
        if name.ends_with(".usda") && method == 0 {
            return Ok(String::from_utf8_lossy(data).into_owned());
        }
        if name.ends_with(".usdc") || name.ends_with(".usd") {
            return Err("this USDZ holds a binary USD layer, which Zornov doesn't read yet; export it with a .usda layer".into());
        }
        at = data_at + size;
    }
    Err("no USD layer found in that USDZ".into())
}

/// An ASCII PLY's triangles or polygons.
pub fn import_ply(text: &str, fit: f32) -> Result<Vec<(String, Poly)>, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("ply") {
        return Err("not a PLY file".into());
    }
    let (mut nv, mut nf) = (0usize, 0usize);
    let mut props: Vec<String> = Vec::new();
    let mut in_vertex = false;
    for l in lines.by_ref() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.as_slice() {
            ["format", f, ..] if *f != "ascii" => return Err("only ASCII PLY files are read".into()),
            ["element", "vertex", n] => {
                nv = n.parse().unwrap_or(0);
                in_vertex = true;
            }
            ["element", "face", n] => {
                nf = n.parse().unwrap_or(0);
                in_vertex = false;
            }
            ["element", ..] => in_vertex = false,
            ["property", _, name] if in_vertex => props.push(name.to_string()),
            ["end_header"] => break,
            _ => {}
        }
    }
    let pi = |n: &str| props.iter().position(|p| p == n);
    let (xi, yi, zi) = (pi("x").ok_or("PLY without x")?, pi("y").ok_or("PLY without y")?, pi("z").ok_or("PLY without z")?);
    let mut p = Poly::new();
    for _ in 0..nv {
        let v: Vec<f32> = lines.next().ok_or("PLY ends early")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        p.positions.push([v.get(xi).copied().unwrap_or(0.0), -v.get(yi).copied().unwrap_or(0.0), -v.get(zi).copied().unwrap_or(0.0)]);
    }
    for _ in 0..nf {
        let v: Vec<u32> = lines.next().ok_or("PLY ends early")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if let Some((&n, rest)) = v.split_first()
            && rest.len() >= n as usize
            && rest[..n as usize].iter().all(|&i| (i as usize) < nv)
        {
            p.add_face(&rest[..n as usize], 0);
        }
    }
    p.ensure_materials();
    let mut out = vec![("Mesh".to_string(), p)];
    fit_all(&mut out, fit);
    Ok(out)
}

/// The importers by extension.
pub fn import(path: &Path, fit: f32) -> Result<Vec<(String, Poly)>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match ext.as_str() {
        "fbx" => import_fbx(&bytes, fit),
        "usda" | "usd" => import_usda(&String::from_utf8_lossy(&bytes), fit),
        "usdz" => import_usda(&usdz_text(&bytes)?, fit),
        "ply" => import_ply(&String::from_utf8_lossy(&bytes), fit),
        "abc" => Err("Alembic (.abc) isn't read yet; bring the model in as FBX, USD or glTF".into()),
        _ => {
            let got = studio_mesh::import::load(path, fit)?;
            let mut p = Poly::from_mesh(&got.mesh);
            crate::edit::tris_to_quads(&mut p, 2.0);
            Ok(vec![(path.file_stem().and_then(|s| s.to_str()).unwrap_or("Mesh").to_owned(), p)])
        }
    }
}

pub const IMPORTS: &[&str] = &["fbx", "usda", "usd", "usdz", "ply", "glb", "gltf", "obj", "stl"];
pub const EXPORTS: &[&str] = &["glb", "fbx", "usdz", "usda", "obj", "stl", "ply", "smodel"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, Part};
    use crate::prim;

    fn crate_asset() -> Asset {
        let mut a = Asset::new("Box");
        let mut p = Part::new("Body", prim::cube([100.0; 3], 2));
        p.materials = vec!["plastic".into()];
        a.parts.push(p);
        a.parts.push(Part::new("Lid", prim::cylinder(30.0, 30.0, 10.0, 12, 1, prim::Cap::Ngon)));
        a.materials.push(crate::material::smart("plastic", 100.0, None).unwrap());
        a.texture_size = 64;
        a
    }

    #[test]
    fn fbx_round_trips_quads_and_ngons() {
        let a = crate_asset();
        let parts = a.output_parts(true).unwrap();
        let bytes = fbx(&parts, &a);
        let back = import_fbx(&bytes, 0.0).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].1.faces(), parts[0].poly.faces());
        assert!((0..back[0].1.faces()).all(|f| back[0].1.face(f).len() == 4));
        assert!((0..back[1].1.faces()).any(|f| back[1].1.face(f).len() == 12), "the cap stays one n-gon");
        // Same shape back (mirrored twice).
        let (lo, hi) = back[0].1.bounds();
        assert!((hi[0] - lo[0] - 100.0).abs() < 1e-3);
        assert!(back[0].1.has_uvs());
    }

    #[test]
    fn usd_text_and_usdz_round_trip() {
        let a = crate_asset();
        let parts = a.output_parts(false).unwrap();
        let (text, pics) = usda(&parts, &a, "textures/");
        assert!(text.contains("UsdPreviewSurface") && text.contains("primvars:st"));
        assert!(!pics.is_empty(), "the baked plastic travels as a texture");
        let back = import_usda(&text, 0.0).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].1.faces(), parts[0].poly.faces());
        let mut files = vec![("model.usda".to_string(), text.clone().into_bytes())];
        files.extend(pics.into_iter().map(|(n, b)| (format!("textures/{n}"), b)));
        let z = zip_stored(&files, 64);
        assert_eq!(usdz_text(&z).unwrap(), text);
    }

    #[test]
    fn stl_ply_and_obj_write() {
        let a = crate_asset();
        let m = a.to_mesh(true).unwrap();
        let s = stl(&m);
        assert_eq!(s.len(), 84 + 50 * m.triangles());
        let back = import_ply(&ply(&m), 0.0).unwrap();
        assert_eq!(back[0].1.faces(), m.triangles());
        let (o, mtl, _) = obj(&a.output_parts(true).unwrap(), "x.mtl");
        assert!(o.lines().filter(|l| l.starts_with("f ")).count() == a.output_parts(true).unwrap().iter().map(|p| p.poly.faces()).sum::<usize>());
        assert!(mtl.contains("newmtl"));
    }
}
