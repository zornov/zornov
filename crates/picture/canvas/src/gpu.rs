// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compositing on the GPU: the pixel engine's wgpu compositor (layer pages
//! resident on the GPU, only touched tiles re-uploaded, every blend mode,
//! mask, clipping group, adjustment and layer effect in shaders) on the
//! device Studio already has. What it cannot do (rare documents) falls
//! back to the CPU compositor, which gives the same pixels.

use studio_pixel_compose::Buffer;
use studio_pixel_geom::Rect;
use studio_pixel_raster::Rgba8Image;

use crate::Canvas;

pub struct GpuView {
    comp: studio_pixel_gpu::Compositor,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Why the last render fell back to the CPU, if it did.
    pub fallback: Option<String>,
}

impl GpuView {
    /// A compositor on `device`; `None` if its pipelines do not build there.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Option<GpuView> {
        let comp = studio_pixel_gpu::Compositor::try_new_with_format(&device, wgpu::TextureFormat::Rgba16Float).ok()?;
        Some(GpuView { comp, device, queue, fallback: None })
    }

    /// The composite of `rect` (document pixels) at full resolution.
    pub fn render(&mut self, c: &Canvas, rect: (i32, i32, u32, u32)) -> Result<Rgba8Image, String> {
        let doc = c.doc().ok_or("no picture open")?;
        let r = Rect::from_xywh(rect.0, rect.1, rect.2.max(1), rect.3.max(1)).intersect(&doc.bounds());
        if r.is_empty() {
            return Ok(Rgba8Image::new(1, 1));
        }
        match studio_pixel_gpu::render_to_vec(&mut self.comp, &self.device, &self.queue, doc, r) {
            Ok(px) => {
                self.fallback = None;
                Ok(Buffer { rect: Rect::from_xywh(0, 0, r.width() as u32, r.height() as u32), px }.to_rgba8())
            }
            Err(e) => {
                self.fallback = Some(e.0.clone());
                Ok(studio_pixel_compose::render_reduced_rect(doc, r, r.width() as u32, r.height() as u32).to_rgba8())
            }
        }
    }

    /// Forget a closed picture's pages.
    pub fn forget(&mut self, doc: studio_pixel_doc::DocId) {
        self.comp.forget_doc(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::NoVision;
    use serde_json::json;

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        pollster_block(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() })).ok()
    }

    /// A tiny executor for the two awaits above.
    fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        let mut f = std::pin::pin!(f);
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn the_gpu_composite_matches_the_cpu_one() {
        let Some((d, q)) = device() else {
            eprintln!("no GPU here; skipped");
            return;
        };
        let Some(mut g) = GpuView::new(d, q) else {
            eprintln!("the compositor does not build on this GPU; skipped");
            return;
        };
        let mut c = Canvas::new(None);
        crate::ops::apply(
            &mut c,
            &[
                json!({"op": "new", "width": 200, "height": 120, "background": "#336699"}),
                json!({"op": "layer", "action": "new", "name": "Paint", "blend": "Multiply", "opacity": 80}),
                json!({"op": "paint", "path": "M10 60 C 60 0 140 120 190 60", "size": 18, "color": "#e08040"}),
                json!({"op": "adjust", "kind": "hue_saturation", "saturation": 30}),
            ],
            &mut NoVision,
        )
        .unwrap();
        let gpu = g.render(&c, (0, 0, 200, 120)).unwrap();
        let cpu = c.view((0, 0, 200, 120), 200, 120).unwrap();
        assert_eq!((gpu.width, gpu.height), (200, 120));
        let worst = gpu.pixels.iter().zip(&cpu.pixels).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 3, "GPU and CPU agree within rounding (worst {worst}, fallback {:?})", g.fallback);
    }
}
