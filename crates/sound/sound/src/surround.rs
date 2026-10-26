// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Channel layouts and the surround panner.
//!
//! A mix is rendered for one [`Layout`]: stereo, 5.1 or 7.1. Channel order
//! inside the crate is the SMPTE / film order `L R C LFE Ls Rs` (5.1) and
//! `L R C LFE Lss Rss Lrs Rrs` (7.1: side then rear surrounds), which is
//! also the order of a 5.1 WAV file. [`Layout::wav_order`] maps 7.1 to the
//! WAV (`WAVE_FORMAT_EXTENSIBLE`) speaker order.
//!
//! Panning is pairwise amplitude panning on the ring of speakers (VBAP in
//! two dimensions, Pulkki 1997) from a position on the room square:
//! `x` −1 left .. 1 right, `y` −1 back .. 1 front. The distance from the
//! centre of the room sets how much of the sound is spread over the whole
//! ring instead of the speaker pair: at the edge a sound comes from one
//! direction, in the middle of the room it is everywhere. A stereo source
//! is panned as two points `width` degrees apart, so the default (front,
//! 60°) puts its left and right channels exactly on L and R. The LFE
//! channel is only ever fed by an explicit send.

/// Speaker layout of a mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Layout {
    /// Two channels.
    #[default]
    Stereo,
    /// 5.1: L R C LFE Ls Rs.
    Surround51,
    /// 7.1: L R C LFE Lss Rss Lrs Rrs.
    Surround71,
}

/// Index of the LFE channel in the surround layouts.
pub const LFE: usize = 3;
/// Index of the centre channel in the surround layouts.
pub const CENTRE: usize = 2;
/// Most channels any layout has.
pub const MAX_CHANNELS: usize = 8;

impl Layout {
    /// Every layout, in menu order.
    pub const ALL: [Layout; 3] = [Layout::Stereo, Layout::Surround51, Layout::Surround71];

    /// Number of channels.
    pub fn channels(self) -> usize {
        match self {
            Layout::Stereo => 2,
            Layout::Surround51 => 6,
            Layout::Surround71 => 8,
        }
    }

    /// Stable id for project files and the AI: `stereo`, `5.1`, `7.1`.
    pub fn id(self) -> &'static str {
        match self {
            Layout::Stereo => "stereo",
            Layout::Surround51 => "5.1",
            Layout::Surround71 => "7.1",
        }
    }

    /// Parses an id (also `51`, `5_1`, `surround`, `71`, `7_1`).
    pub fn from_id(s: &str) -> Option<Layout> {
        Some(match s.trim().to_ascii_lowercase().replace(['_', ' '], ".").as_str() {
            "stereo" | "2.0" | "2" => Layout::Stereo,
            "5.1" | "51" | "surround" | "6" => Layout::Surround51,
            "7.1" | "71" | "8" => Layout::Surround71,
            _ => return None,
        })
    }

    /// Short channel names in crate order.
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Layout::Stereo => &["L", "R"],
            Layout::Surround51 => &["L", "R", "C", "LFE", "Ls", "Rs"],
            Layout::Surround71 => &["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"],
        }
    }

    /// Whether the layout has a centre and an LFE channel.
    pub fn is_surround(self) -> bool {
        self != Layout::Stereo
    }

    /// Speaker azimuths in degrees (0 front, positive to the left), `None`
    /// for the LFE. ITU-R BS.775 placement: L/R ±30°, surrounds ±110° in
    /// 5.1; sides ±90° and rears ±150° in 7.1.
    pub fn azimuths(self) -> &'static [Option<f32>] {
        match self {
            Layout::Stereo => &[Some(30.0), Some(-30.0)],
            Layout::Surround51 => &[Some(30.0), Some(-30.0), Some(0.0), None, Some(110.0), Some(-110.0)],
            Layout::Surround71 => &[Some(30.0), Some(-30.0), Some(0.0), None, Some(90.0), Some(-90.0), Some(150.0), Some(-150.0)],
        }
    }

    /// BS.1770 channel weights: 1 for front channels, 1.41 (+1.5 dB) for
    /// surrounds, 0 for the LFE (excluded from loudness).
    pub fn loudness_weights(self) -> &'static [f64] {
        match self {
            Layout::Stereo => &[1.0, 1.0],
            Layout::Surround51 => &[1.0, 1.0, 1.0, 0.0, 1.41, 1.41],
            Layout::Surround71 => &[1.0, 1.0, 1.0, 0.0, 1.41, 1.41, 1.41, 1.41],
        }
    }

    /// For each WAV channel slot, the crate channel it carries. WAV orders
    /// speakers by their mask bit: FL FR FC LFE BL BR … SL SR, so 7.1's rear
    /// pair comes before the side pair.
    pub fn wav_order(self) -> &'static [usize] {
        match self {
            Layout::Stereo => &[0, 1],
            Layout::Surround51 => &[0, 1, 2, 3, 4, 5],
            Layout::Surround71 => &[0, 1, 2, 3, 6, 7, 4, 5],
        }
    }

    /// The `dwChannelMask` of a `WAVE_FORMAT_EXTENSIBLE` file.
    pub fn wav_channel_mask(self) -> u32 {
        match self {
            Layout::Stereo => 0x3,
            // FL FR FC LFE BL BR: the common 5.1 mask (Ls/Rs as "back").
            Layout::Surround51 => 0x3F,
            // FL FR FC LFE BL BR SL SR.
            Layout::Surround71 => 0x63F,
        }
    }

    /// Fold-down gains for monitoring a surround mix on two speakers (ITU-R
    /// BS.775 Lo/Ro: centre and surrounds at −3 dB, LFE dropped), as
    /// `(to_left, to_right)` per channel.
    pub fn downmix(self) -> &'static [(f32, f32)] {
        const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
        match self {
            Layout::Stereo => &[(1.0, 0.0), (0.0, 1.0)],
            Layout::Surround51 => &[(1.0, 0.0), (0.0, 1.0), (H, H), (0.0, 0.0), (H, 0.0), (0.0, H)],
            Layout::Surround71 => &[(1.0, 0.0), (0.0, 1.0), (H, H), (0.0, 0.0), (H, 0.0), (0.0, H), (H, 0.0), (0.0, H)],
        }
    }
}

/// Where a lane sits in the room, sampled at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// −1 left .. 1 right.
    pub x: f32,
    /// −1 back .. 1 front.
    pub y: f32,
    /// 0 ear level .. 1 overhead (used by object export; the 5.1/7.1 beds
    /// have no height speakers).
    pub z: f32,
}

impl Default for Position {
    fn default() -> Self {
        Self { x: 0.0, y: 1.0, z: 0.0 }
    }
}

impl Position {
    /// Azimuth in degrees (0 front, positive left) and distance from the
    /// room centre (0..1).
    pub fn polar(self) -> (f32, f32) {
        let d = (self.x * self.x + self.y * self.y).sqrt().min(1.0);
        if d < 1e-6 {
            return (0.0, 0.0);
        }
        ((-self.x).atan2(self.y).to_degrees(), d)
    }
}

/// How a lane is panned in a surround mix, beyond its position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanShape {
    /// Angle between a stereo source's two channels, degrees (0..180).
    pub width: f32,
    /// Allow the centre speaker; off keeps a phantom centre between L and R
    /// (music is usually mixed this way, dialogue never).
    pub centre: bool,
    /// Send to the LFE channel in dB (`NEG_INFINITY` = none).
    pub lfe_db: f32,
}

impl Default for PanShape {
    fn default() -> Self {
        Self { width: 60.0, centre: true, lfe_db: f32::NEG_INFINITY }
    }
}

/// Gains of a point source at azimuth `az` with spread `1 − distance` over
/// the speakers of `layout`, written into `out` (crate channel order, LFE 0).
/// `side`: restrict the diffuse part to the left (`1`) or right (`-1`) half,
/// for the two channels of a stereo source; `0` uses the whole ring. The
/// gains have unit power.
pub fn point_gains(layout: Layout, az: f32, distance: f32, centre: bool, side: i8, out: &mut [f32; MAX_CHANNELS]) {
    *out = [0.0; MAX_CHANNELS];
    let az_list = layout.azimuths();
    // Ring speakers sorted by azimuth.
    let mut ring: [(f32, usize); MAX_CHANNELS] = [(0.0, 0); MAX_CHANNELS];
    let mut n = 0;
    for (i, a) in az_list.iter().enumerate() {
        if let Some(a) = a
            && (centre || i != CENTRE || layout == Layout::Stereo)
        {
            ring[n] = (*a, i);
            n += 1;
        }
    }
    let ring = &mut ring[..n];
    ring.sort_by(|a, b| a.0.total_cmp(&b.0));
    let d = distance.clamp(0.0, 1.0);
    let mut directional = [0.0f32; MAX_CHANNELS];
    if layout == Layout::Stereo {
        // Stereo: the equal-power pan law on the azimuth's left-right part
        // (the speakers at ±30° are hard left and right), so a sound behind
        // still comes from the side it is on.
        let pan = (-az.to_radians().sin() * 2.0).clamp(-1.0, 1.0);
        let (l, r) = crate::buffer::pan_gains(pan);
        directional[0] = l;
        directional[1] = r;
    } else {
        pair_gains(ring, az, &mut directional);
    }
    if d >= 0.9999 {
        out[..].copy_from_slice(&directional);
        return;
    }
    // Diffuse part: equal power over the ring (no centre: a diffuse sound
    // has no front), or over one half for one side of a stereo source.
    let mut diffuse = [0.0f32; MAX_CHANNELS];
    let mut m = 0;
    for (a, i) in ring.iter() {
        let take = *i != CENTRE || layout == Layout::Stereo;
        let on_side = match side {
            1 => *a > 0.0,
            -1 => *a < 0.0,
            _ => true,
        };
        if take && on_side {
            diffuse[*i] = 1.0;
            m += 1;
        }
    }
    if m > 0 {
        let g = 1.0 / (m as f32).sqrt();
        for v in diffuse.iter_mut() {
            *v *= g;
        }
    } else {
        diffuse = directional;
    }
    let (wd, wf) = (d * d, 1.0 - d * d);
    for i in 0..MAX_CHANNELS {
        out[i] = (wd * directional[i] * directional[i] + wf * diffuse[i] * diffuse[i]).sqrt();
    }
}

/// Pairwise panning on a ring sorted by azimuth.
fn pair_gains(ring: &[(f32, usize)], az: f32, out: &mut [f32; MAX_CHANNELS]) {
    let n = ring.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        out[ring[0].1] = 1.0;
        return;
    }
    let wrap = |a: f32| -> f32 {
        let mut a = a % 360.0;
        if a < 0.0 {
            a += 360.0;
        }
        a
    };
    // Find the adjacent pair (going round) that encloses the azimuth.
    for k in 0..n {
        let (a1, i1) = ring[k];
        let (a2, i2) = ring[(k + 1) % n];
        let span = wrap(a2 - a1);
        let off = wrap(az - a1);
        if span > 0.0 && off <= span + 1e-4 {
            // Tangent-law panning within the pair (equivalent to 2-D VBAP
            // for a symmetric pair).
            let half = (span * 0.5).to_radians();
            let phi = (off - span * 0.5).to_radians();
            let t = if half.tan().abs() < 1e-6 { 0.0 } else { (phi.tan() / half.tan()).clamp(-1.0, 1.0) };
            // t = (g2 − g1) / (g1 + g2)
            let g1 = (1.0 - t) * 0.5;
            let g2 = (1.0 + t) * 0.5;
            let norm = (g1 * g1 + g2 * g2).sqrt().max(1e-9);
            out[i1] += g1 / norm;
            out[i2] += g2 / norm;
            return;
        }
    }
    out[ring[0].1] = 1.0;
}

/// Gains for a source with `channels` (1 or 2) at `pos` with `shape`:
/// `gains[input][output]`.
pub fn source_gains(layout: Layout, pos: Position, shape: PanShape, channels: usize) -> [[f32; MAX_CHANNELS]; 2] {
    let (az, d) = pos.polar();
    let mut g = [[0.0f32; MAX_CHANNELS]; 2];
    if channels <= 1 {
        point_gains(layout, az, d, shape.centre, 0, &mut g[0]);
    } else {
        let half = shape.width.clamp(0.0, 180.0) * 0.5;
        point_gains(layout, az + half, d, shape.centre, 1, &mut g[0]);
        point_gains(layout, az - half, d, shape.centre, -1, &mut g[1]);
        // Where the two channels land on the same speakers their common
        // (centre) content adds coherently; scale so a mono-in-stereo
        // source keeps the power it has on two speakers.
        let rho: f32 = g[0].iter().zip(&g[1]).map(|(a, b)| a * b).sum();
        let k = 1.0 / (1.0 + rho.max(0.0)).sqrt();
        for row in &mut g {
            for v in row.iter_mut() {
                *v *= k;
            }
        }
    }
    if layout.is_surround() && shape.lfe_db.is_finite() {
        let lfe = crate::buffer::db_to_gain(shape.lfe_db) * if channels <= 1 { 1.0 } else { 0.5 };
        g[0][LFE] = lfe;
        g[1][LFE] = lfe;
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(g: &[f32; MAX_CHANNELS]) -> f32 {
        g.iter().map(|v| v * v).sum()
    }

    #[test]
    fn layouts_describe_themselves() {
        for l in Layout::ALL {
            assert_eq!(l.names().len(), l.channels());
            assert_eq!(l.azimuths().len(), l.channels());
            assert_eq!(l.loudness_weights().len(), l.channels());
            assert_eq!(l.wav_order().len(), l.channels());
            assert_eq!(l.wav_channel_mask().count_ones() as usize, l.channels());
            assert_eq!(Layout::from_id(l.id()), Some(l));
        }
        assert_eq!(Layout::from_id("5_1"), Some(Layout::Surround51));
    }

    #[test]
    fn front_centre_goes_to_the_centre_speaker() {
        let g = source_gains(Layout::Surround51, Position::default(), PanShape::default(), 1);
        assert!((g[0][CENTRE] - 1.0).abs() < 1e-4, "{:?}", g[0]);
        assert!(g[0][0].abs() < 1e-4 && g[0][1].abs() < 1e-4);
        // Without the centre: a phantom centre, equal in L and R.
        let g = source_gains(Layout::Surround51, Position::default(), PanShape { centre: false, ..Default::default() }, 1);
        assert!(g[0][CENTRE].abs() < 1e-6);
        assert!((g[0][0] - g[0][1]).abs() < 1e-4 && (g[0][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }

    #[test]
    fn a_stereo_source_lands_on_left_and_right() {
        let g = source_gains(Layout::Surround51, Position::default(), PanShape::default(), 2);
        assert!((g[0][0] - 1.0).abs() < 1e-3, "left input {:?}", g[0]);
        assert!((g[1][1] - 1.0).abs() < 1e-3, "right input {:?}", g[1]);
        assert!(g[0][CENTRE] < 1e-3 && g[1][CENTRE] < 1e-3);
        // Zero width: both channels in the centre, a mono source's two
        // equal channels sum to the power they had on two speakers.
        let g = source_gains(Layout::Surround51, Position::default(), PanShape { width: 0.0, ..Default::default() }, 2);
        let c = g[0][CENTRE] + g[1][CENTRE];
        assert!((c * c - 2.0).abs() < 1e-3, "{c}");
    }

    #[test]
    fn speaker_positions_are_hit_exactly_and_power_is_kept() {
        let l = Layout::Surround71;
        for (i, a) in l.azimuths().iter().enumerate() {
            let Some(a) = a else { continue };
            let mut g = [0.0; MAX_CHANNELS];
            point_gains(l, *a, 1.0, true, 0, &mut g);
            assert!((g[i] - 1.0).abs() < 1e-3, "speaker {i} at {a}: {g:?}");
        }
        for az in [-170.0f32, -120.0, -45.0, 10.0, 75.0, 135.0, 180.0] {
            for d in [0.0f32, 0.4, 1.0] {
                let mut g = [0.0; MAX_CHANNELS];
                point_gains(Layout::Surround51, az, d, true, 0, &mut g);
                assert!((power(&g) - 1.0).abs() < 1e-3, "az {az} d {d}: {g:?}");
                assert_eq!(g[LFE], 0.0);
            }
        }
    }

    #[test]
    fn behind_goes_to_the_surrounds_and_the_middle_is_everywhere() {
        let g = source_gains(Layout::Surround51, Position { x: 0.0, y: -1.0, z: 0.0 }, PanShape::default(), 1);
        assert!(g[0][4] > 0.6 && g[0][5] > 0.6 && g[0][0] < 1e-3 && g[0][CENTRE] < 1e-3, "{:?}", g[0]);
        let g = source_gains(Layout::Surround51, Position { x: 0.0, y: 0.0, z: 0.0 }, PanShape::default(), 1);
        for ch in [0, 1, 4, 5] {
            assert!((g[0][ch] - 0.5).abs() < 1e-3, "{:?}", g[0]);
        }
        // Hard left in stereo is the left speaker.
        let g = source_gains(Layout::Stereo, Position { x: -1.0, y: 0.0, z: 0.0 }, PanShape::default(), 1);
        assert!((g[0][0] - 1.0).abs() < 1e-3 && g[0][1].abs() < 1e-3);
    }

    #[test]
    fn lfe_only_by_send() {
        let g = source_gains(Layout::Surround51, Position::default(), PanShape { lfe_db: 0.0, ..Default::default() }, 1);
        assert!((g[0][LFE] - 1.0).abs() < 1e-6);
        let g = source_gains(Layout::Stereo, Position::default(), PanShape { lfe_db: 0.0, ..Default::default() }, 1);
        assert_eq!(g[0].iter().skip(2).sum::<f32>(), 0.0);
    }
}
