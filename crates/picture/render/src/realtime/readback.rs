// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading rendered frames back to the CPU with pooled staging buffers: one
//! buffer reused for every exact (synchronous) read of a size, rows unpadded
//! into a caller-owned `Vec`.

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

/// Pooled staging buffers for reading frames back.
#[derive(Default)]
pub struct Readback {
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
}
