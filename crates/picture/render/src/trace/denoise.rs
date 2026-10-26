// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Denoising a path-traced picture: the edge-avoiding à-trous wavelet
//! filter (Dammertz et al., "Edge-Avoiding À-Trous Wavelet Transform for
//! fast Global Illumination Filtering", 2010). The light (the picture
//! divided by the surface colour, so textures stay sharp) is blurred over
//! growing spans, five passes of a 5×5 kernel with holes, each sample
//! weighted down where colour, normal or depth differ, so edges, creases
//! and silhouettes are kept. Fewer samples get a stronger filter.

use super::Passes;

/// Smooths `p.beauty` in place, guided by its passes.
pub fn denoise(p: &mut Passes, w: usize, h: usize, samples: u32) {
    let n = w * h;
    if n == 0 || p.beauty.pixels.len() != n {
        return;
    }
    // Light alone: the picture over its surface colour.
    let alb: Vec<[f32; 3]> = p.albedo.iter().map(|a| a.map(|c| c.max(0.02))).collect();
    let covered: Vec<bool> = p.depth.iter().map(|d| d.is_finite()).collect();
    let mut light: Vec<[f32; 3]> = (0..n).map(|i| {
        let c = p.beauty.pixels[i];
        if covered[i] { [c[0] / alb[i][0], c[1] / alb[i][1], c[2] / alb[i][2]] } else { [c[0], c[1], c[2]] }
    }).collect();
    let strength = (8.0 / (samples as f32).sqrt()).clamp(0.15, 3.0);
    let kernel = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
    let mut sigma_c = 0.6 * strength;
    for pass in 0..5 {
        let step = 1usize << pass;
        let src = light.clone();
        let rows: Vec<Vec<[f32; 3]>> = std::thread::scope(|sc| {
            let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16);
            let per = h.div_ceil(threads).max(1);
            let handles: Vec<_> = (0..h).step_by(per).map(|y0| {
                let (src, covered, normal, depth) = (&src, &covered, &p.normal, &p.depth);
                sc.spawn(move || {
                    let mut out = Vec::with_capacity(per * w);
                    for y in y0..(y0 + per).min(h) {
                        for x in 0..w {
                            let i = y * w + x;
                            if !covered[i] {
                                out.push(src[i]);
                                continue;
                            }
                            let c0 = src[i];
                            let (n0, z0) = (normal[i], depth[i]);
                            let mut acc = [0.0f32; 3];
                            let mut wsum = 0.0f32;
                            for (ky, kv) in kernel.iter().enumerate() {
                                let yy = y as i64 + (ky as i64 - 2) * step as i64;
                                if yy < 0 || yy >= h as i64 {
                                    continue;
                                }
                                for (kx, ku) in kernel.iter().enumerate() {
                                    let xx = x as i64 + (kx as i64 - 2) * step as i64;
                                    if xx < 0 || xx >= w as i64 {
                                        continue;
                                    }
                                    let j = yy as usize * w + xx as usize;
                                    if !covered[j] {
                                        continue;
                                    }
                                    let c = src[j];
                                    let dc = (c[0] - c0[0]).powi(2) + (c[1] - c0[1]).powi(2) + (c[2] - c0[2]).powi(2);
                                    let nd = normal[j];
                                    let dn = (1.0 - (nd[0] * n0[0] + nd[1] * n0[1] + nd[2] * n0[2])).max(0.0);
                                    let dz = (depth[j] - z0).abs() / (z0.abs() * 0.02 + 1.0);
                                    let wgt = ku * kv * (-dc / (sigma_c * sigma_c).max(1e-6)).exp() * (-dn * 32.0).exp() * (-dz * 2.0).exp();
                                    acc = [acc[0] + c[0] * wgt, acc[1] + c[1] * wgt, acc[2] + c[2] * wgt];
                                    wsum += wgt;
                                }
                            }
                            out.push(if wsum > 0.0 { acc.map(|v| v / wsum) } else { c0 });
                        }
                    }
                    out
                })
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        light = rows.into_iter().flatten().collect();
        if light.len() != n {
            return;
        }
        sigma_c *= 0.5;
    }
    for i in 0..n {
        if !covered[i] {
            continue;
        }
        let a = p.beauty.pixels[i][3];
        p.beauty.pixels[i] = [light[i][0] * alb[i][0], light[i][1] * alb[i][1], light[i][2] * alb[i][2], a];
    }
}
