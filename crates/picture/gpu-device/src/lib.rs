// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The process's one GPU device.
//!
//! Every GPU pass outside the window (the renderer, the vector rasterizer,
//! the formula tracer, the path tracer, the air pass) used to open its own
//! device. Several devices in one process crash some drivers: WARP, the
//! software DX12 adapter on Windows machines without a GPU (and on the CI
//! runners), faults inside `d3d10warp.dll` when two of its devices render
//! at once, and the Vulkan loader crashes when devices open and drop on
//! several threads. So there is one device, opened on first use and never
//! dropped, and every pass builds its pipelines on it.
//!
//! wgpu's error scopes are per thread, so passes sharing the device can
//! push and pop them without seeing each other's errors.

use std::sync::OnceLock;

/// The shared device and what the passes need to know about its adapter.
pub struct Shared {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    /// The adapter is a CPU one (WARP, llvmpipe, lavapipe).
    pub software: bool,
    /// The device's limits (the adapter's best).
    pub limits: wgpu::Limits,
}

/// The process's device, or `None` when there is no usable GPU or
/// `STUDIO_RENDER_NO_GPU` is set (the tests and benchmarks use it to force
/// the CPU paths).
pub fn shared() -> Option<&'static Shared> {
    if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
        return None;
    }
    static SHARED: OnceLock<Option<Shared>> = OnceLock::new();
    SHARED.get_or_init(open).as_ref()
}

fn open() -> Option<Shared> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    if cfg!(windows) && wgpu::Backends::from_env().is_none() {
        // DirectX 12, as the window draws (app/src/main.rs): opening a Vulkan
        // device faults inside some drivers (Intel UHD), and tests never pass
        // through main to be told.
        desc.backends = wgpu::Backends::DX12;
    }
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let info = adapter.get_info();
    let limits = adapter.limits();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("studio"),
        required_features: wgpu::Features::empty(),
        required_limits: limits.clone(),
        ..Default::default()
    }))
    .ok()?;
    Some(Shared { device, queue, adapter_name: info.name, software: info.device_type == wgpu::DeviceType::Cpu, limits })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_caller_gets_the_same_device() {
        let (Some(a), Some(b)) = (shared(), shared()) else {
            return; // no GPU here
        };
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn threads_share_it_too() {
        let ids: Vec<_> = (0..4).map(|_| std::thread::spawn(|| shared().map(|s| s as *const Shared as usize))).map(|t| t.join().unwrap()).collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
    }
}
