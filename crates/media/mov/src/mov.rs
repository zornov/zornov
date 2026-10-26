// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QuickTime (`.mov`) writer for ProRes and DNxHR masters (sample entries
//! `apch` and friends, or `AVdh`).
//!
//! Layout: `ftyp`, `wide`, `mdat` (64-bit size), then `moov`. Each video
//! frame is its own chunk, followed by that frame's share of the audio
//! (so audio and picture interleave frame by frame), then one chunk with
//! the timecode sample. Offsets are always `co64`.
//!
//! Audio is `lpcm` with a version 2 sound description: signed, packed,
//! big-endian integers, 24 bits per sample, any channel count.

use crate::StartTimecode;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// What the video track holds.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoTrack {
    pub profile: studio_prores::Profile,
    pub width: u32,
    pub height: u32,
    /// Frame rate as a fraction (24000/1001, 25/1, …).
    pub rate_num: u32,
    pub rate_den: u32,
    /// H.273 code points for the `colr` atom (primaries, transfer, matrix).
    pub colour: (u16, u16, u16),
    /// The frames carry alpha (4444 profiles).
    pub alpha: bool,
}

/// What the audio track holds (24-bit LPCM).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrack {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A ProRes QuickTime file being written.
pub struct MovWriter {
    file: BufWriter<File>,
    video: VideoTrack,
    /// Set for a DNxHR master: the video track then carries `AVdh` frames and
    /// `video.profile` is not used.
    dnx: Option<studio_dnxhr::Profile>,
    audio: Option<AudioTrack>,
    timecode: Option<StartTimecode>,
    /// Timescale of video (and timecode) samples, and frame duration in it.
    timescale: u32,
    frame_dur: u32,
    mdat_start: u64,
    pos: u64,
    video_sizes: Vec<u32>,
    video_offsets: Vec<u64>,
    /// (offset, frames) per audio chunk.
    audio_chunks: Vec<(u64, u32)>,
    audio_frames: u64,
}

fn timescale_of(num: u32, den: u32) -> (u32, u32) {
    if den == 1 { (num * 100, 100) } else { (num, den) }
}

impl MovWriter {
    pub fn create(path: &Path, video: VideoTrack, audio: Option<AudioTrack>, timecode: Option<StartTimecode>) -> io::Result<MovWriter> {
        MovWriter::create_with(path, video, None, audio, timecode)
    }

    /// A QuickTime file whose video track holds DNxHR frames (`AVdh`).
    /// `video.profile` and `video.alpha` are ignored.
    pub fn create_dnxhr(path: &Path, profile: studio_dnxhr::Profile, video: VideoTrack, audio: Option<AudioTrack>, timecode: Option<StartTimecode>) -> io::Result<MovWriter> {
        MovWriter::create_with(path, VideoTrack { alpha: false, ..video }, Some(profile), audio, timecode)
    }

    fn create_with(path: &Path, video: VideoTrack, dnx: Option<studio_dnxhr::Profile>, audio: Option<AudioTrack>, timecode: Option<StartTimecode>) -> io::Result<MovWriter> {
        let mut file = BufWriter::new(File::create(path)?);
        let mut head = Vec::new();
        atom(&mut head, b"ftyp", |b| {
            b.extend_from_slice(b"qt  ");
            b.extend_from_slice(&0x2005_0300u32.to_be_bytes());
            b.extend_from_slice(b"qt  ");
        });
        atom(&mut head, b"wide", |_| {});
        let mdat_start = head.len() as u64;
        // 64-bit mdat: size 1, then the real size (patched in finish).
        head.extend_from_slice(&1u32.to_be_bytes());
        head.extend_from_slice(b"mdat");
        head.extend_from_slice(&0u64.to_be_bytes());
        file.write_all(&head)?;
        let (timescale, frame_dur) = timescale_of(video.rate_num.max(1), video.rate_den.max(1));
        Ok(MovWriter {
            file,
            video,
            dnx,
            audio,
            timecode,
            timescale,
            frame_dur,
            mdat_start,
            pos: head.len() as u64,
            video_sizes: Vec::new(),
            video_offsets: Vec::new(),
            audio_chunks: Vec::new(),
            audio_frames: 0,
        })
    }

    /// Appends one ProRes frame.
    pub fn write_video(&mut self, frame: &[u8]) -> io::Result<()> {
        self.video_offsets.push(self.pos);
        self.video_sizes.push(frame.len() as u32);
        self.file.write_all(frame)?;
        self.pos += frame.len() as u64;
        Ok(())
    }

    /// Appends interleaved audio samples (−1..1), as many as follow the
    /// last video frame.
    pub fn write_audio(&mut self, samples: &[f32]) -> io::Result<()> {
        let Some(a) = self.audio else { return Ok(()) };
        let ch = a.channels.max(1) as usize;
        let frames = samples.len() / ch;
        if frames == 0 {
            return Ok(());
        }
        let mut buf = Vec::with_capacity(frames * ch * 3);
        for s in &samples[..frames * ch] {
            let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            let b = v.to_be_bytes();
            buf.extend_from_slice(&b[1..4]);
        }
        self.audio_chunks.push((self.pos, frames as u32));
        self.audio_frames += frames as u64;
        self.file.write_all(&buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    /// Writes the index and closes the file.
    pub fn finish(mut self) -> io::Result<()> {
        let tc_offset = self.pos;
        if let Some(tc) = &self.timecode {
            self.file.write_all(&tc.frame.to_be_bytes())?;
            self.pos += 4;
        }
        let mdat_size = self.pos - self.mdat_start;
        let moov = self.moov(tc_offset);
        self.file.write_all(&moov)?;
        self.file.flush()?;
        let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(self.mdat_start + 8))?;
        f.write_all(&mdat_size.to_be_bytes())?;
        f.sync_all()
    }

    fn moov(&self, tc_offset: u64) -> Vec<u8> {
        let frames = self.video_sizes.len() as u64;
        let video_dur = frames * self.frame_dur as u64;
        let movie_ts = self.timescale;
        let mut moov = Vec::new();
        atom(&mut moov, b"moov", |m| {
            let next_id = 2 + self.audio.is_some() as u32 + self.timecode.is_some() as u32;
            full(m, b"mvhd", 0, 0, |b| {
                b.extend_from_slice(&[0u8; 8]); // creation, modification
                b.extend_from_slice(&movie_ts.to_be_bytes());
                b.extend_from_slice(&(video_dur as u32).to_be_bytes());
                b.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
                b.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
                b.extend_from_slice(&[0u8; 10]);
                matrix(b);
                b.extend_from_slice(&[0u8; 24]); // preview, poster, selection, current time
                b.extend_from_slice(&next_id.to_be_bytes());
            });
            let tc_id = if self.audio.is_some() { 3 } else { 2 };
            // Video.
            self.trak(m, 1, video_dur, |t| {
                if self.timecode.is_some() {
                    atom(t, b"tref", |r| {
                        atom(r, b"tmcd", |x| x.extend_from_slice(&(tc_id as u32).to_be_bytes()));
                    });
                }
                atom(t, b"mdia", |md| {
                    mdhd(md, self.timescale, video_dur);
                    hdlr(md, b"mhlr", b"vide", "Zornov Video");
                    atom(md, b"minf", |mi| {
                        full(mi, b"vmhd", 0, 1, |b| b.extend_from_slice(&[0, 0x40, 0x80, 0, 0x80, 0, 0x80, 0]));
                        hdlr(mi, b"dhlr", b"alis", "Zornov Data");
                        dinf(mi);
                        atom(mi, b"stbl", |st| {
                            full(st, b"stsd", 0, 0, |b| {
                                b.extend_from_slice(&1u32.to_be_bytes());
                                self.video_entry(b);
                            });
                            stts(st, &[(frames as u32, self.frame_dur)]);
                            stsc(st, &[(1, 1)]);
                            stsz(st, 0, &self.video_sizes);
                            co64(st, &self.video_offsets);
                        });
                    });
                });
            });
            // Audio.
            if let Some(a) = self.audio {
                let dur_movie = (self.audio_frames as f64 / a.sample_rate as f64 * movie_ts as f64).round() as u64;
                self.trak(m, 2, dur_movie, |t| {
                    atom(t, b"mdia", |md| {
                        mdhd(md, a.sample_rate, self.audio_frames);
                        hdlr(md, b"mhlr", b"soun", "Zornov Sound");
                        atom(md, b"minf", |mi| {
                            full(mi, b"smhd", 0, 0, |b| b.extend_from_slice(&[0u8; 4]));
                            hdlr(mi, b"dhlr", b"alis", "Zornov Data");
                            dinf(mi);
                            atom(mi, b"stbl", |st| {
                                full(st, b"stsd", 0, 0, |b| {
                                    b.extend_from_slice(&1u32.to_be_bytes());
                                    lpcm_entry(b, a);
                                });
                                stts(st, &[(self.audio_frames as u32, 1)]);
                                // Runs of chunks with the same frame count.
                                let mut runs: Vec<(u32, u32)> = Vec::new();
                                for (i, (_, n)) in self.audio_chunks.iter().enumerate() {
                                    if runs.last().map(|r| r.1) != Some(*n) {
                                        runs.push((i as u32 + 1, *n));
                                    }
                                }
                                stsc(st, &runs);
                                stsz_fixed(st, 3 * a.channels as u32, self.audio_frames as u32);
                                co64(st, &self.audio_chunks.iter().map(|c| c.0).collect::<Vec<_>>());
                            });
                        });
                    });
                });
            }
            // Timecode.
            if let Some(tc) = &self.timecode {
                self.trak(m, tc_id, video_dur, |t| {
                    atom(t, b"mdia", |md| {
                        mdhd(md, self.timescale, video_dur);
                        hdlr(md, b"mhlr", b"tmcd", "Zornov Time Code");
                        atom(md, b"minf", |mi| {
                            atom(mi, b"gmhd", |g| {
                                full(g, b"gmin", 0, 0, |b| b.extend_from_slice(&[0, 0x40, 0x80, 0, 0x80, 0, 0x80, 0, 0, 0, 0, 0]));
                                atom(g, b"tmcd", |x| {
                                    full(x, b"tcmi", 0, 0, |b| {
                                        b.extend_from_slice(&[0, 0, 0, 0, 0, 12]); // font, face, size
                                        b.extend_from_slice(&[0, 0]);
                                        b.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // text colour
                                        b.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // background
                                        pascal(b, "Lucida Grande");
                                    });
                                });
                            });
                            hdlr(mi, b"dhlr", b"alis", "Zornov Data");
                            dinf(mi);
                            atom(mi, b"stbl", |st| {
                                full(st, b"stsd", 0, 0, |b| {
                                    b.extend_from_slice(&1u32.to_be_bytes());
                                    atom(b, b"tmcd", |e| {
                                        e.extend_from_slice(&[0u8; 6]);
                                        e.extend_from_slice(&1u16.to_be_bytes());
                                        e.extend_from_slice(&0u32.to_be_bytes());
                                        let flags = tc.drop_frame as u32 | 0x2; // drop frame, 24-hour max
                                        e.extend_from_slice(&flags.to_be_bytes());
                                        e.extend_from_slice(&self.timescale.to_be_bytes());
                                        e.extend_from_slice(&self.frame_dur.to_be_bytes());
                                        e.push(tc.fps.min(255) as u8);
                                        e.push(0);
                                        if let Some(reel) = &tc.reel {
                                            atom(e, b"name", |n| {
                                                n.extend_from_slice(&(reel.len() as u16).to_be_bytes());
                                                n.extend_from_slice(&0u16.to_be_bytes());
                                                n.extend_from_slice(reel.as_bytes());
                                            });
                                        }
                                    });
                                });
                                stts(st, &[(1, video_dur as u32)]);
                                stsc(st, &[(1, 1)]);
                                stsz_fixed(st, 4, 1);
                                co64(st, &[tc_offset]);
                            });
                        });
                    });
                });
            }
        });
        moov
    }

    fn trak(&self, m: &mut Vec<u8>, id: u32, duration: u64, body: impl FnOnce(&mut Vec<u8>)) {
        let is_video = id == 1;
        let is_audio = self.audio.is_some() && id == 2;
        atom(m, b"trak", |t| {
            full(t, b"tkhd", 0, if is_video || is_audio { 0xF } else { 0xE }, |b| {
                b.extend_from_slice(&[0u8; 8]);
                b.extend_from_slice(&id.to_be_bytes());
                b.extend_from_slice(&[0u8; 4]);
                b.extend_from_slice(&(duration as u32).to_be_bytes());
                b.extend_from_slice(&[0u8; 8]);
                b.extend_from_slice(&0u16.to_be_bytes()); // layer
                b.extend_from_slice(&0u16.to_be_bytes()); // alternate group
                b.extend_from_slice(&(if is_audio { 0x0100u16 } else { 0 }).to_be_bytes());
                b.extend_from_slice(&[0u8; 2]);
                matrix(b);
                let (w, h) = if is_video { (self.video.width, self.video.height) } else { (0, 0) };
                b.extend_from_slice(&(w << 16).to_be_bytes());
                b.extend_from_slice(&(h << 16).to_be_bytes());
            });
            body(t);
        });
    }

    fn video_entry(&self, b: &mut Vec<u8>) {
        let v = &self.video;
        let fourcc = if self.dnx.is_some() { *b"AVdh" } else { v.profile.fourcc() };
        atom(b, &fourcc, |e| {
            e.extend_from_slice(&[0u8; 6]);
            e.extend_from_slice(&1u16.to_be_bytes()); // data reference
            e.extend_from_slice(&0u16.to_be_bytes()); // version
            e.extend_from_slice(&0u16.to_be_bytes()); // revision
            e.extend_from_slice(if self.dnx.is_some() { b"\0\0\0\0" } else { b"appl" });
            e.extend_from_slice(&0u32.to_be_bytes()); // temporal quality
            e.extend_from_slice(&0x0000_0200u32.to_be_bytes()); // spatial quality
            e.extend_from_slice(&(v.width as u16).to_be_bytes());
            e.extend_from_slice(&(v.height as u16).to_be_bytes());
            e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
            e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
            e.extend_from_slice(&0u32.to_be_bytes());
            e.extend_from_slice(&1u16.to_be_bytes()); // frames per sample
            let name = if let Some(d) = self.dnx { d.name() } else { match v.profile {
                studio_prores::Profile::Proxy => "Apple ProRes 422 Proxy",
                studio_prores::Profile::Lt => "Apple ProRes 422 LT",
                studio_prores::Profile::Standard => "Apple ProRes 422",
                studio_prores::Profile::Hq => "Apple ProRes 422 HQ",
                studio_prores::Profile::P4444 => "Apple ProRes 4444",
                studio_prores::Profile::P4444Xq => "Apple ProRes 4444 XQ",
            } };
            let mut cn = [0u8; 32];
            cn[0] = name.len() as u8;
            cn[1..1 + name.len()].copy_from_slice(name.as_bytes());
            e.extend_from_slice(&cn);
            e.extend_from_slice(&(if v.alpha { 32u16 } else { 24 }).to_be_bytes());
            e.extend_from_slice(&(-1i16).to_be_bytes());
            atom(e, b"colr", |c| {
                c.extend_from_slice(b"nclc");
                c.extend_from_slice(&v.colour.0.to_be_bytes());
                c.extend_from_slice(&v.colour.1.to_be_bytes());
                c.extend_from_slice(&v.colour.2.to_be_bytes());
            });
            atom(e, b"fiel", |f| f.extend_from_slice(&[1, 0]));
            atom(e, b"pasp", |p| {
                p.extend_from_slice(&1u32.to_be_bytes());
                p.extend_from_slice(&1u32.to_be_bytes());
            });
        });
    }
}

fn lpcm_entry(b: &mut Vec<u8>, a: AudioTrack) {
    atom(b, b"lpcm", |e| {
        e.extend_from_slice(&[0u8; 6]);
        e.extend_from_slice(&1u16.to_be_bytes());
        e.extend_from_slice(&2u16.to_be_bytes()); // version 2
        e.extend_from_slice(&0u16.to_be_bytes()); // revision
        e.extend_from_slice(&0u32.to_be_bytes()); // vendor
        e.extend_from_slice(&3u16.to_be_bytes()); // always 3
        e.extend_from_slice(&16u16.to_be_bytes()); // always 16
        e.extend_from_slice(&(-2i16).to_be_bytes()); // always -2
        e.extend_from_slice(&0u16.to_be_bytes()); // always 0
        e.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // always 65536
        e.extend_from_slice(&72u32.to_be_bytes()); // size of struct only
        e.extend_from_slice(&(a.sample_rate as f64).to_be_bytes());
        e.extend_from_slice(&(a.channels as u32).to_be_bytes());
        e.extend_from_slice(&0x7F00_0000u32.to_be_bytes());
        e.extend_from_slice(&24u32.to_be_bytes());
        // Signed integer | big endian | packed.
        e.extend_from_slice(&(0x4u32 | 0x2 | 0x8).to_be_bytes());
        e.extend_from_slice(&(3 * a.channels as u32).to_be_bytes());
        e.extend_from_slice(&1u32.to_be_bytes());
    });
}

fn atom(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn full(out: &mut Vec<u8>, kind: &[u8; 4], version: u8, flags: u32, body: impl FnOnce(&mut Vec<u8>)) {
    atom(out, kind, |b| {
        b.push(version);
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        body(b);
    });
}

fn matrix(b: &mut Vec<u8>) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        b.extend_from_slice(&v.to_be_bytes());
    }
}

fn pascal(b: &mut Vec<u8>, s: &str) {
    b.push(s.len().min(255) as u8);
    b.extend_from_slice(&s.as_bytes()[..s.len().min(255)]);
}

fn mdhd(out: &mut Vec<u8>, timescale: u32, duration: u64) {
    full(out, b"mdhd", 0, 0, |b| {
        b.extend_from_slice(&[0u8; 8]);
        b.extend_from_slice(&timescale.to_be_bytes());
        b.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
        b.extend_from_slice(&0x7FFFu16.to_be_bytes()); // language: unspecified (Macintosh)
        b.extend_from_slice(&0u16.to_be_bytes()); // quality
    });
}

fn hdlr(out: &mut Vec<u8>, ctype: &[u8; 4], subtype: &[u8; 4], name: &str) {
    full(out, b"hdlr", 0, 0, |b| {
        b.extend_from_slice(ctype);
        b.extend_from_slice(subtype);
        b.extend_from_slice(b"appl");
        b.extend_from_slice(&[0u8; 8]);
        pascal(b, name);
    });
}

fn dinf(out: &mut Vec<u8>) {
    atom(out, b"dinf", |d| {
        full(d, b"dref", 0, 0, |b| {
            b.extend_from_slice(&1u32.to_be_bytes());
            full(b, b"alis", 0, 1, |_| {});
        });
    });
}

fn stts(out: &mut Vec<u8>, entries: &[(u32, u32)]) {
    full(out, b"stts", 0, 0, |b| {
        b.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (n, d) in entries {
            b.extend_from_slice(&n.to_be_bytes());
            b.extend_from_slice(&d.to_be_bytes());
        }
    });
}

fn stsc(out: &mut Vec<u8>, runs: &[(u32, u32)]) {
    full(out, b"stsc", 0, 0, |b| {
        b.extend_from_slice(&(runs.len() as u32).to_be_bytes());
        for (first, n) in runs {
            b.extend_from_slice(&first.to_be_bytes());
            b.extend_from_slice(&n.to_be_bytes());
            b.extend_from_slice(&1u32.to_be_bytes());
        }
    });
}

fn stsz(out: &mut Vec<u8>, fixed: u32, sizes: &[u32]) {
    full(out, b"stsz", 0, 0, |b| {
        b.extend_from_slice(&fixed.to_be_bytes());
        b.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for s in sizes {
            b.extend_from_slice(&s.to_be_bytes());
        }
    });
}

fn stsz_fixed(out: &mut Vec<u8>, size: u32, count: u32) {
    full(out, b"stsz", 0, 0, |b| {
        b.extend_from_slice(&size.to_be_bytes());
        b.extend_from_slice(&count.to_be_bytes());
    });
}

fn co64(out: &mut Vec<u8>, offsets: &[u64]) {
    full(out, b"co64", 0, 0, |b| {
        b.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
        for o in offsets {
            b.extend_from_slice(&o.to_be_bytes());
        }
    });
}
