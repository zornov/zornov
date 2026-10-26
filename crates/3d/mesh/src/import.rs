// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Models from other programs: glTF 2.0 (`.glb`, `.gltf` with its buffers
//! and pictures beside it or embedded), Wavefront OBJ (with its `.mtl`)
//! and STL (binary or text). Each is turned into Studio's world (y down,
//! z away), centred, and scaled to a size that fits the canvas.

use crate::{Material, Mesh, Texture, V3, ops};
use std::path::Path;

/// What came in, and anything that was left out on the way.
pub struct Imported {
    pub mesh: Mesh,
    pub notes: Vec<String>,
}

/// The file extensions [`load`] reads.
pub const EXTENSIONS: &[&str] = &["glb", "gltf", "obj", "stl", "ply", "fbx", "smesh"];

/// Reads a model file. `fit` (pixels) scales its largest side to that size
/// after centring it; 0 keeps the file's own size and place.
pub fn load(path: &Path, fit: f32) -> Result<Imported, String> {
    let mut ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // What the bytes say wins over the name (a model saved as ".bin" or
    // ".download", a ".glb" that is really an FBX).
    if let Some(real) = crate::foreign::sniff(&bytes) {
        ext = real.to_owned();
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut out = match ext.as_str() {
        "glb" | "gltf" => gltf(&bytes, dir)?,
        "obj" => obj(&String::from_utf8_lossy(&bytes), dir),
        "stl" => Imported { mesh: stl(&bytes)?, notes: Vec::new() },
        "ply" => {
            let (mesh, notes) = crate::foreign::ply(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            Imported { mesh, notes }
        }
        "fbx" => {
            let (mesh, notes) = crate::foreign::fbx(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            Imported { mesh, notes }
        }
        "smesh" => Imported { mesh: crate::io::from_bytes(&bytes)?, notes: Vec::new() },
        other => {
            return Err(match crate::foreign::unsupported(other) {
                Some(way) => format!("{}: {way}", path.display()),
                None => format!("{}: .{other} is not a model Zornov reads (glb, gltf, obj, stl, ply, fbx)", path.display()),
            })
        }
    };
    if out.mesh.triangles() == 0 {
        return Err(format!("{}: no triangles in the file", path.display()));
    }
    out.mesh.tidy();
    if fit > 0.0 {
        ops::centre_and_fit(&mut out.mesh, fit);
    }
    Ok(out)
}

/// y-up files (glTF, OBJ): Studio's y is down and its z is away.
fn from_y_up(p: V3) -> V3 {
    [p[0], -p[1], -p[2]]
}

/// z-up files (most STL): the model's front looks along -y.
fn from_z_up(p: V3) -> V3 {
    [p[0], -p[2], p[1]]
}

fn decode_picture(bytes: &[u8]) -> Result<Texture, String> {
    let img = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        studio_media::png::decode(bytes).map_err(|e| format!("{e:?}"))?
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        studio_media::jpeg::decode(bytes)?
    } else {
        return Err("only PNG and JPEG textures are read".into());
    };
    Ok(Texture { width: img.width, height: img.height, rgba: img.pixels })
}

fn base64(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return Err("bad base64 in an embedded buffer".into()),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

fn uri_bytes(uri: &str, dir: &Path) -> Result<Vec<u8>, String> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,").ok_or("an embedded buffer is not base64")?;
        return base64(data);
    }
    let name = percent_decode(uri);
    // Only files beside the model (no climbing out of its folder).
    if name.contains("..") || name.starts_with('/') || name.contains("://") {
        return Err(format!("\"{uri}\" is outside the model's folder"));
    }
    std::fs::read(dir.join(&name)).map_err(|e| format!("{name}: {e}"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Column-major, as glTF keeps them: (a·b)[c][r] = Σ a[k][r]·b[c][k].
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}

fn gltf(bytes: &[u8], dir: &Path) -> Result<Imported, String> {
    let file = gltf::Gltf::from_slice(bytes).map_err(|e| format!("not a readable glTF file: {e}"))?;
    let doc = &file.document;
    let mut notes = Vec::new();
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for b in doc.buffers() {
        buffers.push(match b.source() {
            gltf::buffer::Source::Bin => file.blob.clone().ok_or("the GLB has no binary chunk")?,
            gltf::buffer::Source::Uri(uri) => uri_bytes(uri, dir)?,
        });
    }
    let view_bytes = |v: &gltf::buffer::View| -> Option<Vec<u8>> {
        let b = buffers.get(v.buffer().index())?;
        b.get(v.offset()..v.offset() + v.length()).map(|s| s.to_vec())
    };
    let mut pictures: Vec<Option<Texture>> = Vec::new();
    for img in doc.images() {
        let data = match img.source() {
            gltf::image::Source::View { view, .. } => view_bytes(&view).ok_or_else(|| "a texture points past its buffer".to_string()),
            gltf::image::Source::Uri { uri, .. } => uri_bytes(uri, dir),
        };
        pictures.push(match data.and_then(|d| decode_picture(&d)) {
            Ok(t) => Some(t),
            Err(e) => {
                notes.push(format!("texture {} left out: {e}", img.index()));
                None
            }
        });
    }
    let mut mesh = Mesh::default();
    let mut mats: Vec<u32> = Vec::new();
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let c = pbr.base_color_factor();
        let texture = pbr.base_color_texture().and_then(|info| pictures.get(info.texture().source().index()).cloned().flatten());
        mesh.materials.push(Material {
            name: m.name().unwrap_or("Material").to_string(),
            base_colour: c,
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            emissive: m.emissive_factor(),
            subsurface: 0.0,
            transmission: m.transmission().map_or(0.0, |t| t.transmission_factor()),
            ior: m.ior().unwrap_or(1.5),
            texture,
        });
    }
    let default_mat = mesh.materials.len() as u32;
    mesh.materials.push(Material::default());
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let roots: Vec<gltf::Node> = match &scene {
        Some(s) => s.nodes().collect(),
        None => {
            let kids: std::collections::HashSet<usize> = doc.nodes().flat_map(|n| n.children().map(|c| c.index())).collect();
            doc.nodes().filter(|n| !kids.contains(&n.index())).collect()
        }
    };
    // Every node's parent and its place in the world at rest.
    let node_count = doc.nodes().count();
    let mut parent_of: Vec<Option<usize>> = vec![None; node_count];
    let mut global: Vec<[[f32; 4]; 4]> = vec![identity; node_count];
    let mut order: Vec<usize> = Vec::new();
    {
        let mut stack: Vec<(gltf::Node, [[f32; 4]; 4], Option<usize>)> = roots.iter().map(|n| (n.clone(), identity, None)).collect();
        let mut seen = vec![false; node_count];
        while let Some((node, parent, p)) = stack.pop() {
            if std::mem::replace(&mut seen[node.index()], true) {
                continue;
            }
            let world = mat4_mul(&parent, &node.transform().matrix());
            global[node.index()] = world;
            parent_of[node.index()] = p;
            order.push(node.index());
            stack.extend(node.children().map(|c| (c, world, Some(node.index()))));
        }
    }
    // Joints of every skin, as bones in the order found.
    let mut joint_bone: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut joint_nodes: Vec<usize> = Vec::new();
    for skin in doc.skins() {
        for j in skin.joints() {
            if !joint_bone.contains_key(&j.index()) {
                joint_bone.insert(j.index(), joint_nodes.len());
                joint_nodes.push(j.index());
            }
        }
    }
    let mut skin_joints: Vec<[u16; 4]> = Vec::new();
    let mut skin_weights: Vec<[f32; 4]> = Vec::new();
    let mut any_skin = false;
    // Morph targets: (name, per-vertex offsets) laid over the whole mesh.
    let mut morphs: Vec<(String, Vec<(u32, V3)>)> = Vec::new();
    let mut morph_of_mesh: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    let mut skipped_modes = false;
    for &ni in &order {
        let Some(node) = doc.nodes().nth(ni) else { continue };
        let world = global[ni];
        let Some(gm) = node.mesh() else { continue };
        let skin = node.skin();
        // Normals take the inverse transpose; for the rotations and
        // uniform scales most files use, the matrix itself is close enough
        // once renormalised.
        let apply_m = |m: &[[f32; 4]; 4], p: [f32; 3], w: f32| -> V3 { std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r] * w) };
        let apply = |p: [f32; 3], w: f32| -> V3 { apply_m(&world, p, w) };
        let det = world[0][0] * (world[1][1] * world[2][2] - world[2][1] * world[1][2]) - world[1][0] * (world[0][1] * world[2][2] - world[2][1] * world[0][2]) + world[2][0] * (world[0][1] * world[1][2] - world[1][1] * world[0][2]);
        // A skinned mesh is placed by its joints: joint world × inverse bind.
        let joint_mats: Option<(Vec<[[f32; 4]; 4]>, Vec<usize>)> = skin.as_ref().map(|s| {
            let ibm: Vec<[[f32; 4]; 4]> = s.reader(|b| buffers.get(b.index()).map(|v| v.as_slice())).read_inverse_bind_matrices().map(|it| it.collect()).unwrap_or_default();
            let joints: Vec<usize> = s.joints().map(|j| j.index()).collect();
            let mats = joints.iter().enumerate().map(|(k, &j)| mat4_mul(&global[j], ibm.get(k).unwrap_or(&identity))).collect();
            (mats, joints)
        });
        let names = target_names(gm.extras());
        let mut mesh_morphs: Vec<usize> = Vec::new();
        for prim in gm.primitives() {
            let mode = prim.mode();
            if !matches!(mode, gltf::mesh::Mode::Triangles | gltf::mesh::Mode::TriangleStrip | gltf::mesh::Mode::TriangleFan) {
                skipped_modes = true;
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|v| v.as_slice()));
            let Some(positions) = reader.read_positions() else { continue };
            let base = mesh.positions.len() as u32;
            let start = mesh.positions.len();
            let raw: Vec<[f32; 3]> = positions.collect();
            let count = raw.len();
            // Skin per vertex (joint indices into this skin's list).
            let (vj, vw): (Vec<[u16; 4]>, Vec<[f32; 4]>) = match (&joint_mats, reader.read_joints(0), reader.read_weights(0)) {
                (Some(_), Some(j), Some(w)) => (j.into_u16().collect(), w.into_f32().collect()),
                _ => (Vec::new(), Vec::new()),
            };
            let skinned = joint_mats.is_some() && vj.len() == count && vw.len() == count;
            let place = |i: usize, p: [f32; 3], w: f32| -> V3 {
                if skinned && let Some((mats, _)) = &joint_mats {
                    let mut acc = [0.0f32; 3];
                    let mut tot = 0.0;
                    for k in 0..4 {
                        let wk = vw[i][k];
                        if wk <= 0.0 {
                            continue;
                        }
                        if let Some(m) = mats.get(vj[i][k] as usize) {
                            let q = apply_m(m, p, w);
                            acc = crate::add(acc, crate::scale(q, wk));
                            tot += wk;
                        }
                    }
                    if tot > 0.0 {
                        return crate::scale(acc, 1.0 / tot);
                    }
                }
                apply(p, w)
            };
            mesh.positions.extend(raw.iter().enumerate().map(|(i, p)| from_y_up(place(i, *p, 1.0))));
            match reader.read_normals() {
                Some(n) => mesh.normals.extend(n.enumerate().map(|(i, n)| crate::normalise(from_y_up(place(i, n, 0.0))))),
                None => mesh.normals.extend(std::iter::repeat_n([0.0; 3], count)),
            }
            let uv_set = prim.material().pbr_metallic_roughness().base_color_texture().map_or(0, |i| i.tex_coord());
            match reader.read_tex_coords(uv_set) {
                Some(uv) => mesh.uvs.extend(uv.into_f32()),
                None => mesh.uvs.extend(std::iter::repeat_n([0.0; 2], count)),
            }
            mesh.normals.resize(mesh.positions.len(), [0.0; 3]);
            mesh.uvs.resize(mesh.positions.len(), [0.0; 2]);
            // Skin weights onto rig bones (vertices of unskinned parts later
            // follow their node's bone, or the first).
            skin_joints.resize(start, [u16::MAX; 4]);
            skin_weights.resize(start, [0.0; 4]);
            if skinned && let Some((_, joints)) = &joint_mats {
                any_skin = true;
                for i in 0..count {
                    let j: [u16; 4] = std::array::from_fn(|k| joints.get(vj[i][k] as usize).and_then(|n| joint_bone.get(n)).copied().unwrap_or(0) as u16);
                    skin_joints.push(j);
                    skin_weights.push(vw[i]);
                }
            } else {
                // Rigidly on the nearest joint above this node.
                let mut up = Some(ni);
                let mut bone = None;
                while let Some(n) = up {
                    if let Some(b) = joint_bone.get(&n) {
                        bone = Some(*b);
                        break;
                    }
                    up = parent_of[n];
                }
                for _ in 0..count {
                    skin_joints.push(bone.map_or([u16::MAX; 4], |b| [b as u16, 0, 0, 0]));
                    skin_weights.push([1.0, 0.0, 0.0, 0.0]);
                }
            }
            // Morph targets of this primitive, by index within the mesh.
            for (ti, (pos, _, _)) in reader.read_morph_targets().enumerate() {
                let Some(pos) = pos else { continue };
                let name = names.get(ti).cloned().unwrap_or_else(|| format!("shape{ti}"));
                let slot = match mesh_morphs.get(ti) {
                    Some(&k) => k,
                    None => {
                        let k = morphs.iter().position(|(n, _)| *n == name).unwrap_or_else(|| {
                            morphs.push((name.clone(), Vec::new()));
                            morphs.len() - 1
                        });
                        mesh_morphs.push(k);
                        k
                    }
                };
                for (i, d) in pos.enumerate() {
                    if d != [0.0; 3] && i < count {
                        morphs[slot].1.push((base + i as u32, from_y_up(place(i, d, 0.0))));
                    }
                }
            }
            let list: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..count as u32).collect(),
            };
            let mut tris: Vec<[u32; 3]> = match mode {
                gltf::mesh::Mode::TriangleStrip => (2..list.len()).map(|k| if k % 2 == 0 { [list[k - 2], list[k - 1], list[k]] } else { [list[k - 1], list[k - 2], list[k]] }).collect(),
                gltf::mesh::Mode::TriangleFan => (2..list.len()).map(|k| [list[0], list[k - 1], list[k]]).collect(),
                _ => list.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            };
            if det < 0.0 && !skinned {
                // A mirroring transform turns faces inside out; turn them back.
                for t in &mut tris {
                    t.swap(1, 2);
                }
            }
            let mat = prim.material().index().map_or(default_mat, |i| i as u32);
            for t in tris {
                if t.iter().all(|&i| (i as usize) < count) {
                    mesh.indices.extend(t.map(|i| i + base));
                    mats.push(mat);
                }
            }
        }
        morph_of_mesh.insert(gm.index(), mesh_morphs);
    }
    if skipped_modes {
        notes.push("points and lines left out (only surfaces are models)".into());
    }
    if !joint_nodes.is_empty() || !morphs.is_empty() {
        let rig = gltf_rig(doc, &buffers, &global, &parent_of, &joint_nodes, &joint_bone, any_skin.then_some((skin_joints, skin_weights)), &morphs, &morph_of_mesh, mesh.positions.len(), &mut notes);
        mesh.rig = Some(std::sync::Arc::new(rig));
    } else if doc.animations().next().is_some() {
        notes.push("animation of whole objects left out: the model comes in at rest; animate it with Zornov's 3D transform".into());
    }
    let missing_normals = mesh.normals.iter().all(|n| *n == [0.0; 3]);
    mesh.regroup(&mats);
    if missing_normals {
        mesh.normals.clear();
    }
    Ok(Imported { mesh, notes })
}

/// `targetNames` from a glTF mesh's extras (how exporters name morph
/// targets), read without a JSON library.
fn target_names(extras: &gltf::json::Extras) -> Vec<String> {
    let Some(raw) = extras.as_ref() else { return Vec::new() };
    let text = raw.get();
    let Some(at) = text.find("\"targetNames\"") else { return Vec::new() };
    let rest = &text[at + 13..];
    let Some(open) = rest.find('[') else { return Vec::new() };
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in rest[open + 1..].chars() {
        if in_str {
            if esc {
                cur.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_str = true;
        } else if c == ']' {
            break;
        }
    }
    out
}

/// glTF y-up rotations into Studio's frame (half a turn about x).
fn quat_from_y_up(q: crate::rig::Quat) -> crate::rig::Quat {
    [q[0], -q[1], -q[2], q[3]]
}

/// The rotation part of a column-major matrix (scale taken out).
fn mat_rotation(m: &[[f32; 4]; 4]) -> crate::rig::Quat {
    let x = crate::normalise([m[0][0], m[0][1], m[0][2]]);
    let y0 = [m[1][0], m[1][1], m[1][2]];
    let y = crate::normalise(crate::sub(y0, crate::scale(x, crate::dot(y0, x))));
    let mut z = crate::cross(x, y);
    if crate::dot(z, [m[2][0], m[2][1], m[2][2]]) < 0.0 {
        // Mirrored: keep a proper rotation.
        z = crate::scale(z, 1.0);
    }
    crate::rig::q_from_axes(x, y, z)
}

/// The rig from a glTF file's skins, morph targets and animations.
#[allow(clippy::too_many_arguments)]
fn gltf_rig(
    doc: &gltf::Document,
    buffers: &[Vec<u8>],
    global: &[[[f32; 4]; 4]],
    parent_of: &[Option<usize>],
    joint_nodes: &[usize],
    joint_bone: &std::collections::HashMap<usize, usize>,
    skin: Option<(Vec<[u16; 4]>, Vec<[f32; 4]>)>,
    morphs: &[(String, Vec<(u32, V3)>)],
    morph_of_mesh: &std::collections::HashMap<usize, Vec<usize>>,
    vertices: usize,
    notes: &mut Vec<String>,
) -> crate::rig::Rig {
    use crate::rig::{Action, Bone, BonePose, Channel, Rig, ShapeKey, Track, q_mul, q_normalise};
    let pos_of = |m: &[[f32; 4]; 4]| from_y_up([m[3][0], m[3][1], m[3][2]]);
    let mut rig = Rig::default();
    // Bones: head at the joint, tail along the joint's Y axis as far as its
    // nearest child joint (or half its parent's length at the ends).
    let joint_parent = |n: usize| -> Option<usize> {
        let mut up = parent_of[n];
        while let Some(p) = up {
            if let Some(b) = joint_bone.get(&p) {
                return Some(*b);
            }
            up = parent_of[p];
        }
        None
    };
    let mut lens = vec![0.0f32; joint_nodes.len()];
    for (b, &n) in joint_nodes.iter().enumerate() {
        let head = pos_of(&global[n]);
        let kids: Vec<V3> = joint_nodes.iter().filter(|&&c| joint_parent(c) == Some(b)).map(|&c| pos_of(&global[c])).collect();
        lens[b] = kids.iter().map(|k| crate::length(crate::sub(*k, head))).filter(|l| *l > 1e-6).fold(f32::INFINITY, f32::min);
    }
    for (b, &n) in joint_nodes.iter().enumerate() {
        let head = pos_of(&global[n]);
        let len = if lens[b].is_finite() { lens[b] } else { joint_parent(n).map_or(0.1, |p| if lens[p].is_finite() { lens[p] * 0.5 } else { 0.1 }) };
        let m = &global[n];
        let ydir = crate::normalise(from_y_up([m[1][0], m[1][1], m[1][2]]));
        let ydir = if ydir == [0.0; 3] { [0.0, -1.0, 0.0] } else { ydir };
        let name = doc.nodes().nth(n).and_then(|x| x.name().map(str::to_owned)).unwrap_or_else(|| format!("bone{b}"));
        rig.bones.push(Bone::new(name, joint_parent(n), head, crate::add(head, crate::scale(ydir, len.max(1e-4)))));
    }
    if let Some((mut j, w)) = skin {
        for jj in &mut j {
            for k in jj.iter_mut() {
                if *k == u16::MAX {
                    *k = 0;
                }
            }
        }
        j.resize(vertices, [0; 4]);
        let mut w = w;
        w.resize(vertices, [1.0, 0.0, 0.0, 0.0]);
        rig.joints = j;
        rig.weights = w;
    }
    for (name, offsets) in morphs {
        let mut k = ShapeKey { name: name.clone(), indices: Vec::new(), deltas: Vec::new() };
        let mut list = offsets.clone();
        list.sort_by_key(|e| e.0);
        for (i, d) in list {
            k.indices.push(i);
            k.deltas.push(d);
        }
        rig.shapes.push(k);
    }
    // Bones' parents must come first; joints in skins usually do already.
    let unsorted = rig.bones.iter().enumerate().any(|(i, b)| b.parent.is_some_and(|p| p >= i));
    if unsorted {
        rig.sort();
    }
    // Animations: sampled 30 times a second, each joint's turn in the
    // world since rest carried onto its bone.
    let rest_world = rig.rest_world();
    let node_count = global.len();
    for anim in doc.animations() {
        let name = anim.name().map(str::to_owned).unwrap_or_else(|| format!("Animation {}", anim.index() + 1));
        let mut end = 0.0f32;
        let mut chans: Vec<(usize, gltf::animation::Property, Vec<f32>, Vec<Vec<f32>>, gltf::animation::Interpolation)> = Vec::new();
        for ch in anim.channels() {
            let reader = ch.reader(|b| buffers.get(b.index()).map(|v| v.as_slice()));
            let Some(times) = reader.read_inputs() else { continue };
            let times: Vec<f32> = times.collect();
            let Some(outs) = reader.read_outputs() else { continue };
            use gltf::animation::util::ReadOutputs;
            let values: Vec<Vec<f32>> = match outs {
                ReadOutputs::Translations(t) => t.map(|v| v.to_vec()).collect(),
                ReadOutputs::Rotations(r) => r.into_f32().map(|v| v.to_vec()).collect(),
                ReadOutputs::Scales(s) => s.map(|v| v.to_vec()).collect(),
                ReadOutputs::MorphTargetWeights(w) => {
                    let all: Vec<f32> = w.into_f32().collect();
                    let per = (all.len() / times.len().max(1)).max(1);
                    all.chunks(per).map(|c| c.to_vec()).collect()
                }
            };
            end = end.max(times.last().copied().unwrap_or(0.0));
            chans.push((ch.target().node().index(), ch.target().property(), times, values, ch.sampler().interpolation()));
        }
        if chans.is_empty() || end <= 0.0 {
            continue;
        }
        let sample = |times: &[f32], values: &[Vec<f32>], interp: gltf::animation::Interpolation, t: f32, rot: bool| -> Vec<f32> {
            // Cubic splines keep (in-tangent, value, out-tangent) per key.
            let cubic = interp == gltf::animation::Interpolation::CubicSpline;
            let val = |k: usize| -> &Vec<f32> { if cubic { &values[(3 * k + 1).min(values.len() - 1)] } else { &values[k.min(values.len() - 1)] } };
            let n = times.len();
            if n == 0 || values.is_empty() {
                return Vec::new();
            }
            if t <= times[0] || n == 1 {
                return val(0).clone();
            }
            if t >= times[n - 1] {
                return val(n - 1).clone();
            }
            let i = times.partition_point(|&k| k <= t).max(1) - 1;
            let (a, b) = (val(i), val(i + 1));
            if interp == gltf::animation::Interpolation::Step {
                return a.clone();
            }
            let u = (t - times[i]) / (times[i + 1] - times[i]).max(1e-9);
            if rot && a.len() == 4 && b.len() == 4 {
                return crate::rig::q_slerp([a[0], a[1], a[2], a[3]], [b[0], b[1], b[2], b[3]], u).to_vec();
            }
            a.iter().zip(b).map(|(x, y)| x + (y - x) * u).collect()
        };
        let fps = 30.0;
        let frames = (end * fps).ceil() as usize;
        let mut poses: Vec<Vec<BonePose>> = Vec::with_capacity(frames + 1);
        let mut shape_tracks: Vec<Vec<f32>> = vec![Vec::new(); rig.shapes.len()];
        let root_joints: Vec<usize> = (0..rig.bones.len()).filter(|&b| rig.bones[b].parent.is_none()).collect();
        for f in 0..=frames {
            let t = (f as f32 / fps).min(end);
            // Local TRS of every node at t.
            let mut trs: Vec<([f32; 3], [f32; 4], [f32; 3])> = doc.nodes().map(|n| n.transform().decomposed()).collect();
            let mut weights: Vec<Option<Vec<f32>>> = vec![None; node_count];
            for (node, prop, times, values, interp) in &chans {
                let v = sample(times, values, *interp, t, *prop == gltf::animation::Property::Rotation);
                let Some(slot) = trs.get_mut(*node) else { continue };
                match prop {
                    gltf::animation::Property::Translation if v.len() >= 3 => slot.0 = [v[0], v[1], v[2]],
                    gltf::animation::Property::Rotation if v.len() >= 4 => slot.1 = [v[0], v[1], v[2], v[3]],
                    gltf::animation::Property::Scale if v.len() >= 3 => slot.2 = [v[0], v[1], v[2]],
                    gltf::animation::Property::MorphTargetWeights => weights[*node] = Some(v),
                    _ => {}
                }
            }
            // Globals in parent-first order.
            let mut g: Vec<Option<[[f32; 4]; 4]>> = vec![None; node_count];
            fn global_of(i: usize, g: &mut [Option<[[f32; 4]; 4]>], trs: &[([f32; 3], [f32; 4], [f32; 3])], parent_of: &[Option<usize>]) -> [[f32; 4]; 4] {
                if let Some(m) = g[i] {
                    return m;
                }
                let (t, r, s) = trs[i];
                let local = gltf::scene::Transform::Decomposed { translation: t, rotation: r, scale: s }.matrix();
                let m = match parent_of[i] {
                    Some(p) => mat4_mul(&global_of(p, g, trs, parent_of), &local),
                    None => local,
                };
                g[i] = Some(m);
                m
            }
            let mut world_rot: Vec<crate::rig::Quat> = Vec::with_capacity(rig.bones.len());
            for (b, &n) in joint_nodes.iter().enumerate().take(rig.bones.len()) {
                let now = global_of(n, &mut g, &trs, parent_of);
                let turn = quat_from_y_up(q_mul(mat_rotation(&now), crate::rig::q_conj(mat_rotation(&global[n]))));
                world_rot.push(q_normalise(q_mul(turn, rest_world[b].rot)));
            }
            let root = root_joints.first().map(|&b| pos_of(&global_of(joint_nodes[b], &mut g, &trs, parent_of)));
            if !joint_nodes.is_empty() {
                poses.push(rig.local_from_world(&world_rot, root));
            }
            // Morph weights: from whichever node animates its mesh's.
            for (ni, w) in weights.iter().enumerate() {
                let Some(w) = w else { continue };
                let Some(mesh_index) = doc.nodes().nth(ni).and_then(|n| n.mesh()).map(|m| m.index()) else { continue };
                if let Some(slots) = morph_of_mesh.get(&mesh_index) {
                    for (ti, &slot) in slots.iter().enumerate() {
                        if let (Some(v), Some(track)) = (w.get(ti), shape_tracks.get_mut(slot))
                            && track.len() == f
                        {
                            track.push(*v);
                        }
                    }
                }
            }
        }
        let mut action = if poses.is_empty() { Action { name: name.clone(), duration: end, tracks: Vec::new() } } else { Action::from_frames(name.clone(), fps, &poses) };
        action.duration = end;
        let times: Vec<f32> = (0..=frames).map(|f| (f as f32 / fps).min(end)).collect();
        for (slot, values) in shape_tracks.into_iter().enumerate() {
            if values.len() == times.len() && values.iter().any(|v| v.abs() > 1e-5) {
                action.tracks.push(Track { channel: Channel::Shape(slot), times: times.clone(), values: values.into_iter().map(|v| [v, 0.0, 0.0, 0.0]).collect(), step: false });
            }
        }
        rig.actions.push(action);
    }
    if !rig.actions.is_empty() {
        notes.push(format!("{} animation{} kept: {}", rig.actions.len(), if rig.actions.len() == 1 { "" } else { "s" }, rig.actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if !rig.bones.is_empty() {
        notes.push(format!("rigged: {} bones{}", rig.bones.len(), if rig.is_skinned() { ", skinned" } else { "" }));
    }
    if !rig.shapes.is_empty() {
        notes.push(format!("{} shape keys", rig.shapes.len()));
    }
    rig
}

fn obj(text: &str, dir: &Path) -> Imported {
    let mut notes = Vec::new();
    let (mut vs, mut vts, mut vns): (Vec<V3>, Vec<[f32; 2]>, Vec<V3>) = (Vec::new(), Vec::new(), Vec::new());
    let mut mesh = Mesh::default();
    let mut mats: Vec<u32> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut current = 0u32;
    let mut library: Vec<Material> = Vec::new();
    let mut seen: std::collections::HashMap<(i64, i64, i64), u32> = std::collections::HashMap::new();
    let num = |s: Option<&str>| s.and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    let mut any_normal = true;
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => vs.push(from_y_up([num(w.next()), num(w.next()), num(w.next())])),
            Some("vt") => {
                let (u, v) = (num(w.next()), num(w.next()));
                vts.push([u, 1.0 - v]);
            }
            Some("vn") => vns.push(from_y_up([num(w.next()), num(w.next()), num(w.next())])),
            Some("mtllib") => {
                let name = line.trim_start()[6..].trim();
                match std::fs::read_to_string(dir.join(name)) {
                    Ok(t) => library.extend(mtl(&t, dir, &mut notes)),
                    Err(e) => notes.push(format!("{name}: {e}; using grey")),
                }
            }
            Some("usemtl") => {
                let name = w.next().unwrap_or("").to_string();
                current = match names.iter().position(|n| *n == name) {
                    Some(i) => i as u32,
                    None => {
                        names.push(name);
                        (names.len() - 1) as u32
                    }
                };
            }
            Some("f") => {
                let corner: Vec<u32> = w
                    .filter_map(|c| {
                        let mut p = c.split('/');
                        let idx = |s: Option<&str>, n: usize| -> i64 {
                            match s.and_then(|v| v.parse::<i64>().ok()) {
                                Some(i) if i < 0 => n as i64 + i,
                                Some(i) => i - 1,
                                None => -1,
                            }
                        };
                        let vi = idx(p.next(), vs.len());
                        let ti = idx(p.next(), vts.len());
                        let ni = idx(p.next(), vns.len());
                        if vi < 0 || vi as usize >= vs.len() {
                            return None;
                        }
                        Some(*seen.entry((vi, ti, ni)).or_insert_with(|| {
                            mesh.positions.push(vs[vi as usize]);
                            mesh.uvs.push(if ti >= 0 && (ti as usize) < vts.len() { vts[ti as usize] } else { [0.0; 2] });
                            if ni >= 0 && (ni as usize) < vns.len() {
                                mesh.normals.push(vns[ni as usize]);
                            } else {
                                any_normal = false;
                                mesh.normals.push([0.0; 3]);
                            }
                            (mesh.positions.len() - 1) as u32
                        }))
                    })
                    .collect();
                for k in 1..corner.len().saturating_sub(1) {
                    mesh.indices.extend_from_slice(&[corner[0], corner[k], corner[k + 1]]);
                    mats.push(current);
                }
            }
            _ => {}
        }
    }
    if names.is_empty() {
        names.push(String::new());
    }
    mesh.materials = names.iter().map(|n| library.iter().find(|m| m.name == *n).cloned().unwrap_or_else(|| Material { name: if n.is_empty() { "Default".into() } else { n.clone() }, ..Material::default() })).collect();
    mesh.regroup(&mats);
    if !any_normal {
        mesh.normals.clear();
    }
    Imported { mesh, notes }
}

fn mtl(text: &str, dir: &Path, notes: &mut Vec<String>) -> Vec<Material> {
    let mut out: Vec<Material> = Vec::new();
    let num = |s: Option<&str>| s.and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let key = w.next();
        if key == Some("newmtl") {
            out.push(Material { name: w.next().unwrap_or("").to_string(), ..Material::default() });
            continue;
        }
        let Some(m) = out.last_mut() else { continue };
        match key {
            Some("Kd") => {
                let c = [num(w.next()), num(w.next()), num(w.next())];
                m.base_colour = [c[0], c[1], c[2], m.base_colour[3]];
            }
            Some("d") => m.base_colour[3] = num(w.next()),
            Some("Tr") => m.base_colour[3] = 1.0 - num(w.next()),
            Some("Ns") => m.roughness = (1.0 - (num(w.next()) / 1000.0).sqrt()).clamp(0.04, 1.0),
            Some("Pr") => m.roughness = num(w.next()),
            Some("Pm") => m.metallic = num(w.next()),
            Some("Ke") => m.emissive = [num(w.next()), num(w.next()), num(w.next())],
            Some("map_Kd") => {
                // The file name is the last word (options come first).
                if let Some(name) = line.split_whitespace().last() {
                    match uri_bytes(name, dir).and_then(|b| decode_picture(&b)) {
                        Ok(t) => {
                            m.texture = Some(t);
                            m.base_colour = [1.0, 1.0, 1.0, m.base_colour[3]];
                        }
                        Err(e) => notes.push(format!("{name}: {e}")),
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn stl(bytes: &[u8]) -> Result<Mesh, String> {
    let mut mesh = Mesh::default();
    let binary_len = bytes.get(80..84).map(|c| u32::from_le_bytes(c.try_into().expect("4")) as usize);
    let is_binary = binary_len.is_some_and(|n| 84 + n * 50 == bytes.len()) || !bytes.starts_with(b"solid");
    if is_binary {
        let n = binary_len.ok_or("the STL file is cut short")?;
        if 84 + n * 50 > bytes.len() {
            return Err("the STL file is cut short".into());
        }
        let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4"));
        for t in 0..n {
            let o = 84 + t * 50 + 12;
            for k in 0..3 {
                let p = o + k * 12;
                mesh.positions.push(from_z_up([f(p), f(p + 4), f(p + 8)]));
                mesh.indices.push(mesh.indices.len() as u32);
            }
        }
    } else {
        for line in String::from_utf8_lossy(bytes).lines() {
            let mut w = line.split_whitespace();
            if w.next() == Some("vertex") {
                let v: Vec<f32> = w.filter_map(|s| s.parse().ok()).collect();
                if v.len() == 3 {
                    mesh.positions.push(from_z_up([v[0], v[1], v[2]]));
                    mesh.indices.push(mesh.indices.len() as u32);
                }
            }
        }
        mesh.indices.truncate(mesh.indices.len() / 3 * 3);
    }
    mesh.single_group();
    // STL has no shared corners: weld them, then smooth with sharp edges.
    mesh.normals = vec![[0.0; 3]; mesh.positions.len()];
    mesh.uvs = vec![[0.0; 2]; mesh.positions.len()];
    ops::weld(&mut mesh);
    ops::recompute_normals(&mut mesh, 40.0);
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_with_quads_and_negative_indices() {
        let text = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\nf -4/-4 -2/-2 -1/-1\n";
        let m = obj(text, Path::new(".")).mesh;
        assert_eq!(m.triangles(), 3);
        // y-up to y-down.
        assert!(m.positions.iter().any(|p| p[1] == -1.0));
        assert!(m.uvs.iter().any(|uv| uv[1] == 1.0));
    }

    #[test]
    fn binary_stl_round_trip_through_bytes() {
        let mut b = vec![0u8; 80];
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in [0.0f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0, 0]);
        let m = stl(&b).unwrap();
        assert_eq!(m.triangles(), 1);
    }

    #[test]
    fn glb_written_by_export_reads_back() {
        let mut m = crate::prim::torus(100.0, 30.0, 24, 12);
        m.materials[0].base_colour = [0.9, 0.1, 0.1, 1.0];
        m.materials[0].texture = Some(Texture { width: 2, height: 2, rgba: vec![255; 16] });
        let glb = crate::export::glb(&m);
        let back = gltf(&glb, Path::new(".")).unwrap();
        assert_eq!(back.mesh.triangles(), m.triangles());
        let (lo, hi) = back.mesh.bounds();
        let (lo0, hi0) = m.bounds();
        for i in 0..3 {
            assert!((lo[i] - lo0[i]).abs() < 0.01 && (hi[i] - hi0[i]).abs() < 0.01);
        }
        assert_eq!(back.mesh.materials[0].base_colour, [0.9, 0.1, 0.1, 1.0]);
        assert!(back.mesh.materials[0].texture.is_some());
    }

    #[test]
    fn a_rigged_character_goes_out_and_back_through_glb() {
        let mut m = crate::character::character(300.0, &crate::character::Build { detail: 1, ..Default::default() });
        let rig = std::sync::Arc::make_mut(m.rig.as_mut().unwrap());
        rig.actions.push(crate::motion::make(rig, "walk", &crate::motion::Style::default()).unwrap());
        let walk_at = |mesh: &Mesh, t: f32| {
            let r = mesh.rig.as_ref().unwrap();
            let p = r.evaluate(&crate::rig::PoseInput { action: Some(("walk".into(), t, true)), ..Default::default() });
            // Heads: a leaf bone's length is not in the file.
            r.bone_lines(&p)[r.bone("toe.L").unwrap()].0
        };
        let dir = std::env::temp_dir().join(format!("studio-rig-glb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("walker.glb");
        std::fs::write(&path, crate::export::glb(&m)).unwrap();
        let back = load(&path, 0.0).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let r = back.mesh.rig.as_ref().expect("rig kept");
        let src = m.rig.as_ref().unwrap();
        assert_eq!(r.bones.len(), src.bones.len(), "{:?}", back.notes);
        assert_eq!(r.shapes.len(), src.shapes.len());
        assert!(r.is_skinned());
        assert!(r.action("walk").is_some(), "{:?}", back.notes);
        // Same mesh at rest, same step mid-walk.
        assert!(crate::length(crate::sub(back.mesh.bounds().0, m.bounds().0)) < 0.5);
        let (a, b) = (walk_at(&m, 0.3), walk_at(&back.mesh, 0.3));
        assert!(crate::length(crate::sub(a, b)) < 2.0, "{a:?} vs {b:?}");
    }
}
