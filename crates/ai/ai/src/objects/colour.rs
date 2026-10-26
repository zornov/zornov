// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The main colours of a region, in words ("navy", "red", "light grey").

use super::BBox;
use crate::frame::Frame;

/// A colour name for an RGB value (0..255).
pub fn name(r: u8, g: u8, b: u8) -> &'static str {
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let mx = rf.max(gf).max(bf);
    let mn = rf.min(gf).min(bf);
    let v = mx;
    let d = mx - mn;
    let s = if mx > 0.0 { d / mx } else { 0.0 };
    if v < 0.16 {
        return "black";
    }
    if s < 0.14 || d < 0.07 {
        return if v > 0.86 { "white" } else if v > 0.62 { "light grey" } else if v > 0.34 { "grey" } else { "dark grey" };
    }
    let h = {
        let h = if mx == rf { ((gf - bf) / d).rem_euclid(6.0) } else if mx == gf { (bf - rf) / d + 2.0 } else { (rf - gf) / d + 4.0 };
        h * 60.0
    };
    match h {
        h if !(15.0..345.0).contains(&h) => {
            if v < 0.45 { "maroon" } else if s < 0.5 && v > 0.75 { "pink" } else { "red" }
        }
        h if h < 45.0 => {
            if v < 0.62 { "brown" } else if s < 0.38 { "beige" } else { "orange" }
        }
        h if h < 70.0 => {
            if v < 0.5 { "olive" } else if s < 0.3 { "beige" } else { "yellow" }
        }
        h if h < 165.0 => if v < 0.3 { "dark green" } else { "green" },
        h if h < 200.0 => "teal",
        h if h < 250.0 => {
            if v < 0.62 { "navy" } else if s < 0.4 && v > 0.7 { "light blue" } else { "blue" }
        }
        h if h < 290.0 => "purple",
        h if h < 345.0 => {
            if s < 0.5 && v > 0.75 { "pink" } else { "magenta" }
        }
        _ => "red",
    }
}

/// The colours that cover at least a sixth of the middle of `b` in
/// `frame`, biggest first (at most three).
pub fn dominant(frame: &Frame, b: &BBox) -> Vec<String> {
    let inner = b.grown(0.7);
    let (w, h) = (frame.width as f32, frame.height as f32);
    let x0 = (inner.x * w).clamp(0.0, w - 1.0) as usize;
    let y0 = (inner.y * h).clamp(0.0, h - 1.0) as usize;
    let x1 = ((inner.right() * w).ceil() as usize).clamp(x0 + 1, frame.width);
    let y1 = ((inner.bottom() * h).ceil() as usize).clamp(y0 + 1, frame.height);
    let step = (((x1 - x0) * (y1 - y0)) as f32 / 4000.0).sqrt().ceil().max(1.0) as usize;
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    let mut total = 0usize;
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let i = (y * frame.width + x) * 4;
            let n = name(frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]);
            match counts.iter_mut().find(|c| c.0 == n) {
                Some(c) => c.1 += 1,
                None => counts.push((n, 1)),
            }
            total += 1;
            x += step;
        }
        y += step;
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    counts.into_iter().filter(|c| c.1 * 6 >= total).take(3).map(|c| c.0.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_for_common_colours() {
        assert_eq!(name(0, 0, 0), "black");
        assert_eq!(name(250, 250, 250), "white");
        assert_eq!(name(200, 20, 20), "red");
        assert_eq!(name(20, 160, 40), "green");
        assert_eq!(name(20, 40, 150), "navy");
        assert_eq!(name(30, 110, 230), "blue");
        assert_eq!(name(120, 70, 30), "brown");
        assert_eq!(name(250, 200, 20), "yellow");
        assert_eq!(name(128, 128, 128), "grey");
        assert_eq!(name(255, 150, 190), "pink");
    }

    #[test]
    fn the_middle_of_the_box_decides() {
        // Red square on a green wall; the box is the square plus a margin.
        let mut f = Frame::filled(100, 100, [20, 160, 40, 255]);
        for y in 30..70 {
            for x in 30..70 {
                f.rgba[(y * 100 + x) * 4..][..3].copy_from_slice(&[210, 20, 20]);
            }
        }
        let c = dominant(&f, &BBox::new(0.28, 0.28, 0.44, 0.44));
        assert_eq!(c.first().map(String::as_str), Some("red"), "{c:?}");
        assert!(!c.contains(&"green".to_string()) || c.len() > 1);
    }
}
