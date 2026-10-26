// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Corruption fuzzer over Annex-B streams: h264fuzz <rounds> <files...>
//! Run in a debug build to also catch arithmetic overflow.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rounds: usize = args[1].parse().unwrap();
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut rnd = move || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for f in &args[2..] {
        let data = std::fs::read(f).unwrap();
        let nals = studio_h264::split_annex_b(&data);
        for r in 0..rounds {
            let mut dec = studio_h264::Decoder::new_annex_b();
            let rate = 1 + (rnd() % 20) as usize;
            for n in &nals {
                let mut v = n.to_vec();
                if (rnd() % 100) < rate as u64 && v.len() > 2 {
                    match rnd() % 4 {
                        0 => {
                            for _ in 0..1 + rnd() % 6 {
                                let i = 1 + (rnd() as usize % (v.len() - 1));
                                v[i] ^= 1 << (rnd() % 8);
                            }
                        }
                        1 => v.truncate(1 + rnd() as usize % (v.len() - 1)),
                        2 => {
                            let i = 1 + (rnd() as usize % (v.len() - 1));
                            let e = (i + 1 + rnd() as usize % 40).min(v.len());
                            for b in &mut v[i..e] {
                                *b = rnd() as u8;
                            }
                        }
                        _ => {
                            let i = 1 + (rnd() as usize % (v.len() - 1));
                            v.insert(i, rnd() as u8);
                        }
                    }
                }
                let mut au = vec![0, 0, 0, 1];
                au.extend_from_slice(&v);
                let _ = dec.decode(&au);
            }
            let _ = dec.flush();
            if r % 10 == 9 {
                eprint!(".");
            }
        }
        eprintln!(" {f} ok");
    }
}
