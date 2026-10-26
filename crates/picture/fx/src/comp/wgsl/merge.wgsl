// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Merge: the main input (binding A here) is the node's B, the second
// input is the node's A (the foreground). p[0].x operation (MERGE_OPS).
fn merge_px(op: i32, a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    let aa = a.a;
    let ba = b.a;
    let uni = aa + ba - aa * ba;
    switch op {
        case 1: { return b + a * (1.0 - ba); }
        case 2: { return a + b; }
        case 3: { return a - b; }
        case 4: { return b - a; }
        case 5: { return vec4<f32>(a.rgb * b.rgb, uni); }
        case 6: { return vec4<f32>(a.rgb + b.rgb - a.rgb * b.rgb, uni); }
        case 7: {
            let lo = 2.0 * a.rgb * b.rgb;
            let hi = vec3<f32>(1.0) - 2.0 * (vec3<f32>(1.0) - a.rgb) * (vec3<f32>(1.0) - b.rgb);
            return vec4<f32>(select(hi, lo, b.rgb < vec3<f32>(0.5)), uni);
        }
        case 8: { return vec4<f32>(abs(a.rgb - b.rgb), uni); }
        case 9: { return a * ba; }
        case 10: { return a * (1.0 - ba); }
        case 11: { return a * ba + b * (1.0 - aa); }
        case 12: { return a * (1.0 - ba) + b * (1.0 - aa); }
        case 13: { return b * aa; }
        case 14: { return b * (1.0 - aa); }
        case 15: { return max(a, b); }
        case 16: { return min(a, b); }
        case 17: { return (a + b) * 0.5; }
        case 18: { return a; }
        default: { return a + b * (1.0 - aa); }
    }
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let bg = load_a(cur);
    if (u.has_b < 0.5) {
        return bg;
    }
    let fg = load_b(cur);
    let flags = u32(u.has_aux);
    let ma = aux_a(cur);
    let sa = aux_b(cur);
    let covered = fg.a > 0.5;
    var o = ma;
    // Depth: the nearer surface where the foreground covers.
    if ((flags & 32u) != 0u) {
        if ((flags & 4u) != 0u) {
            if (covered) {
                o.x = min(ma.x, sa.x);
            }
        } else {
            o.x = sa.x;
        }
    }
    if ((flags & 8u) == 0u && (flags & 64u) != 0u) {
        o.y = sa.y;
        o.z = sa.z;
    }
    if ((flags & 128u) != 0u) {
        if (covered) {
            o.w = sa.w;
        } else if ((flags & 16u) == 0u) {
            o.w = 0.0;
        }
    }
    aux_out = o;
    return merge_px(i32(u.p[0].x + 0.5), fg, bg);
}
