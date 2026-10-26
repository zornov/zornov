// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Particles: draws the packed buffer `particles::pack` writes (simulated on
// the CPU, closed-form and deterministic). Words:
//   0 tiles across, 1 tiles down, 2 tile size, 3 particle count,
//   4 offset of the particle data, 8.. one offset per tile and an end,
//   then particle indices per tile (far to near), then 16 words per
//   particle: x, y, size, angle, r, g, b, a (premultiplied; glows add),
//   tail x, tail y, aspect, sprite, extra, -, glow, depth.
// The sprite formulas are `particles::coverage`'s.

@group(0) @binding(4) var<storage, read> data: array<u32>;

fn wf(i: u32) -> f32 {
    return bitcast<f32>(data[i]);
}

fn bump(d: f32, r: f32) -> f32 {
    let t = clamp(1.0 - d / max(r, 1.0e-4), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xf = floor(x);
    let yf = floor(y);
    let xi = i32(xf);
    let yi = i32(yf);
    let tx = fade(x - xf);
    let ty = fade(y - yf);
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    return mix(mix(a, b, tx), mix(c, d, tx), ty);
}

// Coverage and shade of a sprite at offset `d` from its centre.
fn sprite(kind: u32, dv: vec2<f32>, size: f32, angle: f32, tail: vec2<f32>, aspect: f32, extra: f32) -> vec2<f32> {
    let s = sin(angle);
    let c = cos(angle);
    let lx = dv.x * c + dv.y * s;
    let ly = -dv.x * s + dv.y * c;
    let d = length(dv);
    switch kind {
        case 0u: {
            let b = bump(d, size);
            return vec2<f32>(b * b, 1.0);
        }
        case 2u: {
            let b = bump(d, size);
            return vec2<f32>(b * (0.6 + 0.4 * b), 1.0);
        }
        case 1u, 4u: {
            let l2 = dot(tail, tail);
            var t = 0.0;
            if (l2 > 1.0e-6) {
                t = clamp(dot(dv, tail) / l2, 0.0, 1.0);
            }
            let e = dv - t * tail;
            let b = bump(length(e), size);
            if (kind == 4u) {
                return vec2<f32>(b * b * (1.0 - 0.8 * t), 1.0);
            }
            return vec2<f32>(b * (1.0 - 0.7 * t), 1.0);
        }
        case 3u: {
            let minor = max(size * aspect, 0.5);
            let qx = lx / size;
            let qy = ly / minor;
            let q = sqrt(qx * qx + qy * qy);
            let cov = clamp((1.0 - q) * minor, 0.0, 1.0);
            let rib = 1.0 - 0.35 * bump(abs(ly), max(0.12 * minor, 0.5));
            return vec2<f32>(cov, (0.75 + 0.3 * clamp(lx / size, -1.0, 1.0)) * rib);
        }
        case 5u: {
            let b = bump(d, size);
            let n = vnoise(lx / size * 1.7 + extra, ly / size * 1.7 + extra * 0.7, 0x5eedu);
            return vec2<f32>(b * b * (0.45 + 0.55 * n), 1.0);
        }
        case 6u: {
            let ring = bump(abs(d - 0.85 * size), max(0.15 * size, 0.7));
            let body = 0.08 * bump(d, size);
            let hl = bump(length(dv + vec2<f32>(0.4 * size, 0.4 * size)), max(0.22 * size, 0.6));
            return vec2<f32>(min(0.8 * ring + body + hl, 1.0), 1.0);
        }
        case 7u: {
            let hw = max(size * aspect, 0.4);
            let hh = size * 0.6;
            let qx = abs(lx) - hw;
            let qy = abs(ly) - hh;
            let outside = length(vec2<f32>(max(qx, 0.0), max(qy, 0.0)));
            let sd = outside + min(max(qx, qy), 0.0);
            return vec2<f32>(clamp(0.5 - sd, 0.0, 1.0), 0.7 + 0.3 * aspect);
        }
        case 8u: {
            let core = bump(d, size * 0.35);
            let thin = max(size * 0.06, 0.5);
            let glare = bump(abs(ly), thin) * bump(abs(lx), size) + bump(abs(lx), thin) * bump(abs(ly), size);
            return vec2<f32>(min(core * core + 0.6 * glare, 1.0), 1.0);
        }
        default: {
            let qy = dv.y / max(aspect, 0.1);
            let q = sqrt(dv.x * dv.x + qy * qy);
            let rr = size * (0.3 + 0.7 * extra);
            let b = bump(abs(q - rr), max(size * 0.12, 0.8));
            return vec2<f32>(b * (1.0 - extra), 1.0);
        }
    }
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    var acc = tex(pos);
    if (data[3] == 0u) {
        return acc;
    }
    let tx = data[0];
    let tile = data[2];
    let tiles = tx * data[1];
    let index_base = 8u + tiles + 1u;
    let base_data = data[4];
    let ix = u32(pos.x);
    let iy = u32(pos.y);
    let t = (iy / tile) * tx + ix / tile;
    let a = data[8u + t];
    let b = data[8u + t + 1u];
    for (var i = a; i < b; i = i + 1u) {
        let p = base_data + data[index_base + i] * 16u;
        let sc = sprite(u32(wf(p + 11u)), pos - vec2<f32>(wf(p), wf(p + 1u)), wf(p + 2u), wf(p + 3u), vec2<f32>(wf(p + 8u), wf(p + 9u)), wf(p + 10u), wf(p + 12u));
        let cov = sc.x;
        if (cov <= 0.0) {
            continue;
        }
        let col = vec4<f32>(wf(p + 4u) * sc.y * cov, wf(p + 5u) * sc.y * cov, wf(p + 6u) * sc.y * cov, wf(p + 7u) * cov);
        if (wf(p + 14u) > 0.5) {
            let l = col.rgb;
            acc = vec4<f32>(acc.rgb + l, max(acc.a, min(max(l.r, max(l.g, l.b)), 1.0)));
        } else {
            acc = col + acc * (1.0 - col.a);
        }
    }
    return acc;
}
