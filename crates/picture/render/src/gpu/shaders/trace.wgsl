// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// The path tracer on the GPU. Mirrors crates/picture/render/src/trace/cpu.rs
// function for function; see there for the method. One invocation is one
// pixel; each dispatch adds one sample per pixel to the running sums.

struct U {
    cam_pos: vec4<f32>,    // xyz, zoom
    cam_right: vec4<f32>,  // xyz, centre x
    cam_down: vec4<f32>,   // xyz, centre y
    cam_fwd: vec4<f32>,    // xyz, lens radius
    misc: vec4<f32>,       // focus, extent, haze, env turn (radians)
    haze_col: vec4<f32>,   // rgb, emit total
    counts0: vec4<u32>,    // width, height, lights, emitters
    counts1: vec4<u32>,    // volumes, env w, env h, flags (1 moving, 2 background, 4 env)
    offs0: vec4<u32>,      // pos1, normals, uvs, materials (vec4 offsets)
    offs1: vec4<u32>,      // lights, emitters, volumes, env (data offsets)
    offs2: vec4<u32>,      // volume cells (word offset), cdf cols offset, nodes, sample
    sets: vec4<f32>,       // bounces, clamp, shutter on, 0
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> geo: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> tris: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> nodes: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> data: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> words: array<u32>;
@group(0) @binding(6) var<storage, read> cdf: array<f32>;
@group(0) @binding(7) var<storage, read_write> accum: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read_write> aux: array<vec4<f32>>;

const PI: f32 = 3.14159265;
const HAIR: u32 = 1u;
const UNLIT: u32 = 2u;
const NO_SHADOW: u32 = 4u;
const INF: f32 = 1e30;

var<private> rng_state: u32;

fn rng() -> f32 {
    rng_state = rng_state * 747796405u + 2891336453u;
    var w = ((rng_state >> ((rng_state >> 28u) + 4u)) ^ rng_state) * 277803737u;
    w = (w >> 22u) ^ w;
    return f32(w >> 8u) / 16777216.0;
}

fn seed(px: u32, py: u32, s: u32) {
    rng_state = (px * 1973u + py * 9277u + s * 26699u) | 1u;
    rng();
    rng();
}

fn lum(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

fn corner(v: u32, time: f32) -> vec3<f32> {
    let a = geo[v].xyz;
    if ((u.counts1.w & 1u) == 0u || time <= 0.0) {
        return a;
    }
    let b = geo[u.offs0.x + v].xyz;
    return a + (b - a) * time;
}

struct Tri { a: vec3<f32>, b: vec3<f32>, c: vec3<f32> };

fn tri_at(t: u32, time: f32) -> Tri {
    let c = tris[t];
    return Tri(corner(c.x, time), corner(c.y, time), corner(c.z, time));
}

// Möller–Trumbore: (t, u, v), t < 0 for a miss.
fn ray_tri(tr: Tri, o: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let e1 = tr.b - tr.a;
    let e2 = tr.c - tr.a;
    let p = cross(d, e2);
    let det = dot(e1, p);
    if (abs(det) < 1e-12) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
    let inv = 1.0 / det;
    let s = o - tr.a;
    let uu = dot(s, p) * inv;
    if (uu < 0.0 || uu > 1.0) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
    let q = cross(s, e1);
    let vv = dot(d, q) * inv;
    if (vv < 0.0 || uu + vv > 1.0) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
    return vec3<f32>(dot(e2, q) * inv, uu, vv);
}

struct Hit { t: f32, tri: u32, u: f32, v: f32, ok: bool };

fn safe_inv(x: f32) -> f32 {
    if (abs(x) > 1e-12) {
        return 1.0 / x;
    }
    return select(-1e12, 1e12, x >= 0.0);
}

fn intersect(o: vec3<f32>, d: vec3<f32>, t_min: f32, t_max: f32, time: f32) -> Hit {
    var best = Hit(t_max, 0u, 0.0, 0.0, false);
    if (u.offs2.z == 0u) {
        return best;
    }
    let inv = vec3<f32>(safe_inv(d.x), safe_inv(d.y), safe_inv(d.z));
    var stack: array<u32, 64>;
    var sp = 1u;
    stack[0] = 0u;
    var far = t_max;
    loop {
        if (sp == 0u) {
            break;
        }
        sp = sp - 1u;
        let ni = stack[sp];
        let n0 = nodes[2u * ni];
        let n1 = nodes[2u * ni + 1u];
        let a = (n0.xyz - o) * inv;
        let b = (n1.xyz - o) * inv;
        let t0 = max(t_min, max(max(min(a.x, b.x), min(a.y, b.y)), min(a.z, b.z)));
        let t1 = min(far, min(min(max(a.x, b.x), max(a.y, b.y)), max(a.z, b.z)));
        if (t0 > t1) {
            continue;
        }
        let index = bitcast<u32>(n0.w);
        let count = bitcast<u32>(n1.w);
        if (count > 0u) {
            for (var t = index; t < index + count; t = t + 1u) {
                let r = ray_tri(tri_at(t, time), o, d);
                if (r.x > t_min && r.x < far) {
                    far = r.x;
                    best = Hit(r.x, t, r.y, r.z, true);
                }
            }
        } else if (sp + 2u <= 64u) {
            stack[sp] = index;
            stack[sp + 1u] = ni + 1u;
            sp = sp + 2u;
        }
    }
    return best;
}

struct Mat { m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, m3: vec4<f32> };

fn mat(i: u32) -> Mat {
    let b = u.offs0.w + 4u * i;
    return Mat(data[b], data[b + 1u], data[b + 2u], data[b + 3u]);
}

fn srgb(v: u32) -> f32 {
    let c = f32(v) / 255.0;
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn texture(m: Mat, uv: vec2<f32>) -> vec4<f32> {
    let size = bitcast<u32>(m.m3.y);
    if (size == 0u) {
        return vec4<f32>(1.0);
    }
    let w = size & 0xffffu;
    let h = size >> 16u;
    let off = bitcast<u32>(m.m3.x);
    let x = min(u32((uv.x - floor(uv.x)) * f32(w)), w - 1u);
    let y = min(u32((uv.y - floor(uv.y)) * f32(h)), h - 1u);
    let p = words[off + y * w + x];
    return vec4<f32>(srgb(p & 0xffu), srgb((p >> 8u) & 0xffu), srgb((p >> 16u) & 0xffu), f32(p >> 24u) / 255.0);
}

struct Surface {
    p: vec3<f32>,
    ng: vec3<f32>,
    n: vec3<f32>,
    front: bool,
    base: vec4<f32>,
    metallic: f32,
    roughness: f32,
    transmission: f32,
    ior: f32,
    emissive: vec3<f32>,
    subsurface: f32,
    flags: u32,
    object: u32,
};

fn surface(h: Hit, o: vec3<f32>, d: vec3<f32>, time: f32) -> Surface {
    let c = tris[h.tri];
    let m = mat(c.w & 0xffffu);
    let tr = tri_at(h.tri, time);
    let p = o + d * h.t;
    let ng = normalize(cross(tr.b - tr.a, tr.c - tr.a));
    let w = 1.0 - h.u - h.v;
    let nb = u.offs0.y;
    let sn0 = geo[nb + c.x].xyz * w + geo[nb + c.y].xyz * h.u + geo[nb + c.z].xyz * h.v;
    var n = ng;
    let smooth_k = m.m3.z;
    if (dot(sn0, sn0) > 1e-12) {
        var sn = normalize(sn0);
        if (dot(sn, ng) < 0.0) {
            sn = -sn;
        }
        n = normalize(ng * (1.0 - smooth_k) + sn * smooth_k);
    }
    let front = dot(ng, d) < 0.0;
    if (dot(n, d) > 0.0) {
        n = -n;
    }
    let ub = u.offs0.z;
    let uvs = geo[ub + c.x].xy * w + geo[ub + c.y].xy * h.u + geo[ub + c.z].xy * h.v;
    let tex = texture(m, uvs);
    return Surface(p, ng, n, front, m.m0 * tex, m.m1.x, m.m1.y, m.m1.z, m.m1.w, m.m2.xyz, m.m2.w, bitcast<u32>(m.m3.w), c.w >> 16u);
}

fn frame_t(n: vec3<f32>) -> vec3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    return vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
}

fn frame_b(n: vec3<f32>) -> vec3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    return vec3<f32>(b, s + n.y * n.y * a, -n.y);
}

fn cosine_dir(n: vec3<f32>, u1: f32, u2: f32) -> vec3<f32> {
    let r = sqrt(u1);
    let phi = 2.0 * PI * u2;
    return normalize(frame_t(n) * (r * cos(phi)) + frame_b(n) * (r * sin(phi)) + n * sqrt(max(1.0 - u1, 0.0)));
}

fn uniform_sphere(u1: f32, u2: f32) -> vec3<f32> {
    let z = 1.0 - 2.0 * u1;
    let r = sqrt(max(1.0 - z * z, 0.0));
    let phi = 2.0 * PI * u2;
    return vec3<f32>(r * cos(phi), r * sin(phi), z);
}

fn ggx_d(ndh: f32, a2: f32) -> f32 {
    let k = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / max(PI * k * k, 1e-12);
}

fn smith_g1(ndv: f32, a2: f32) -> f32 {
    return 2.0 * ndv / max(ndv + sqrt(a2 + (1.0 - a2) * ndv * ndv), 1e-12);
}

fn schlick(f0: vec3<f32>, c: f32) -> vec3<f32> {
    let k = pow(clamp(1.0 - c, 0.0, 1.0), 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * k;
}

fn fresnel_dielectric(cos_i: f32, eta: f32) -> f32 {
    let sin_t2 = max(1.0 - cos_i * cos_i, 0.0) / (eta * eta);
    if (sin_t2 >= 1.0) {
        return 1.0;
    }
    let cos_t = sqrt(1.0 - sin_t2);
    let rs = (cos_i - eta * cos_t) / (cos_i + eta * cos_t);
    let rp = (eta * cos_i - cos_t) / (eta * cos_i + cos_t);
    return 0.5 * (rs * rs + rp * rp);
}

struct Lobes { f0: vec3<f32>, diffuse: vec3<f32>, a2: f32, p_spec: f32, p_diff: f32, p_trans: f32 };

fn lobes(sf: Surface, ndv: f32) -> Lobes {
    let base = sf.base.xyz;
    let df0 = pow((sf.ior - 1.0) / (sf.ior + 1.0), 2.0);
    let f0 = vec3<f32>(df0) + (base - vec3<f32>(df0)) * sf.metallic;
    let hair = (sf.flags & HAIR) != 0u;
    var rough = sf.roughness;
    if (hair) {
        rough = max(rough, 0.3);
    }
    let a = max(rough * rough, 1e-4);
    let fr = lum(schlick(f0, ndv));
    let opaque = (1.0 - sf.metallic) * (1.0 - sf.transmission);
    let diffuse = base * opaque;
    let ws = clamp(fr, 0.04, 1.0);
    let wd = lum(diffuse) * (1.0 - fr);
    let wt = (1.0 - sf.metallic) * sf.transmission * max(1.0 - fr, 0.2) + select(0.0, 0.25, hair);
    let total = max(ws + wd + wt, 1e-6);
    return Lobes(f0, diffuse, a * a, ws / total, wd / total, wt / total);
}

struct Eval { f: vec3<f32>, pdf: f32 };

fn eval(sf: Surface, l: Lobes, wo: vec3<f32>, wi: vec3<f32>) -> Eval {
    let n = sf.n;
    let ndl = dot(n, wi);
    let ndv = max(dot(n, wo), 1e-4);
    if (ndl <= 0.0) {
        return Eval(vec3<f32>(0.0), 0.0);
    }
    let h = normalize(wi + wo);
    let ndh = max(dot(n, h), 0.0);
    let vdh = max(dot(wo, h), 1e-4);
    let d = ggx_d(ndh, l.a2);
    let g = smith_g1(ndv, l.a2) * smith_g1(ndl, l.a2);
    let f = schlick(l.f0, vdh);
    let spec = f * (d * g / max(4.0 * ndv * ndl, 1e-6));
    let diff = l.diffuse * (1.0 - lum(f)) / PI;
    return Eval((spec + diff) * ndl, l.p_spec * d * ndh / (4.0 * vdh) + l.p_diff * ndl / PI);
}

fn power(a: f32, b: f32) -> f32 {
    let a2 = a * a;
    let b2 = b * b;
    if (a2 + b2 <= 0.0) {
        return 0.0;
    }
    return a2 / (a2 + b2);
}

// ------------------------------------------------------------- volumes

struct Vol { r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>, dims: vec3<u32>, offset: u32, colour: vec3<f32>, density: f32, fire: f32, lo: vec3<f32>, hi: vec3<f32> };

fn vol(i: u32) -> Vol {
    let b = u.offs1.z + 7u * i;
    let d = bitcast<vec4<u32>>(data[b + 3u]);
    let c = data[b + 4u];
    let e = data[b + 5u];
    return Vol(data[b], data[b + 1u], data[b + 2u], d.xyz, d.w, c.xyz, c.w, e.x, vec3<f32>(e.y, e.z, e.w), vec3<f32>(data[b + 6u].xyz));
}

fn vol_cell(v: Vol, x: u32, y: u32, z: u32) -> vec2<f32> {
    let idx = v.offset + (z * v.dims.y + y) * v.dims.x + x;
    let wd = words[u.offs2.x + idx / 2u];
    var pair = wd & 0xffffu;
    if (idx % 2u == 1u) {
        pair = wd >> 16u;
    }
    return vec2<f32>(f32(pair & 0xffu) / 255.0, f32(pair >> 8u) / 255.0);
}

fn volume_at(v: Vol, p: vec3<f32>) -> vec2<f32> {
    let b = vec3<f32>(dot(v.r0.xyz, p) + v.r0.w, dot(v.r1.xyz, p) + v.r1.w, dot(v.r2.xyz, p) + v.r2.w);
    if (any(b < vec3<f32>(0.0)) || any(b >= vec3<f32>(1.0))) {
        return vec2<f32>(0.0);
    }
    let dims = vec3<f32>(v.dims);
    let g = b * dims - vec3<f32>(0.5);
    let i0 = min(vec3<u32>(max(g, vec3<f32>(0.0))), v.dims - vec3<u32>(1u));
    let i1 = min(i0 + vec3<u32>(1u), v.dims - vec3<u32>(1u));
    let f = clamp(g - vec3<f32>(i0), vec3<f32>(0.0), vec3<f32>(1.0));
    let c000 = vol_cell(v, i0.x, i0.y, i0.z);
    let c100 = vol_cell(v, i1.x, i0.y, i0.z);
    let c010 = vol_cell(v, i0.x, i1.y, i0.z);
    let c110 = vol_cell(v, i1.x, i1.y, i0.z);
    let c001 = vol_cell(v, i0.x, i0.y, i1.z);
    let c101 = vol_cell(v, i1.x, i0.y, i1.z);
    let c011 = vol_cell(v, i0.x, i1.y, i1.z);
    let c111 = vol_cell(v, i1.x, i1.y, i1.z);
    let x0 = mix(mix(c000, c100, f.x), mix(c010, c110, f.x), f.y);
    let x1 = mix(mix(c001, c101, f.x), mix(c011, c111, f.x), f.y);
    return mix(x0, x1, f.z);
}

fn box_span(lo: vec3<f32>, hi: vec3<f32>, o: vec3<f32>, d: vec3<f32>, t_max: f32) -> vec2<f32> {
    let inv = vec3<f32>(safe_inv(d.x), safe_inv(d.y), safe_inv(d.z));
    let a = (lo - o) * inv;
    let b = (hi - o) * inv;
    let t0 = max(0.0, max(max(min(a.x, b.x), min(a.y, b.y)), min(a.z, b.z)));
    let t1 = min(t_max, min(min(max(a.x, b.x), max(a.y, b.y)), max(a.z, b.z)));
    return vec2<f32>(t0, t1);
}

fn volume_transmittance(o: vec3<f32>, d: vec3<f32>, dist: f32) -> f32 {
    var tr = 1.0;
    for (var i = 0u; i < u.counts1.x; i = i + 1u) {
        let v = vol(i);
        let span = box_span(v.lo, v.hi, o, d, min(dist, 1e9));
        if (span.x >= span.y) {
            continue;
        }
        var t = span.x;
        loop {
            t = t - log(1.0 - rng()) / max(v.density, 1e-6);
            if (t >= span.y) {
                break;
            }
            tr = tr * (1.0 - volume_at(v, o + d * t).x);
            if (tr < 1e-3) {
                return 0.0;
            }
        }
    }
    return tr;
}

fn visibility(from_p: vec3<f32>, d: vec3<f32>, dist: f32, time: f32) -> vec3<f32> {
    var t = vec3<f32>(1.0);
    let eps = u.misc.y * 1e-5 + 1e-3;
    var o = from_p;
    var left = dist;
    for (var k = 0; k < 8; k = k + 1) {
        let h = intersect(o, d, eps, left - eps, time);
        if (!h.ok) {
            break;
        }
        let m = mat(tris[h.tri].w & 0xffffu);
        let flags = bitcast<u32>(m.m3.w);
        if ((flags & NO_SHADOW) == 0u) {
            let alpha = m.m0.w;
            let pass_k = (1.0 - alpha) + alpha * m.m1.z * (1.0 - m.m1.x);
            if (pass_k <= 0.0) {
                return vec3<f32>(0.0);
            }
            t = t * (vec3<f32>(1.0 - alpha) + alpha * m.m1.z * m.m0.xyz);
        }
        o = o + d * h.t;
        left = left - h.t;
        if (lum(t) < 1e-4) {
            return vec3<f32>(0.0);
        }
    }
    return t * volume_transmittance(from_p, d, dist);
}

fn fire_glow(o: vec3<f32>, d: vec3<f32>, t_end: f32) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    for (var i = 0u; i < u.counts1.x; i = i + 1u) {
        let v = vol(i);
        if (v.fire <= 0.0) {
            continue;
        }
        let span = box_span(v.lo, v.hi, o, d, min(t_end, 1e9));
        if (span.x >= span.y) {
            continue;
        }
        let ds = (span.y - span.x) / 32.0;
        var optical = 0.0;
        for (var k = 0; k < 32; k = k + 1) {
            let p = o + d * (span.x + (f32(k) + 0.5) * ds);
            let s = volume_at(v, p);
            if (s.y > 0.0) {
                out = out + fire_colour(s.y) * (v.fire * s.y * 4.0 * v.density * ds * exp(-optical));
            }
            optical = optical + s.x * v.density * ds;
        }
    }
    return out;
}

fn fire_colour(heat: f32) -> vec3<f32> {
    let h = clamp(heat, 0.0, 1.0);
    return vec3<f32>(h * 1.6, h * h * 1.1, h * h * h * h * 0.6);
}

// --------------------------------------------------------- environment

fn env_uv(d: vec3<f32>) -> vec2<f32> {
    let n = normalize(d);
    let phi = atan2(n.x, n.z) + u.misc.w;
    let uu = fract(phi / (2.0 * PI) + 0.5);
    let vv = acos(clamp(n.y, -1.0, 1.0)) / PI;
    return vec2<f32>(uu, 1.0 - vv);
}

fn env_dir(uu: f32, vv: f32) -> vec3<f32> {
    let phi = (uu - 0.5) * 2.0 * PI - u.misc.w;
    let theta = (1.0 - vv) * PI;
    let r = sin(theta);
    return vec3<f32>(r * sin(phi), cos(theta), r * cos(phi));
}

fn env_at(d: vec3<f32>) -> vec3<f32> {
    if ((u.counts1.w & 4u) == 0u) {
        return vec3<f32>(0.0);
    }
    let uv = env_uv(d);
    let w = u.counts1.y;
    let h = u.counts1.z;
    let x = min(u32(uv.x * f32(w)), w - 1u);
    let y = min(u32(uv.y * f32(h)), h - 1u);
    return data[u.offs1.w + y * w + x].xyz;
}

fn env_pdf_xy(x: u32, y: u32) -> f32 {
    let w = u.counts1.y;
    let h = u.counts1.z;
    let total = cdf[h];
    let row = u.offs2.y + y * (w + 1u);
    let p = (cdf[row + x + 1u] - cdf[row + x]) / max(total, 1e-12);
    let s = max(sin((f32(y) + 0.5) / f32(h) * PI), 1e-6);
    return p * f32(w * h) / (2.0 * PI * PI * s);
}

fn env_pdf(d: vec3<f32>) -> f32 {
    if ((u.counts1.w & 4u) == 0u) {
        return 0.0;
    }
    let uv = env_uv(d);
    let x = min(u32(uv.x * f32(u.counts1.y)), u.counts1.y - 1u);
    let y = min(u32(uv.y * f32(u.counts1.z)), u.counts1.z - 1u);
    return env_pdf_xy(x, y);
}

struct EnvSample { d: vec3<f32>, pdf: f32 };

fn sample_env(u1: f32, u2: f32) -> EnvSample {
    let w = u.counts1.y;
    let h = u.counts1.z;
    let total = cdf[h];
    if (total <= 0.0 || w == 0u) {
        return EnvSample(vec3<f32>(0.0, -1.0, 0.0), 0.0);
    }
    // Binary searches in the row totals, then the row's columns.
    let target_r = u1 * total;
    var lo = 0u;
    var hi = h;
    loop {
        if (hi - lo <= 1u) {
            break;
        }
        let mid = (lo + hi) / 2u;
        if (cdf[mid] <= target_r) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let y = lo;
    let row = u.offs2.y + y * (w + 1u);
    let target_c = u2 * cdf[row + w];
    var a = 0u;
    var b = w;
    loop {
        if (b - a <= 1u) {
            break;
        }
        let mid = (a + b) / 2u;
        if (cdf[row + mid] <= target_c) {
            a = mid;
        } else {
            b = mid;
        }
    }
    let x = a;
    return EnvSample(env_dir((f32(x) + 0.5) / f32(w), (f32(y) + 0.5) / f32(h)), env_pdf_xy(x, y));
}

// --------------------------------------------------------------- lights

struct LightS { e: vec3<f32>, wi: vec3<f32>, dist: f32, ok: bool };

fn smoothstep_(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / max(e1 - e0, 1e-6), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn light_sample(li: u32, p: vec3<f32>) -> LightS {
    let b = u.offs1.x + 4u * li;
    let l0 = data[b];
    let l1 = data[b + 1u];
    let l2 = data[b + 2u];
    let l3 = data[b + 3u];
    let kind = u32(l0.x);
    if (kind == 0u) {
        let to = -normalize(l2.xyz);
        let z = 1.0 - rng() * (1.0 - l0.y);
        let r = sqrt(max(1.0 - z * z, 0.0));
        let phi = 2.0 * PI * rng();
        let wi = normalize(frame_t(to) * (r * cos(phi)) + frame_b(to) * (r * sin(phi)) + to * z);
        return LightS(l3.xyz * PI, wi, INF, true);
    }
    let dv = l1.xyz - p;
    let dist = length(dv);
    if (dist < 1e-4) {
        return LightS(vec3<f32>(0.0), vec3<f32>(0.0, 1.0, 0.0), 0.0, false);
    }
    let wi = dv / dist;
    var k = 1.0;
    if (l2.w > 0.0) {
        k = 1.0 - smoothstep_(l1.w, l1.w + l2.w, dist);
    }
    if (kind == 1u) {
        k = k * smoothstep_(l0.z, l0.w, dot(-wi, normalize(l2.xyz)));
    }
    return LightS(l3.xyz * (PI * k), wi, dist, k > 0.0);
}

fn light_dark(li: u32) -> f32 {
    return data[u.offs1.x + 4u * li + 3u].w;
}

// ----------------------------------------------------------------- paths

struct Path { rgb: vec3<f32>, alpha: f32, depth: f32, normal: vec3<f32>, albedo: vec3<f32>, object: u32 };

fn camera_ray_o(px: vec2<f32>, lens: vec2<f32>) -> array<vec3<f32>, 2> {
    let x = (px.x - u.cam_right.w) / u.cam_pos.w;
    let y = (px.y - u.cam_down.w) / u.cam_pos.w;
    let d = normalize(u.cam_right.xyz * x + u.cam_down.xyz * y + u.cam_fwd.xyz);
    let lr = u.cam_fwd.w;
    if (lr <= 0.0) {
        return array<vec3<f32>, 2>(u.cam_pos.xyz, d);
    }
    let t = u.misc.x / max(dot(d, u.cam_fwd.xyz), 1e-4);
    let focal = u.cam_pos.xyz + d * t;
    let a = lens.x;
    let b = lens.y;
    var r = 0.0;
    var phi = 0.0;
    if (a != 0.0 || b != 0.0) {
        if (abs(a) > abs(b)) {
            r = a;
            phi = 0.78539816 * (b / a);
        } else {
            r = b;
            phi = 1.5707963 - 0.78539816 * (a / b);
        }
    }
    let o = u.cam_pos.xyz + u.cam_right.xyz * (r * cos(phi) * lr) + u.cam_down.xyz * (r * sin(phi) * lr);
    return array<vec3<f32>, 2>(o, normalize(focal - o));
}

fn random_walk(sf: Surface, time: f32) -> array<vec4<f32>, 2> {
    let mfp = max(sf.subsurface * 6.0, 0.2);
    let eps = u.misc.y * 1e-5 + 1e-3;
    var p = sf.p - sf.n * eps;
    var dir = cosine_dir(-sf.n, rng(), rng());
    for (var k = 0; k < 24; k = k + 1) {
        let step = -log(1.0 - rng()) * mfp;
        let h = intersect(p, dir, eps, step, time);
        if (h.ok) {
            let tr = tri_at(h.tri, time);
            var n = normalize(cross(tr.b - tr.a, tr.c - tr.a));
            if (dot(n, dir) < 0.0) {
                n = -n;
            }
            return array<vec4<f32>, 2>(vec4<f32>(p + dir * h.t, 1.0), vec4<f32>(n, 0.0));
        }
        p = p + dir * step;
        dir = normalize(uniform_sphere(rng(), rng()));
    }
    return array<vec4<f32>, 2>(vec4<f32>(0.0), vec4<f32>(0.0));
}

fn trace_path(px: vec2<f32>) -> Path {
    var time = 0.0;
    if ((u.counts1.w & 1u) != 0u && u.sets.z > 0.0) {
        time = rng();
    }
    let lens = vec2<f32>(rng() * 2.0 - 1.0, rng() * 2.0 - 1.0);
    let cr = camera_ray_o(px, lens);
    var o = cr[0];
    var d = cr[1];
    var out = Path(vec3<f32>(0.0), 0.0, INF, vec3<f32>(0.0), vec3<f32>(0.0), 0u);
    var tp = vec3<f32>(1.0);
    var last_pdf = 0.0;
    var last_delta = true;
    let eps = u.misc.y * 1e-5 + 1e-3;
    let max_b = u32(clamp(u.sets.x, 1.0, 32.0));
    var bounce = 0u;
    var passes = 0;
    loop {
        if (bounce > max_b) {
            break;
        }
        let hit = intersect(o, d, eps, INF, time);
        var seg_end = 1e9;
        if (hit.ok) {
            seg_end = hit.t;
        }
        // Smoke along the way.
        var vt = INF;
        var vi = 0u;
        var vheat = 0.0;
        for (var i = 0u; i < u.counts1.x; i = i + 1u) {
            let v = vol(i);
            let span = box_span(v.lo, v.hi, o, d, seg_end);
            if (span.x >= span.y) {
                continue;
            }
            var t = span.x;
            loop {
                t = t - log(1.0 - rng()) / max(v.density, 1e-6);
                if (t >= span.y || t >= vt) {
                    break;
                }
                let s = volume_at(v, o + d * t);
                if (rng() < s.x) {
                    vt = t;
                    vi = i;
                    vheat = s.y;
                    break;
                }
            }
        }
        // Fire glows along the way (up to where the path scatters).
        let glow = fire_glow(o, d, min(vt, seg_end));
        if (dot(glow, glow) > 0.0) {
            out.rgb = out.rgb + tp * glow;
            if (bounce == 0u) {
                out.alpha = max(out.alpha, min(lum(glow), 1.0));
            }
        }
        if (vt < INF) {
            let v = vol(vi);
            let p = o + d * vt;
            if (bounce == 0u) {
                out.alpha = 1.0;
            }
            let albedo = clamp(lum(v.colour), 0.0, 1.0);
            if (rng() >= max(albedo, 0.05)) {
                break;
            }
            tp = tp * v.colour / max(albedo, 0.05);
            for (var li = 0u; li < u.counts0.z; li = li + 1u) {
                let ls = light_sample(li, p);
                if (ls.ok) {
                    out.rgb = out.rgb + tp * ls.e * visibility(p, ls.wi, ls.dist, time) * (0.25 / PI);
                }
            }
            if ((u.counts1.w & 4u) != 0u) {
                let es = sample_env(rng(), rng());
                if (es.pdf > 0.0) {
                    out.rgb = out.rgb + tp * env_at(es.d) * visibility(p, es.d, INF, time) * (1.0 / (4.0 * PI * es.pdf) * power(es.pdf, 1.0 / (4.0 * PI)));
                }
            }
            o = p;
            d = normalize(uniform_sphere(rng(), rng()));
            last_pdf = 1.0 / (4.0 * PI);
            last_delta = false;
            bounce = bounce + 1u;
            continue;
        }
        if (!hit.ok) {
            if (bounce == 0u) {
                if ((u.counts1.w & 2u) != 0u) {
                    out.rgb = out.rgb + tp * env_at(d);
                    out.alpha = 1.0;
                }
            } else if ((u.counts1.w & 4u) != 0u) {
                var w = 1.0;
                if (!last_delta) {
                    w = power(last_pdf, env_pdf(d));
                }
                out.rgb = out.rgb + tp * env_at(d) * w;
            }
            break;
        }
        let sf = surface(hit, o, d, time);
        if (sf.base.w < 1.0 && rng() > sf.base.w && passes < 16) {
            o = sf.p;
            passes = passes + 1;
            continue;
        }
        if (bounce == 0u) {
            out.alpha = 1.0;
            out.depth = hit.t * dot(d, u.cam_fwd.xyz);
            out.normal = sf.n;
            out.albedo = sf.base.xyz;
            out.object = sf.object + 1u;
            if (u.misc.z > 0.0) {
                let k = 1.0 - exp(-hit.t * u.misc.z);
                out.rgb = out.rgb + u.haze_col.xyz * k;
                tp = tp * (1.0 - k);
            }
        }
        if (dot(sf.emissive, sf.emissive) > 0.0) {
            var w = 1.0;
            if (bounce > 0u && !last_delta) {
                let cos_l = max(abs(dot(sf.ng, d)), 1e-4);
                let pdf_light = lum(sf.emissive) / max(u.haze_col.w, 1e-9) * hit.t * hit.t / cos_l;
                w = power(last_pdf, pdf_light);
            }
            out.rgb = out.rgb + tp * sf.emissive * w;
        }
        if ((sf.flags & UNLIT) != 0u) {
            out.rgb = out.rgb + tp * sf.base.xyz;
            break;
        }
        let wo = -d;
        let ndv = max(dot(sf.n, wo), 1e-4);
        let l = lobes(sf, ndv);
        let p_off = sf.p + sf.n * eps;
        if (l.p_spec + l.p_diff > 0.0) {
            for (var li = 0u; li < u.counts0.z; li = li + 1u) {
                let ls = light_sample(li, p_off);
                if (!ls.ok) {
                    continue;
                }
                let ev = eval(sf, l, wo, ls.wi);
                if (lum(ev.f) <= 0.0) {
                    continue;
                }
                let dark = light_dark(li);
                let vis = vec3<f32>(1.0) - dark * (vec3<f32>(1.0) - visibility(p_off, ls.wi, ls.dist, time));
                out.rgb = out.rgb + tp * ev.f * ls.e * vis;
            }
            let n_emit = u.counts0.w;
            let total = u.haze_col.w;
            if (n_emit > 0u && total > 0.0) {
                let x = rng() * total;
                var a = 0u;
                var b = n_emit;
                loop {
                    if (a >= b) {
                        break;
                    }
                    let mid = (a + b) / 2u;
                    if (data[u.offs1.y + mid].y < x) {
                        a = mid + 1u;
                    } else {
                        b = mid;
                    }
                }
                let e = data[u.offs1.y + min(a, n_emit - 1u)];
                let ti = bitcast<u32>(e.x);
                let tr = tri_at(ti, time);
                var r1 = rng();
                var r2 = rng();
                if (r1 + r2 > 1.0) {
                    r1 = 1.0 - r1;
                    r2 = 1.0 - r2;
                }
                let q = tr.a + (tr.b - tr.a) * r1 + (tr.c - tr.a) * r2;
                let dv = q - p_off;
                let dist = length(dv);
                if (dist > eps) {
                    let wi = dv / dist;
                    let ln = normalize(cross(tr.b - tr.a, tr.c - tr.a));
                    let cos_l = abs(dot(ln, wi));
                    let m = mat(tris[ti].w & 0xffffu);
                    let le = m.m2.xyz;
                    let pdf = lum(le) / total * dist * dist / max(cos_l, 1e-4);
                    let ev = eval(sf, l, wo, wi);
                    if (lum(ev.f) > 0.0 && cos_l > 0.0) {
                        out.rgb = out.rgb + tp * ev.f * le * visibility(p_off, wi, dist - eps * 2.0, time) * (power(pdf, ev.pdf) / pdf);
                    }
                }
            }
            if ((u.counts1.w & 4u) != 0u) {
                let es = sample_env(rng(), rng());
                if (es.pdf > 0.0) {
                    let ev = eval(sf, l, wo, es.d);
                    if (lum(ev.f) > 0.0) {
                        out.rgb = out.rgb + tp * ev.f * env_at(es.d) * visibility(p_off, es.d, INF, time) * (power(es.pdf, ev.pdf) / es.pdf);
                    }
                }
            }
        }
        if (bounce == max_b) {
            break;
        }
        let pick = rng();
        let u1 = rng();
        let u2 = rng();
        if (pick < l.p_trans) {
            if ((sf.flags & HAIR) != 0u) {
                tp = tp * sqrt(sf.base.xyz);
                o = sf.p + d * (eps * 2.0);
                last_delta = true;
                bounce = bounce + 1u;
                continue;
            }
            let a = sqrt(l.a2);
            let phi = 2.0 * PI * u1;
            let cos_t = sqrt(max((1.0 - u2) / (1.0 + (a * a - 1.0) * u2), 0.0));
            let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
            let m = normalize(frame_t(sf.n) * (sin_t * cos(phi)) + frame_b(sf.n) * (sin_t * sin(phi)) + sf.n * cos_t);
            var eta = 1.0 / sf.ior;
            if (sf.front) {
                eta = sf.ior;
            }
            let cos_i = max(dot(wo, m), 1e-4);
            let fr = fresnel_dielectric(cos_i, eta);
            if (rng() < fr) {
                d = normalize(m * (2.0 * dot(wo, m)) - wo);
                o = sf.p + sf.n * eps;
            } else {
                let r = 1.0 / eta;
                let k = 1.0 - r * r * (1.0 - cos_i * cos_i);
                d = normalize(-wo * r + m * (r * cos_i - sqrt(max(k, 0.0))));
                o = sf.p - sf.n * eps;
                tp = tp * sf.base.xyz;
            }
            last_delta = true;
        } else if (pick < l.p_trans + l.p_spec) {
            let phi = 2.0 * PI * u1;
            let cos_t = sqrt(max((1.0 - u2) / (1.0 + (l.a2 - 1.0) * u2), 0.0));
            let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
            let hv = normalize(frame_t(sf.n) * (sin_t * cos(phi)) + frame_b(sf.n) * (sin_t * sin(phi)) + sf.n * cos_t);
            let wi = normalize(hv * (2.0 * dot(wo, hv)) - wo);
            let ev = eval(sf, l, wo, wi);
            if (ev.pdf <= 0.0 || lum(ev.f) <= 0.0) {
                break;
            }
            tp = tp * ev.f / ev.pdf;
            o = p_off;
            d = wi;
            last_pdf = ev.pdf;
            last_delta = l.a2 < 1e-3;
        } else {
            if (sf.subsurface > 0.0 && rng() < sf.subsurface) {
                let walk = random_walk(sf, time);
                if (walk[0].w == 0.0) {
                    break;
                }
                let n_exit = walk[1].xyz;
                let wi = cosine_dir(n_exit, rng(), rng());
                tp = tp * sf.base.xyz;
                o = walk[0].xyz + n_exit * eps;
                d = wi;
                last_pdf = max(dot(n_exit, wi), 1e-4) / PI;
                last_delta = false;
                bounce = bounce + 1u;
                continue;
            }
            let wi = cosine_dir(sf.n, u1, u2);
            let ev = eval(sf, l, wo, wi);
            if (ev.pdf <= 0.0) {
                break;
            }
            tp = tp * ev.f / ev.pdf;
            o = p_off;
            d = wi;
            last_pdf = ev.pdf;
            last_delta = false;
        }
        bounce = bounce + 1u;
        if (bounce > 3u) {
            let q = min(max(tp.x, max(tp.y, tp.z)), 0.95);
            if (rng() > q) {
                break;
            }
            tp = tp / q;
        }
    }
    let m = lum(out.rgb);
    if (m > u.sets.y) {
        out.rgb = out.rgb * (u.sets.y / m);
    }
    if (any(out.rgb != out.rgb)) {
        out.rgb = vec3<f32>(0.0);
    }
    return out;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = u.counts0.x;
    let h = u.counts0.y;
    if (id.x >= w || id.y >= h) {
        return;
    }
    let sample = u.offs2.w;
    seed(id.x, id.y, sample);
    let r = trace_path(vec2<f32>(f32(id.x) + rng(), f32(id.y) + rng()));
    let i = id.y * w + id.x;
    accum[i] = accum[i] + vec4<f32>(r.rgb, r.alpha);
    // First hit (kept from the first sample that hit anything).
    if (sample == 0u || (aux[2u * i].w >= INF && r.depth < INF)) {
        aux[2u * i] = vec4<f32>(r.normal, r.depth);
        aux[2u * i + 1u] = vec4<f32>(r.albedo, bitcast<f32>(r.object));
    }
}
