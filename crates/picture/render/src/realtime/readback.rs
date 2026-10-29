// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading rendered frames back to the CPU with pooled staging buffers, rows
//! unpadded into a caller-owned `Vec`.
//!
//! Two paths. The exact path (`sync_target` / `read_sync`) reuses one buffer
//! per size and waits for the GPU: the frame asked for, as export needs. The
//! ring path (`ring_target` / `ring_latest`) copies frame N into slot
//! N % `RING`, starts mapping it and hands back the newest frame that has
//! already finished, normally N-1, with a non-blocking poll. It waits only
//! on the first frame after the size changes, so callers always get a
//! picture, and when the GPU is more than `RING` frames behind.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Slots in the ring.
pub const RING: usize = 3;

/// Bytes per texture row once padded for a buffer copy.
pub fn padded_row(width: u32, bpp: u32) -> u32 {
    (width * bpp).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

fn make_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false })
}

/// Copies the padded rows of `data` into `out`, `row` bytes each.
fn unpad(data: &[u8], padded: usize, row: usize, height: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(row * height);
    if padded == row {
        out.extend_from_slice(&data[..row * height]);
    } else {
        for y in 0..height {
            out.extend_from_slice(&data[y * padded..y * padded + row]);
        }
    }
}

/// Counters, for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadbackStats {
    /// Calls that waited for the GPU (`PollType::Wait`).
    pub blocking_waits: u64,
    /// Non-blocking polls.
    pub polls: u64,
    /// Staging buffers created.
    pub buffers_created: u64,
}

enum State {
    Idle,
    /// `map_async` asked; the flag is 0 until the callback ran, then 1 (ok) or 2 (failed).
    Mapping { seq: u64, flag: Arc<AtomicU8> },
    Mapped { seq: u64 },
}

struct Slot {
    buf: wgpu::Buffer,
    state: State,
}

/// What a ring read handed back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Latest {
    pub width: u32,
    pub height: u32,
    /// Frames between the one recorded and the one returned: 0 on the first
    /// frame after a size change, normally 1.
    pub lag: u64,
    /// False when no frame had finished yet; the caller's `Vec` is untouched.
    pub fresh: bool,
}

/// Pooled staging buffers for reading frames back.
#[derive(Default)]
pub struct Readback {
    ring: Vec<Slot>,
    ring_key: (u32, u32, u32),
    /// Sequence number of the next frame recorded into the ring.
    next: u64,
    /// A frame has been handed out since the ring was (re)built.
    warm: bool,
    sync: Option<(u64, wgpu::Buffer)>,
    /// Layout of the buffer last handed out by `sync_target`.
    layout: (u32, u32, u32),
    stats: ReadbackStats,
}

impl Readback {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> ReadbackStats {
        self.stats
    }

    /// The staging buffer for an exact read of a `width × height` texture of
    /// `bpp` bytes a pixel, and its padded row length. Recorded copies must
    /// use that row length; the buffer is only recreated when the size
    /// changes.
    pub fn sync_target(&mut self, device: &wgpu::Device, width: u32, height: u32, bpp: u32) -> (&wgpu::Buffer, u32) {
        let padded = padded_row(width, bpp);
        let size = padded as u64 * height as u64;
        if self.sync.as_ref().is_none_or(|(s, _)| *s != size) {
            self.sync = Some((size, make_buffer(device, size)));
            self.stats.buffers_created += 1;
        }
        self.layout = (width, height, bpp);
        (&self.sync.as_ref().unwrap().1, padded)
    }

    /// Waits for the submitted copy into the `sync_target` buffer and
    /// unpads it into `out`.
    pub fn read_sync(&mut self, device: &wgpu::Device, out: &mut Vec<u8>) -> Result<(), String> {
        let (width, height, bpp) = self.layout;
        let buffer = &self.sync.as_ref().ok_or("no readback buffer")?.1;
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.stats.blocking_waits += 1;
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("reading the frame back: {e:?}"))?;
        let data = slice.get_mapped_range().map_err(|e| format!("reading the frame back: {e:?}"))?;
        unpad(&data, padded_row(width, bpp) as usize, (width * bpp) as usize, height as usize, out);
        drop(data);
        buffer.unmap();
        Ok(())
    }

    /// Moves finished maps from `Mapping` to `Mapped`.
    fn promote(&mut self) -> Result<(), String> {
        for slot in &mut self.ring {
            if let State::Mapping { seq, flag } = &slot.state {
                match flag.load(Ordering::Acquire) {
                    0 => {}
                    1 => slot.state = State::Mapped { seq: *seq },
                    _ => {
                        slot.state = State::Idle;
                        return Err("reading the frame back: map failed".into());
                    }
                }
            }
        }
        Ok(())
    }

    /// The ring slot the next frame is copied into, rebuilt when the size
    /// changes, and its padded row length. Frees the slot first: normally it
    /// is long finished, and only a GPU more than `RING` frames behind makes
    /// this wait.
    pub fn ring_target(&mut self, device: &wgpu::Device, width: u32, height: u32, bpp: u32) -> Result<(&wgpu::Buffer, u32), String> {
        let padded = padded_row(width, bpp);
        if self.ring.is_empty() || self.ring_key != (width, height, bpp) {
            let size = padded as u64 * height as u64;
            self.ring = (0..RING).map(|_| Slot { buf: make_buffer(device, size), state: State::Idle }).collect();
            self.stats.buffers_created += RING as u64;
            self.ring_key = (width, height, bpp);
            self.next = 0;
            self.warm = false;
        }
        let i = (self.next % RING as u64) as usize;
        if matches!(self.ring[i].state, State::Mapping { .. }) {
            self.stats.polls += 1;
            let _ = device.poll(wgpu::PollType::Poll);
            self.promote()?;
        }
        if matches!(self.ring[i].state, State::Mapping { .. }) {
            self.stats.blocking_waits += 1;
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            self.promote()?;
        }
        let slot = &mut self.ring[i];
        if matches!(slot.state, State::Mapped { .. }) {
            slot.buf.unmap();
            slot.state = State::Idle;
        }
        Ok((&slot.buf, padded))
    }

    /// After the copy into the `ring_target` slot was submitted: starts
    /// mapping it and unpads the newest earlier frame that has finished into
    /// `out`. Polls without waiting, except for the first frame after the
    /// ring was built, which is waited for so there is always a picture.
    pub fn ring_latest(&mut self, device: &wgpu::Device, out: &mut Vec<u8>) -> Result<Latest, String> {
        let (width, height, bpp) = self.ring_key;
        let cur = self.next;
        self.next += 1;
        let flag = Arc::new(AtomicU8::new(0));
        let slot = &mut self.ring[(cur % RING as u64) as usize];
        let f = flag.clone();
        slot.buf.slice(..).map_async(wgpu::MapMode::Read, move |r| f.store(if r.is_ok() { 1 } else { 2 }, Ordering::Release));
        slot.state = State::Mapping { seq: cur, flag };
        self.stats.polls += 1;
        let _ = device.poll(wgpu::PollType::Poll);
        self.promote()?;
        let newest = |ring: &[Slot], below: u64| ring.iter().enumerate().filter_map(|(i, s)| if let State::Mapped { seq } = s.state { (seq < below).then_some((seq, i)) } else { None }).max();
        let mut pick = newest(&self.ring, cur);
        if pick.is_none() && !self.warm {
            self.stats.blocking_waits += 1;
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            self.promote()?;
            pick = newest(&self.ring, cur + 1);
        }
        let Some((seq, i)) = pick else {
            return Ok(Latest { width, height, lag: 0, fresh: false });
        };
        let slot = &self.ring[i];
        let data = slot.buf.slice(..).get_mapped_range().map_err(|e| format!("reading the frame back: {e:?}"))?;
        unpad(&data, padded_row(width, bpp) as usize, (width * bpp) as usize, height as usize, out);
        drop(data);
        // The one handed out and anything older are done with, except the
        // frame just recorded (first frame): the next call hands it out again.
        for s in &mut self.ring {
            if matches!(s.state, State::Mapped { seq: q } if q < seq || (q == seq && seq != cur)) {
                s.buf.unmap();
                s.state = State::Idle;
            }
        }
        self.warm = true;
        Ok(Latest { width, height, lag: cur - seq, fresh: true })
    }
}
