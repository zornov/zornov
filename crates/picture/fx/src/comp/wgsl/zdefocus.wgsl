// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Depth of field from the depth channel. p: focus, focus at point,
// focus point, depth of field, size, shape (0 disc, 1 gaussian), quality,
// output (0 result, 1 focus map).
fn coc(z: f32, focus: f32, dof: f32, size: f32) -> f32 {
    if (!(z > 0.0) || !(focus > 0.0)) {
        return 0.0;
    }
    let half = dof * 0.5;
    if (abs(z - focus) <= half) {
        return 0.0;
    }
    var zz = z + half;
    if (z > focus) {
        zz = z - half;
    }
    return min(size * abs(1.0 - focus / zz), size);
}

fn has_depth() -> bool {
    return (u32(u.has_aux) & 4u) != 0u;
}

fn depth_at(q: vec2<i32>) -> f32 {
    if (has_depth()) {
        return aux_a(q).x;
    }
    return 3.0e38;
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let size = u.p[4].x;
    let dof = u.p[3].x;
    var focus = u.p[0].x;
    if (u.p[1].x > 0.5 && has_depth()) {
        let fp = u.p[2].xy * u.size;
        // Bilinear, as the CPU's plane sample.
        let p = clamp(fp - vec2<f32>(0.5), vec2<f32>(0.0), u.size - vec2<f32>(1.0));
        let p0 = floor(p);
        let f = p - p0;
        let i0 = vec2<i32>(p0);
        let a = mix(aux_a(i0).x, aux_a(i0 + vec2<i32>(1, 0)).x, f.x);
        let b = mix(aux_a(i0 + vec2<i32>(0, 1)).x, aux_a(i0 + vec2<i32>(1, 1)).x, f.x);
        focus = mix(a, b, f.y);
    }
    let cz = depth_at(cur);
    var cc = size;
    if (has_depth()) {
        cc = coc(cz, focus, dof, size);
    }
    if (u.p[7].x > 0.5) {
        let v = cc / max(size, 1e-6);
        return vec4<f32>(v, v, v, 1.0);
    }
    if (size <= 0.25) {
        return load_a(cur);
    }
    let n = max(i32(u.p[6].x + 0.5), 8);
    let gaussian = u.p[5].x > 0.5;
    let golden = 3.14159265 * (3.0 - sqrt(5.0));
    let wc = 1.0 / (cc * cc + 1.0);
    var acc = load_a(cur) * wc;
    var wsum = wc;
    for (var i = 0; i < n; i = i + 1) {
        let r = sqrt((f32(i) + 0.5) / f32(n));
        let ang = f32(i) * golden;
        let o = vec2<f32>(r * cos(ang), r * sin(ang)) * size;
        let d = length(o);
        let sp = clamp(round(vec2<f32>(cur) + o), vec2<f32>(0.0), u.size - vec2<f32>(1.0));
        let j = vec2<i32>(sp);
        let zj = depth_at(j);
        var sc = size;
        if (has_depth()) {
            sc = coc(zj, focus, dof, size);
            if (zj > cz) {
                sc = min(sc, cc);
            }
        }
        if (sc < d) {
            continue;
        }
        var wt = 1.0 / (sc * sc + 1.0);
        if (gaussian) {
            let k = d / max(sc, 1e-3);
            wt = wt * exp(-2.0 * k * k);
        }
        acc = acc + load_a(j) * wt;
        wsum = wsum + wt;
    }
    return acc / max(wsum, 1e-9);
}
