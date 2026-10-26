// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Development switches read once from the environment:
//! - `STUDIO_HEVC_SKIP_LOOP_FILTER`, `STUDIO_HEVC_SKIP_DEBLOCK`, `STUDIO_HEVC_SKIP_SAO`
//! - `STUDIO_HEVC_DUMP_REGION="poc,c,x0,y0,x1,y1"` (sample dump before deblocking / SAO)
//! - `STUDIO_HEVC_TRACE_CU=poc`, `STUDIO_HEVC_DUMP_SAO=poc`

use std::sync::OnceLock;

#[derive(Default, Debug)]
pub struct DebugOpts {
    pub skip_deblock: bool,
    pub skip_sao: bool,
    pub dump_region: Option<[i64; 6]>,
    pub trace_cu: Option<i32>,
    pub dump_sao: Option<i32>,
}

pub fn opts() -> &'static DebugOpts {
    static O: OnceLock<DebugOpts> = OnceLock::new();
    O.get_or_init(|| {
        let set = |n: &str| std::env::var_os(n).is_some();
        let num = |n: &str| std::env::var(n).ok().and_then(|v| v.trim().parse::<i32>().ok());
        let skip_lf = set("STUDIO_HEVC_SKIP_LOOP_FILTER");
        let dump_region = std::env::var("STUDIO_HEVC_DUMP_REGION").ok().and_then(|v| {
            let f: Vec<i64> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if f.len() == 6 {
                Some([f[0], f[1], f[2], f[3], f[4], f[5]])
            } else {
                None
            }
        });
        DebugOpts {
            skip_deblock: skip_lf || set("STUDIO_HEVC_SKIP_DEBLOCK"),
            skip_sao: skip_lf || set("STUDIO_HEVC_SKIP_SAO"),
            dump_region,
            trace_cu: num("STUDIO_HEVC_TRACE_CU"),
            dump_sao: num("STUDIO_HEVC_DUMP_SAO"),
        }
    })
}
