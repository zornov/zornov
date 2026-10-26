// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Matte nodes, chosen by OP (defined before this file): 0 id matte,
// 1 depth matte, 2 luma matte, 3 copy alpha, 4 despill.
fn matte_out(q: vec4<f32>, m0: f32, out: i32) -> vec4<f32> {
    let m = clamp(m0, 0.0, 1.0);
    if (out == 1) {
        return vec4<f32>(m, m, m, m);
    }
    if (out == 2) {
        return vec4<f32>(straight(q) * m, m);
    }
    return q * m;
}

fn spill_limit(c: vec3<f32>, dom: i32, method: i32, balance: f32) -> f32 {
    var rl = c.r;
    var bl = c.b;
    if (dom == 2) {
        rl = c.r;
        bl = c.g;
    }
    if (method == 1) {
        return balance * rl + (1.0 - balance) * bl;
    }
    if (method == 2) {
        return max(rl, bl);
    }
    return 0.5 * (rl + bl);
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let q = load_a(cur);
    let ax = aux_a(cur);
    let flags = u32(u.has_aux);
    switch OP {
        case 0: {
            var m = 0.0;
            if ((flags & 16u) != 0u && abs(ax.w - u.p[0].x) < 0.5) {
                m = 1.0;
            }
            return matte_out(q, m, i32(u.p[1].x + 0.5));
        }
        case 1: {
            let inv = u.p[3].x > 0.5;
            var k = 0.0;
            if ((flags & 4u) != 0u) {
                let z = ax.x;
                let near = u.p[0].x;
                let far = u.p[1].x;
                let soft = max(u.p[2].x, 1e-6);
                k = sstep(near - soft, near, z) * (1.0 - sstep(far, far + soft, z));
            }
            if (inv) {
                k = 1.0 - k;
            }
            return matte_out(q, k, i32(u.p[4].x + 0.5));
        }
        case 2: {
            let c = straight(q);
            let l = srgb_encode(clamp(luma(c), 0.0, 1.0));
            var k = sstep(u.p[0].x, max(u.p[1].x, u.p[0].x + 1e-4), l);
            if (u.p[2].x > 0.5) {
                k = 1.0 - k;
            }
            return matte_out(q, k, i32(u.p[3].x + 0.5));
        }
        case 3: {
            if (u.has_b < 0.5) {
                return q;
            }
            var m = clamp(load_b(cur).a, 0.0, 1.0);
            if (u.p[1].x > 0.5) {
                m = 1.0 - m;
            }
            let c = straight(q);
            let ba = q.a;
            let op = i32(u.p[0].x + 0.5);
            var na = m;
            if (op == 1) {
                na = ba * m;
            } else if (op == 2) {
                na = min(ba + m, 1.0);
            } else if (op == 3) {
                na = max(ba - m, 0.0);
            }
            return vec4<f32>(c * na, na);
        }
        default: {
            if (q.a <= 1e-6) {
                return q;
            }
            var c = q.rgb / q.a;
            var dom = 1;
            if (u.p[0].x > 0.5) {
                dom = 2;
            }
            let limit = spill_limit(c, dom, i32(u.p[1].x + 0.5), u.p[2].x);
            let spill = max(c[dom] - limit, 0.0) * u.p[3].x;
            c[dom] = c[dom] - spill;
            var lw = 0.7152;
            if (dom == 2) {
                lw = 0.0722;
            }
            c = c + vec3<f32>(u.p[4].x * spill * lw);
            let rep = u.p[5];
            if (rep.a > 0.0) {
                c = c + spill * (rep.rgb / max(rep.a, 1e-6)) * rep.a;
            }
            return vec4<f32>(c * q.a, q.a);
        }
    }
}
