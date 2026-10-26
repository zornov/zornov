// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shuffle. The main input is the node's B, the second its A. p: r, g, b,
// a sources (SHUFFLE_FROM), depth (keep, A depth, B luma, A luma, none),
// motion (keep, A, none), id (keep, A, none).
fn pick(src: i32) -> f32 {
    if (src >= 12) {
        return select(0.0, 1.0, src == 13);
    }
    var px: vec4<f32>;
    var ax: vec4<f32>;
    var ch = src;
    if (src < 6) {
        px = load_a(cur);
        ax = aux_a(cur);
    } else {
        if (u.has_b < 0.5) {
            return 0.0;
        }
        px = load_b(cur);
        ax = aux_b(cur);
        ch = src - 6;
    }
    if (ch < 4) {
        return px[ch];
    }
    if (ch == 4) {
        let bit = select(32u, 4u, src < 6);
        return select(0.0, ax.x, (u32(u.has_aux) & bit) != 0u);
    }
    return luma(px.rgb) + 0.0 * px.a;
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let ma = aux_a(cur);
    let sa = aux_b(cur);
    var o = ma;
    let d = i32(u.p[4].x + 0.5);
    if (d == 1) {
        o.x = sa.x;
    } else if (d == 2) {
        o.x = luma(load_a(cur).rgb);
    } else if (d == 3) {
        o.x = luma(load_b(cur).rgb);
    } else if (d == 4) {
        o.x = 0.0;
    }
    let m = i32(u.p[5].x + 0.5);
    if (m == 1) {
        o.y = sa.y;
        o.z = sa.z;
    } else if (m == 2) {
        o.y = 0.0;
        o.z = 0.0;
    }
    let idc = i32(u.p[6].x + 0.5);
    if (idc == 1) {
        o.w = sa.w;
    } else if (idc == 2) {
        o.w = 0.0;
    }
    aux_out = o;
    return vec4<f32>(pick(i32(u.p[0].x + 0.5)), pick(i32(u.p[1].x + 0.5)), pick(i32(u.p[2].x + 0.5)), pick(i32(u.p[3].x + 0.5)));
}
