// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Small per-pixel nodes, chosen by OP (defined before this file):
// 0 invert, 1 clamp, 2 premult, 3 unpremult, 4 crop, 5 constant, 6 ramp.
fn node(pos: vec2<f32>) -> vec4<f32> {
    let q = load_a(cur);
    switch OP {
        case 0: {
            let ch = i32(u.p[0].x + 0.5);
            let c = straight(q);
            let a = q.a;
            let a2 = select(a, 1.0 - clamp(a, 0.0, 1.0), ch != 1);
            if (ch == 0) {
                return vec4<f32>(c * a2, a2);
            }
            return vec4<f32>((vec3<f32>(1.0) - c) * a2, a2);
        }
        case 1: {
            let lo = u.p[0].x;
            let hi = max(u.p[1].x, lo);
            return vec4<f32>(clamp(q.rgb, vec3<f32>(lo), vec3<f32>(hi)), clamp(q.a, 0.0, 1.0));
        }
        case 2: { return vec4<f32>(q.rgb * q.a, q.a); }
        case 3: {
            if (q.a > 1e-6) {
                return vec4<f32>(q.rgb / q.a, q.a);
            }
            return q;
        }
        case 4: {
            let x0 = u.p[0].x * u.size.x;
            let y0 = u.p[0].y * u.size.y;
            let x1 = u.p[1].x * u.size.x;
            let y1 = u.p[1].y * u.size.y;
            let soft = max(u.p[2].x, 1e-3);
            let d = min(min(pos.x - x0, x1 - pos.x), min(pos.y - y0, y1 - pos.y));
            return q * clamp(d / soft + 0.5, 0.0, 1.0);
        }
        case 5: { return u.p[0]; }
        default: {
            let a = u.p[0].xy * u.size;
            let b = u.p[1].xy * u.size;
            let d = b - a;
            let len2 = max(dot(d, d), 1e-6);
            let qq = pos - a;
            var t = dot(qq, d) / len2;
            if (u.p[4].x > 0.5) {
                t = sqrt(dot(qq, qq) / len2);
            }
            t = clamp(t, 0.0, 1.0);
            // Colours arrive premultiplied: mix their straight values.
            let ca = vec4<f32>(u.p[2].rgb / max(u.p[2].a, 1e-6), u.p[2].a);
            let cb = vec4<f32>(u.p[3].rgb / max(u.p[3].a, 1e-6), u.p[3].a);
            let c = mix(ca, cb, t);
            return vec4<f32>(c.rgb * c.a, c.a);
        }
    }
}
