// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scale factor band tables (ISO/IEC 14496-3 Tables 4.129..4.147) and TNS
//! band limits (Table 4.155) for 1024/128-line windows.

static SWB_1024_96: [u16; 42] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 156, 172, 188, 212,
    240, 276, 320, 384, 448, 512, 576, 640, 704, 768, 832, 896, 960, 1024,
];
static SWB_128_96: [u16; 13] = [0, 4, 8, 12, 16, 20, 24, 32, 40, 48, 64, 92, 128];
static SWB_1024_64: [u16; 48] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 100, 112, 124, 140, 156, 172, 192, 216, 240,
    268, 304, 344, 384, 424, 464, 504, 544, 584, 624, 664, 704, 744, 784, 824, 864, 904, 944, 984, 1024,
];
static SWB_1024_48: [u16; 50] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160, 176, 196, 216, 240, 264,
    292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704, 736, 768, 800, 832, 864, 896, 928, 1024,
];
static SWB_128_48: [u16; 15] = [0, 4, 8, 12, 16, 20, 28, 36, 44, 56, 68, 80, 96, 112, 128];
static SWB_1024_32: [u16; 52] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160, 176, 196, 216, 240, 264,
    292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704, 736, 768, 800, 832, 864, 896, 928, 960, 992,
    1024,
];
static SWB_1024_24: [u16; 48] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 52, 60, 68, 76, 84, 92, 100, 108, 116, 124, 136, 148, 160, 172, 188, 204,
    220, 240, 260, 284, 308, 336, 364, 396, 432, 468, 508, 552, 600, 652, 704, 768, 832, 896, 960, 1024,
];
static SWB_128_24: [u16; 16] = [0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 64, 76, 92, 108, 128];
static SWB_1024_16: [u16; 44] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 100, 112, 124, 136, 148, 160, 172, 184, 196, 212, 228, 244, 260, 280,
    300, 320, 344, 368, 396, 424, 456, 492, 532, 572, 616, 664, 716, 772, 832, 896, 960, 1024,
];
static SWB_128_16: [u16; 16] = [0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 60, 72, 88, 108, 128];
static SWB_1024_8: [u16; 41] = [
    0, 12, 24, 36, 48, 60, 72, 84, 96, 108, 120, 132, 144, 156, 172, 188, 204, 220, 236, 252, 268, 288, 308, 328, 348,
    372, 396, 420, 448, 476, 508, 544, 580, 620, 664, 712, 764, 820, 880, 944, 1024,
];
static SWB_128_8: [u16; 16] = [0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 60, 72, 88, 108, 128];

/// Band layout for one sampling-frequency index.
pub(crate) struct BandTables {
    pub long: &'static [u16],
    pub short: &'static [u16],
    pub tns_max_long: usize,
    pub tns_max_short: usize,
}

/// Indexed by sampling-frequency index 0..=12 (96 kHz .. 7.35 kHz).
pub(crate) static BANDS: [BandTables; 13] = [
    BandTables { long: &SWB_1024_96, short: &SWB_128_96, tns_max_long: 31, tns_max_short: 9 },
    BandTables { long: &SWB_1024_96, short: &SWB_128_96, tns_max_long: 31, tns_max_short: 9 },
    BandTables { long: &SWB_1024_64, short: &SWB_128_96, tns_max_long: 34, tns_max_short: 10 },
    BandTables { long: &SWB_1024_48, short: &SWB_128_48, tns_max_long: 40, tns_max_short: 14 },
    BandTables { long: &SWB_1024_48, short: &SWB_128_48, tns_max_long: 42, tns_max_short: 14 },
    BandTables { long: &SWB_1024_32, short: &SWB_128_48, tns_max_long: 51, tns_max_short: 14 },
    BandTables { long: &SWB_1024_24, short: &SWB_128_24, tns_max_long: 46, tns_max_short: 14 },
    BandTables { long: &SWB_1024_24, short: &SWB_128_24, tns_max_long: 46, tns_max_short: 14 },
    BandTables { long: &SWB_1024_16, short: &SWB_128_16, tns_max_long: 42, tns_max_short: 14 },
    BandTables { long: &SWB_1024_16, short: &SWB_128_16, tns_max_long: 42, tns_max_short: 14 },
    BandTables { long: &SWB_1024_16, short: &SWB_128_16, tns_max_long: 42, tns_max_short: 14 },
    BandTables { long: &SWB_1024_8, short: &SWB_128_8, tns_max_long: 39, tns_max_short: 14 },
    BandTables { long: &SWB_1024_8, short: &SWB_128_8, tns_max_long: 39, tns_max_short: 14 },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_tables_are_monotonic_and_complete() {
        let long_bands = [41, 41, 47, 49, 49, 51, 47, 47, 43, 43, 43, 40, 40];
        let short_bands = [12, 12, 12, 14, 14, 14, 15, 15, 15, 15, 15, 15, 15];
        for (i, b) in BANDS.iter().enumerate() {
            assert_eq!(b.long.len() - 1, long_bands[i], "long {i}");
            assert_eq!(b.short.len() - 1, short_bands[i], "short {i}");
            assert_eq!(*b.long.last().unwrap(), 1024);
            assert_eq!(*b.short.last().unwrap(), 128);
            assert!(b.long.windows(2).all(|w| w[0] < w[1] && (w[1] - w[0]) % 4 == 0));
            assert!(b.short.windows(2).all(|w| w[0] < w[1] && (w[1] - w[0]) % 4 == 0));
            assert!(b.tns_max_long <= long_bands[i] && b.tns_max_short <= short_bands[i]);
        }
    }
}
