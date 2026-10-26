// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Models out to other programs: GLB (glTF 2.0 in one file, textures
//! inside; a rigged model keeps its skeleton, skin, shape keys as morph
//! targets and actions as animations) and OBJ with its `.mtl`. Both are
//! y-up, as those programs expect.

use crate::Mesh;

fn to_y_up(p: [f32; 3]) -> [f32; 3] {
    [p[0], -p[1], -p[2]]
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn num(v: f32) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

/// The whole model as one `.glb` file.
pub fn glb(m: &Mesh) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<String> = Vec::new();
    let mut accessors: Vec<String> = Vec::new();
    let mut push_view = |bin: &mut Vec<u8>, data: &[u8], target: Option<u32>| -> usize {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let at = bin.len();
        bin.extend_from_slice(data);
        let t = target.map(|t| format!(",\"target\":{t}")).unwrap_or_default();
        views.push(format!("{{\"buffer\":0,\"byteOffset\":{at},\"byteLength\":{}{t}}}", data.len()));
        views.len() - 1
    };
    let f32s = |vals: &mut dyn Iterator<Item = f32>| -> Vec<u8> { vals.flat_map(|v| v.to_le_bytes()).collect() };
    let (lo, hi) = {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &m.positions {
            let q = to_y_up(*p);
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        (lo, hi)
    };
    let n = m.positions.len();
    let pv = push_view(&mut bin, &f32s(&mut m.positions.iter().flat_map(|p| to_y_up(*p))), Some(34962));
    accessors.push(format!("{{\"bufferView\":{pv},\"componentType\":5126,\"count\":{n},\"type\":\"VEC3\",\"min\":[{},{},{}],\"max\":[{},{},{}]}}", num(lo[0]), num(lo[1]), num(lo[2]), num(hi[0]), num(hi[1]), num(hi[2])));
    let nv = push_view(&mut bin, &f32s(&mut m.normals.iter().flat_map(|p| to_y_up(*p))), Some(34962));
    accessors.push(format!("{{\"bufferView\":{nv},\"componentType\":5126,\"count\":{n},\"type\":\"VEC3\"}}"));
    let uv = push_view(&mut bin, &f32s(&mut m.uvs.iter().flat_map(|u| *u)), Some(34962));
    accessors.push(format!("{{\"bufferView\":{uv},\"componentType\":5126,\"count\":{n},\"type\":\"VEC2\"}}"));
    // The rig: skin attributes and morph targets shared by every primitive.
    let rig = m.rig.as_deref().filter(|r| !r.bones.is_empty() || !r.shapes.is_empty());
    let mut skin_attrs = String::new();
    let mut targets = String::new();
    if let Some(r) = rig {
        if r.is_skinned() && r.joints.len() == n {
            let jv = push_view(&mut bin, &r.joints.iter().flat_map(|j| j.iter().flat_map(|x| x.to_le_bytes())).collect::<Vec<u8>>(), Some(34962));
            accessors.push(format!("{{\"bufferView\":{jv},\"componentType\":5123,\"count\":{n},\"type\":\"VEC4\"}}"));
            let ja = accessors.len() - 1;
            let wv = push_view(&mut bin, &f32s(&mut r.weights.iter().flat_map(|w| *w)), Some(34962));
            accessors.push(format!("{{\"bufferView\":{wv},\"componentType\":5126,\"count\":{n},\"type\":\"VEC4\"}}"));
            skin_attrs = format!(",\"JOINTS_0\":{ja},\"WEIGHTS_0\":{}", accessors.len() - 1);
        }
        let mut list = Vec::new();
        for k in &r.shapes {
            let mut dense = vec![[0.0f32; 3]; n];
            for (i, d) in k.indices.iter().zip(&k.deltas) {
                if let Some(v) = dense.get_mut(*i as usize) {
                    *v = to_y_up(*d);
                }
            }
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for d in &dense {
                for i in 0..3 {
                    lo[i] = lo[i].min(d[i]);
                    hi[i] = hi[i].max(d[i]);
                }
            }
            let v = push_view(&mut bin, &f32s(&mut dense.iter().flat_map(|d| *d)), Some(34962));
            accessors.push(format!("{{\"bufferView\":{v},\"componentType\":5126,\"count\":{n},\"type\":\"VEC3\",\"min\":[{},{},{}],\"max\":[{},{},{}]}}", num(lo[0]), num(lo[1]), num(lo[2]), num(hi[0]), num(hi[1]), num(hi[2])));
            list.push(format!("{{\"POSITION\":{}}}", accessors.len() - 1));
        }
        if !list.is_empty() {
            targets = format!(",\"targets\":[{}]", list.join(","));
        }
    }
    let mut prims = Vec::new();
    for g in &m.groups {
        let idx: Vec<u8> = m.indices[3 * g.start as usize..3 * (g.start + g.count) as usize].iter().flat_map(|i| i.to_le_bytes()).collect();
        let iv = push_view(&mut bin, &idx, Some(34963));
        accessors.push(format!("{{\"bufferView\":{iv},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}", g.count * 3));
        prims.push(format!("{{\"attributes\":{{\"POSITION\":0,\"NORMAL\":1,\"TEXCOORD_0\":2{skin_attrs}}},\"indices\":{},\"material\":{}{targets}}}", accessors.len() - 1, g.material));
    }
    let mut images = Vec::new();
    let mut materials = Vec::new();
    for mat in &m.materials {
        let tex = mat.texture.as_ref().map(|t| {
            let png = studio_media::png::encode(&studio_media::RgbaImage { width: t.width, height: t.height, pixels: t.rgba.clone() });
            let v = push_view(&mut bin, &png, None);
            images.push(format!("{{\"bufferView\":{v},\"mimeType\":\"image/png\"}}"));
            images.len() - 1
        });
        let c = mat.base_colour;
        let tex_json = tex.map(|i| format!(",\"baseColorTexture\":{{\"index\":{i}}}")).unwrap_or_default();
        let blend = if c[3] < 1.0 { ",\"alphaMode\":\"BLEND\"" } else { "" };
        let glass = if mat.transmission > 0.0 { format!(",\"extensions\":{{\"KHR_materials_transmission\":{{\"transmissionFactor\":{}}},\"KHR_materials_ior\":{{\"ior\":{}}}}}", num(mat.transmission), num(mat.ior)) } else { String::new() };
        materials.push(format!(
            "{{\"name\":{},\"pbrMetallicRoughness\":{{\"baseColorFactor\":[{},{},{},{}],\"metallicFactor\":{},\"roughnessFactor\":{}{tex_json}}},\"emissiveFactor\":[{},{},{}]{blend}{glass},\"doubleSided\":true}}",
            json_str(&mat.name), num(c[0]), num(c[1]), num(c[2]), num(c[3]), num(mat.metallic), num(mat.roughness), num(mat.emissive[0].min(1.0)), num(mat.emissive[1].min(1.0)), num(mat.emissive[2].min(1.0))
        ));
    }
    // Joints as nodes 1.., their rest poses, inverse bind matrices, and
    // each action as an animation.
    let mut nodes = vec![format!("{{\"mesh\":0,\"name\":\"Model\"{}}}", if rig.is_some_and(|r| r.is_skinned() && !r.bones.is_empty()) { ",\"skin\":0" } else { "" })];
    let mut scene_nodes = vec!["0".to_string()];
    let mut skins = String::new();
    let mut animations = Vec::new();
    let mut mesh_extra = String::new();
    if let Some(r) = rig {
        if !r.shapes.is_empty() {
            mesh_extra = format!(",\"weights\":[{}],\"extras\":{{\"targetNames\":[{}]}}", vec!["0"; r.shapes.len()].join(","), r.shapes.iter().map(|k| json_str(&k.name)).collect::<Vec<_>>().join(","));
        }
        if !r.bones.is_empty() {
            let rest = r.rest_world();
            let rest_local = r.rest_local();
            let q_up = |q: crate::rig::Quat| [q[0], -q[1], -q[2], q[3]];
            for (i, b) in r.bones.iter().enumerate() {
                let l = rest_local[i];
                let (t, q) = (to_y_up(l.pos), q_up(l.rot));
                let kids: Vec<String> = r.children(i).iter().map(|c| (c + 1).to_string()).collect();
                let children = if kids.is_empty() { String::new() } else { format!(",\"children\":[{}]", kids.join(",")) };
                nodes.push(format!("{{\"name\":{},\"translation\":[{},{},{}],\"rotation\":[{},{},{},{}]{children}}}", json_str(&b.name), num(t[0]), num(t[1]), num(t[2]), num(q[0]), num(q[1]), num(q[2]), num(q[3])));
                if b.parent.is_none() {
                    scene_nodes.push((i + 1).to_string());
                }
            }
            // Inverse bind: the inverse of each joint's rest frame, y-up,
            // column-major.
            let ibm: Vec<f32> = rest
                .iter()
                .flat_map(|x| {
                    let inv = crate::rig::Xf { rot: q_up(x.rot), pos: to_y_up(x.pos), scale: 1.0 }.inverse();
                    let m = inv.matrix();
                    [m[0][0], m[1][0], m[2][0], 0.0, m[0][1], m[1][1], m[2][1], 0.0, m[0][2], m[1][2], m[2][2], 0.0, m[0][3], m[1][3], m[2][3], 1.0]
                })
                .collect();
            let iv = push_view(&mut bin, &f32s(&mut ibm.into_iter()), None);
            accessors.push(format!("{{\"bufferView\":{iv},\"componentType\":5126,\"count\":{},\"type\":\"MAT4\"}}", r.bones.len()));
            if r.is_skinned() {
                skins = format!(",\"skins\":[{{\"inverseBindMatrices\":{},\"joints\":[{}]}}]", accessors.len() - 1, (1..=r.bones.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
            }
        }
        for a in &r.actions {
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            let rest_local = r.rest_local();
            let q_up = |q: crate::rig::Quat| [q[0], -q[1], -q[2], q[3]];
            for t in &a.tracks {
                if t.times.is_empty() {
                    continue;
                }
                let (node, path, data, kind): (usize, &str, Vec<f32>, &str) = match t.channel {
                    crate::rig::Channel::Rotation(b) if b < r.bones.len() => (b + 1, "rotation", t.values.iter().flat_map(|v| q_up(crate::rig::q_mul(rest_local[b].rot, *v))).collect(), "VEC4"),
                    crate::rig::Channel::Location(b) if b < r.bones.len() => (b + 1, "translation", t.values.iter().flat_map(|v| to_y_up(rest_local[b].apply([v[0], v[1], v[2]]))).collect(), "VEC3"),
                    crate::rig::Channel::Scale(b) if b < r.bones.len() => (b + 1, "scale", t.values.iter().flat_map(|v| [v[0]; 3]).collect(), "VEC3"),
                    _ => continue,
                };
                let input = {
                    let v = push_view(&mut bin, &f32s(&mut t.times.iter().copied()), None);
                    let (lo, hi) = (t.times.first().copied().unwrap_or(0.0), t.times.last().copied().unwrap_or(0.0));
                    accessors.push(format!("{{\"bufferView\":{v},\"componentType\":5126,\"count\":{},\"type\":\"SCALAR\",\"min\":[{}],\"max\":[{}]}}", t.times.len(), num(lo), num(hi)));
                    accessors.len() - 1
                };
                let ov = push_view(&mut bin, &f32s(&mut data.into_iter()), None);
                accessors.push(format!("{{\"bufferView\":{ov},\"componentType\":5126,\"count\":{},\"type\":\"{kind}\"}}", t.times.len()));
                samplers.push(format!("{{\"input\":{input},\"output\":{},\"interpolation\":\"{}\"}}", accessors.len() - 1, if t.step { "STEP" } else { "LINEAR" }));
                channels.push(format!("{{\"sampler\":{},\"target\":{{\"node\":{node},\"path\":\"{path}\"}}}}", samplers.len() - 1));
            }
            // Shape keys: one sampler of every weight at the union of times.
            let shape_tracks: Vec<&crate::rig::Track> = a.tracks.iter().filter(|t| matches!(t.channel, crate::rig::Channel::Shape(_))).collect();
            if !shape_tracks.is_empty() && !r.shapes.is_empty() {
                let mut times: Vec<f32> = shape_tracks.iter().flat_map(|t| t.times.iter().copied()).collect();
                times.sort_by(f32::total_cmp);
                times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
                let mut data = Vec::with_capacity(times.len() * r.shapes.len());
                for &tm in &times {
                    for s in 0..r.shapes.len() {
                        data.push(shape_tracks.iter().find(|t| t.channel == crate::rig::Channel::Shape(s)).map_or(0.0, |t| t.sample(tm)[0]));
                    }
                }
                let input = {
                    let v = push_view(&mut bin, &f32s(&mut times.iter().copied()), None);
                    let (lo, hi) = (times.first().copied().unwrap_or(0.0), times.last().copied().unwrap_or(0.0));
                    accessors.push(format!("{{\"bufferView\":{v},\"componentType\":5126,\"count\":{},\"type\":\"SCALAR\",\"min\":[{}],\"max\":[{}]}}", times.len(), num(lo), num(hi)));
                    accessors.len() - 1
                };
                let ov = push_view(&mut bin, &f32s(&mut data.into_iter()), None);
                accessors.push(format!("{{\"bufferView\":{ov},\"componentType\":5126,\"count\":{},\"type\":\"SCALAR\"}}", times.len() * r.shapes.len()));
                samplers.push(format!("{{\"input\":{input},\"output\":{},\"interpolation\":\"LINEAR\"}}", accessors.len() - 1));
                channels.push(format!("{{\"sampler\":{},\"target\":{{\"node\":0,\"path\":\"weights\"}}}}", samplers.len() - 1));
            }
            if !channels.is_empty() {
                animations.push(format!("{{\"name\":{},\"samplers\":[{}],\"channels\":[{}]}}", json_str(&a.name), samplers.join(","), channels.join(",")));
            }
        }
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let textures: Vec<String> = (0..images.len()).map(|i| format!("{{\"source\":{i}}}")).collect();
    let mut json = format!(
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"Zornov\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],\"nodes\":[{}],\"meshes\":[{{\"primitives\":[{}]{mesh_extra}}}],\"materials\":[{}],\"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{}}}]{skins}",
        scene_nodes.join(","), nodes.join(","), prims.join(","), materials.join(","), accessors.join(","), views.join(","), bin.len()
    );
    if !animations.is_empty() {
        json.push_str(&format!(",\"animations\":[{}]", animations.join(",")));
    }
    if m.materials.iter().any(|x| x.transmission > 0.0) {
        json.push_str(",\"extensionsUsed\":[\"KHR_materials_transmission\",\"KHR_materials_ior\"]");
    }
    if !images.is_empty() {
        json.push_str(&format!(",\"images\":[{}],\"textures\":[{}]", images.join(","), textures.join(",")));
    }
    json.push('}');
    let mut json = json.into_bytes();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// The model as OBJ text plus its MTL (named `mtl_name` in the OBJ), and
/// the texture pictures the MTL names, as (file name, PNG bytes).
pub fn obj(m: &Mesh, mtl_name: &str) -> (String, String, Vec<(String, Vec<u8>)>) {
    use std::fmt::Write;
    let mut o = format!("# Zornov\nmtllib {mtl_name}\n");
    for p in &m.positions {
        let q = to_y_up(*p);
        let _ = writeln!(o, "v {} {} {}", q[0], q[1], q[2]);
    }
    for uv in &m.uvs {
        let _ = writeln!(o, "vt {} {}", uv[0], 1.0 - uv[1]);
    }
    for n in &m.normals {
        let q = to_y_up(*n);
        let _ = writeln!(o, "vn {} {} {}", q[0], q[1], q[2]);
    }
    let mut mtl = String::new();
    let mut pictures = Vec::new();
    let names: Vec<String> = m.materials.iter().enumerate().map(|(i, mat)| format!("m{i}_{}", mat.name.replace(char::is_whitespace, "_"))).collect();
    for (i, mat) in m.materials.iter().enumerate() {
        let c = mat.base_colour;
        let _ = writeln!(mtl, "newmtl {}\nKd {} {} {}\nd {}\nPr {}\nPm {}\nKe {} {} {}", names[i], c[0], c[1], c[2], c[3], mat.roughness, mat.metallic, mat.emissive[0], mat.emissive[1], mat.emissive[2]);
        if let Some(t) = &mat.texture {
            let file = format!("{}.png", names[i]);
            pictures.push((file.clone(), studio_media::png::encode(&studio_media::RgbaImage { width: t.width, height: t.height, pixels: t.rgba.clone() })));
            let _ = writeln!(mtl, "map_Kd {file}");
        }
        mtl.push('\n');
    }
    for g in &m.groups {
        let _ = writeln!(o, "usemtl {}", names.get(g.material as usize).map_or("m0", |s| s));
        for t in g.start..g.start + g.count {
            let [a, b, c] = m.tri(t as usize).map(|v| v + 1);
            let _ = writeln!(o, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}");
        }
    }
    (o, mtl, pictures)
}
