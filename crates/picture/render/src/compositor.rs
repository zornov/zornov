// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The CPU compositor: the reference implementation of the pipeline, used
//! for golden tests, export on machines without a usable GPU, and as the
//! fallback the GPU path compares itself against.
//!
//! Per output frame: resolve the scene, start from the background, then
//! for each node bottom to top fetch the picture (decode cache, generator
//! or nested render), crop → effects → grade → masks → pins (puppet, corner pin) →
//! motion blur →
//! transform → opacity → blend. Transitions draw both sides on their own
//! transparent canvases and combine them; adjustment clips process the
//! canvas so far.

use std::sync::Arc;

use crate::blend::composite;
use crate::effects::{self, EffectCtx};
use crate::filter::{gaussian_blur, rotate, scale_px};
use crate::frame::Frame;
use crate::mask::{self, PreparedMask};
use crate::output::{EncodedFrame, OutputDepth, OutputSettings, encode};
use crate::puppet;
use crate::reuse::{Content, ContentKey, EntryId, LayerCache, ReuseStats, media_picture};
use crate::resolve::{Node, RenderList, resolve};
use crate::scene::{Canvas, ClipDesc, Micros, SECOND, Scene, SourceDesc};
use crate::shadow;
use crate::source::Sources;
use crate::space::{self, CameraDesc, Caster, LightDesc, NEAR, Proj};
use crate::transform::{Affine, Homography, Pose, crop_rect, layer_matrix, layer_quad, proj_quad, quad_bounds, visible_rect};
use crate::yuv::PictureRef;

/// Something that renders a scene at a time to a frame.
pub trait FrameRenderer: Send {
    /// Renders scene time `t` at `target` pixels (`None` = canvas size).
    /// The target keeps the canvas aspect; the preview passes the viewer's
    /// pixel size, never larger than the canvas.
    fn render(&mut self, scene: &dyn Scene, t: Micros, target: Option<(u32, u32)>) -> Frame;

    /// Renders and converts to the output space in one go. The GPU does the
    /// conversion in a shader and reads back the small encoded buffer; the
    /// default goes through [`FrameRenderer::render`] and the CPU encoder.
    fn render_encoded(&mut self, scene: &dyn Scene, t: Micros, target: Option<(u32, u32)>, settings: &OutputSettings, depth: OutputDepth, frame_index: u32) -> EncodedFrame {
        let frame = self.render(scene, t, target);
        encode(&frame, settings, depth, frame_index)
    }

    /// The decode and media access this renderer uses.
    fn sources(&mut self) -> &mut Sources;

    fn name(&self) -> &'static str;
}

/// The size to draw at for `settings`: export may ask for more than the
/// canvas (a 4K master of a 1080p project, up to 8192 px a side, aspect
/// kept); the preview never upscales.
pub fn output_size(canvas: &Canvas, target: Option<(u32, u32)>, settings: &OutputSettings) -> (u32, u32) {
    match target {
        Some((tw, th)) if settings.allow_upscale && (tw > canvas.width || th > canvas.height) => {
            let (tw, th) = (tw.clamp(1, 8192), th.clamp(1, 8192));
            let s = (tw as f64 / canvas.width.max(1) as f64).min(th as f64 / canvas.height.max(1) as f64);
            (((canvas.width as f64 * s).round() as u32).max(1), ((canvas.height as f64 * s).round() as u32).max(1))
        }
        _ => target_size(canvas, target),
    }
}

/// Clamps a requested size to the canvas and keeps its aspect ratio.
pub fn target_size(canvas: &Canvas, target: Option<(u32, u32)>) -> (u32, u32) {
    let Some((tw, th)) = target else { return (canvas.width, canvas.height) };
    let tw = tw.clamp(1, canvas.width.max(1));
    let th = th.clamp(1, canvas.height.max(1));
    let s = (tw as f64 / canvas.width as f64).min(th as f64 / canvas.height as f64);
    (((canvas.width as f64 * s).round() as u32).max(1), ((canvas.height as f64 * s).round() as u32).max(1))
}

/// A layer's picture ready to be placed.
pub struct LayerPicture {
    /// The processed picture (cropped region, effects, grade, masks),
    /// shared with the layer cache and with identical copies.
    pub image: Arc<Frame>,
    /// Map from image pixels to target pixels.
    pub matrix: Affine,
    /// Visible rectangle in image pixels after the pose's clip insets.
    pub visible: [f32; 4],
    /// Combined clip and pose opacity.
    pub opacity: f32,
    /// Set when the layer is drawn in perspective (3D); `matrix` is then
    /// unused.
    pub proj: Option<ProjLayer>,
}

/// The perspective part of a [`LayerPicture`].
#[derive(Clone, Copy, Debug)]
pub struct ProjLayer {
    pub proj: Proj,
    /// What it is seen through, and the render scale (target px per
    /// canvas px), to place extruded slices.
    pub camera: CameraDesc,
    pub scale: f32,
    /// Canvas size in canvas pixels.
    pub canvas: [f32; 2],
    /// Clip id (a layer does not shadow itself).
    pub id: u64,
}

impl ProjLayer {
    /// Target-pixel map of the picture pushed `d` pixels back.
    pub fn slice(&self, d: f32) -> (Homography, space::Plane) {
        if d == 0.0 {
            return (self.proj.h, self.proj.plane);
        }
        let plane = self.proj.plane.pushed(d);
        (Homography::from_affine(&Affine::scale(self.scale, self.scale)).then_after(&plane.homography(&self.camera, self.canvas)), plane)
    }

    /// Depths (pixels back) of the face and the extruded slices behind
    /// it, far to near, each with its shade.
    pub fn slices(&self, visible: [f32; 4]) -> Vec<(f32, f32)> {
        let e = self.proj.material.extrude;
        let mut out = vec![(0.0, 1.0)];
        if e > 0.0 {
            let n = ((e / 2.0).round() as usize).clamp(2, 48);
            let side = self.proj.material.side_shade;
            out.extend((1..=n).map(|k| (e * k as f32 / n as f32, side)));
        }
        let cam = self.camera;
        let depth = |d: f32| space::plane_depth(&cam, &self.proj.plane.pushed(d), visible);
        out.sort_by(|a, b| depth(b.0).partial_cmp(&depth(a.0)).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// The lights and shadow casters a 3D run is drawn with.
pub struct Lighting<'a> {
    pub lights: &'a [LightDesc],
    pub casters: Vec<Caster>,
    pub caster_images: Vec<&'a Frame>,
}

/// Geometry of a layer without its pixels: the map from the full source
/// to target pixels and the visible source rectangle.
pub struct LayerGeometry {
    pub source_size: [f32; 2],
    pub matrix: Affine,
    pub visible: [f32; 4],
    /// The perspective map (source px → target px) of a 3D layer.
    pub proj: Option<Homography>,
    /// Depth of its centre in front of the lens (3D layers).
    pub depth: Option<f32>,
}

impl LayerGeometry {
    /// Canvas outline: top-left, top-right, bottom-right, bottom-left.
    pub fn quad(&self) -> [[f32; 2]; 4] {
        match &self.proj {
            Some(h) => proj_quad(h, self.visible, NEAR),
            None => layer_quad(&self.matrix, self.visible),
        }
    }

    /// Whether a point (target pixels) lands on the layer.
    pub fn hit(&self, point: [f32; 2]) -> bool {
        match &self.proj {
            Some(h) => crate::transform::hit_proj(h, self.visible, point, NEAR),
            None => crate::transform::hit(&self.matrix, self.visible, point),
        }
    }
}

/// The size a clip's source is treated as having (display size after
/// rotation for media; the canvas for generators; the nested canvas for
/// compound clips).
pub fn source_size(sources: &mut Sources, clip: &ClipDesc, canvas: &Canvas) -> Option<[f32; 2]> {
    match &clip.source {
        SourceDesc::Media { media, .. } => {
            let info = sources.info(*media)?;
            let (w, h) = info.display_size();
            Some([w as f32, h as f32])
        }
        SourceDesc::Nested { scene, .. } => {
            let c = scene.canvas();
            Some([c.width as f32, c.height as f32])
        }
        SourceDesc::Generator(_) | SourceDesc::Mesh(_) | SourceDesc::World(_) => Some(canvas.size()),
        SourceDesc::Adjustment | SourceDesc::Camera(_) | SourceDesc::Light(_) => None,
    }
}

/// Geometry of a clip at render scale `s` (target pixels per canvas pixel).
/// `camera` is what 3D layers are seen through.
pub fn layer_geometry(sources: &mut Sources, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, camera: &CameraDesc, s: f32) -> Option<LayerGeometry> {
    let source = source_size(sources, clip, canvas)?;
    let m = layer_matrix(canvas.size(), source, &clip.placement, pose);
    let matrix = Affine::scale(s, s).then_after(&m);
    let visible = visible_rect(source, clip.placement.crop, pose.clip);
    let (proj, depth) = match space::source_plane(canvas, source, clip, pose, camera) {
        Some((plane, cam)) => {
            let h = Homography::from_affine(&Affine::scale(s, s)).then_after(&plane.homography(&cam, canvas.size()));
            (Some(h), Some(space::plane_depth(&cam, &plane, visible)))
        }
        None => (None, None),
    };
    Some(LayerGeometry { source_size: source, matrix, visible, proj, depth })
}

/// The passes of a 2D layer's ground shadow, for a layer drawn with
/// `matrix` over `visible` at `opacity` (see [`crate::shadow`]). A matte
/// layer casts none.
#[allow(clippy::too_many_arguments)]
pub fn shadow_passes(sources: &mut Sources, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), camera: &CameraDesc, matrix: &Affine, visible: [f32; 4], opacity: f32, sh: &shadow::ShadowDesc) -> Vec<shadow::Pass> {
    if clip.blend.is_matte() {
        return Vec::new();
    }
    let s = target.0 as f32 / canvas.width.max(1) as f32;
    let Some(geo) = layer_geometry(sources, clip, pose, canvas, camera, s) else { return Vec::new() };
    let (feet, height) = shadow::anchor(sh, &geo.matrix, geo.source_size, clip.placement.crop, s);
    shadow::passes(sh, matrix, visible, opacity, feet, height, target)
}

/// Blur (target px, a Gaussian sigma) that depth of field gives a layer
/// at `depth`.
pub fn dof_sigma(camera: &CameraDesc, depth: f32, s: f32) -> f32 {
    match camera.dof {
        Some(d) if depth > NEAR => 0.5 * d.aperture * d.blur_level * (depth - d.focus).abs() / depth * s,
        _ => 0.0,
    }
}

/// Whether a node draws inside a 3D run (sorted by depth with its
/// neighbours).
pub fn in_space(node: &Node) -> bool {
    matches!(node, Node::Layer { clip, .. } if clip.space.is_some() && !clip.blend.is_matte())
}

/// Lights a premultiplied pixel of a 3D layer at picture px `uv`.
#[inline]
pub fn light_pixel(src: [f32; 4], uv: [f32; 2], plane: &space::Plane, normal: space::V3, pl: &ProjLayer, side: f32, env: &Lighting) -> [f32; 4] {
    if !pl.proj.lit {
        return if side == 1.0 { src } else { [src[0] * side, src[1] * side, src[2] * side, src[3]] };
    }
    let world = plane.at(uv);
    space::shade(src, world, normal, pl.camera.position, &pl.proj.material, env.lights, side, |li| {
        space::shadow(world, &env.lights[li], &env.casters, pl.id, |ci, u, v| env.caster_images[ci].sample(u, v)[3])
    })
}

/// Bytes of processed layers a CPU compositor keeps by default.
pub const CPU_CACHE_BUDGET: usize = 384 << 20;

/// Reference CPU renderer.
pub struct CpuCompositor {
    sources: Sources,
    /// Processed layer pictures kept between frames.
    cache: LayerCache<Arc<Frame>>,
    /// Recursion guard for nested scenes.
    depth: u32,
}

impl CpuCompositor {
    pub fn new(sources: Sources) -> CpuCompositor {
        CpuCompositor::with_budget(sources, CPU_CACHE_BUDGET)
    }

    /// A compositor keeping up to `budget` bytes of processed layers.
    pub fn with_budget(sources: Sources, budget: usize) -> CpuCompositor {
        CpuCompositor { sources, cache: LayerCache::new(budget), depth: 0 }
    }

    /// Layer cache counters.
    pub fn reuse_stats(&self) -> ReuseStats {
        self.cache.stats()
    }

    /// Fetches, rotates and scales the raw picture of a clip. Returns the
    /// picture and how many source pixels one image pixel covers.
    fn raw_picture(&mut self, clip: &ClipDesc, canvas: &Canvas, target: (u32, u32), picture: Option<(PictureRef, u32)>) -> Option<(Frame, f32)> {
        match &clip.source {
            SourceDesc::Media { .. } => {
                let (pic, rotation) = picture?;
                let mut frame = pic.to_frame(!canvas.output.is_hdr());
                if let Some((m, invert)) = crate::reuse::matte_picture(&mut self.sources, clip) {
                    crate::matte::apply(&mut frame, &m, invert);
                }
                if rotation != 0 {
                    frame = rotate(&frame, rotation);
                }
                let k = crate::reuse::media_scale(&mut self.sources, clip, frame.width);
                Some((frame, k))
            }
            SourceDesc::Generator(g) => {
                let frame = effects::generate(g, target.0, target.1);
                Some((frame, canvas.width as f32 / target.0 as f32))
            }
            SourceDesc::Mesh(m) => {
                let frame = crate::mesh_render::render(m, canvas, target);
                Some((frame, canvas.width as f32 / target.0 as f32))
            }
            SourceDesc::World(w) => {
                let frame = crate::world_scene::render(w, canvas, target);
                Some((frame, canvas.width as f32 / target.0 as f32))
            }
            SourceDesc::Nested { scene, time } => {
                if self.depth >= 8 {
                    return None;
                }
                let nested = scene.canvas();
                let s = target.0 as f32 / canvas.width.max(1) as f32;
                let size = (((nested.width as f32 * s).round() as u32).max(1), ((nested.height as f32 * s).round() as u32).max(1));
                self.depth += 1;
                let frame = self.render_scene(scene.as_ref(), *time, size);
                self.depth -= 1;
                Some((frame, nested.width as f32 / size.0 as f32))
            }
            SourceDesc::Adjustment | SourceDesc::Camera(_) | SourceDesc::Light(_) => None,
        }
    }

    /// A layer's processed picture before placement: crop, effects, grade,
    /// masks and pins. Kept in the layer cache and reused while its key
    /// stays the same (see [`crate::reuse`]): a step added here must put
    /// what it reads into [`ContentKey`].
    fn content(&mut self, clip: &ClipDesc, canvas: &Canvas, target: (u32, u32), t: Micros) -> Option<(Content<Arc<Frame>>, Option<EntryId>)> {
        let picture = media_picture(&mut self.sources, clip);
        let key = ContentKey::of(clip, canvas, target, t, picture.as_ref());
        if let Some(key) = &key
            && let Some((c, id)) = self.cache.get(key)
        {
            return Some((c, Some(id)));
        }
        let (raw, k) = self.raw_picture(clip, canvas, target, picture)?;
        let source_size = [raw.width as f32 * k, raw.height as f32 * k];
        // Crop in image pixels.
        let crop_img = crop_rect([raw.width as f32, raw.height as f32], clip.placement.crop);
        let (cx, cy) = (crop_img[0].floor() as u32, crop_img[1].floor() as u32);
        let (cw, ch) = (((crop_img[2] - crop_img[0]).round() as u32).max(1), ((crop_img[3] - crop_img[1]).round() as u32).max(1));
        let mut image = if cx == 0 && cy == 0 && cw == raw.width && ch == raw.height { raw } else { raw.crop(cx, cy, cw, ch) };
        // Effects and grade at the clip's own time.
        let rel = (t - clip.start).max(0);
        let ctx = EffectCtx { time: rel as f32 / SECOND as f32, frame_rate: canvas.frame_rate as f32, seed: clip.seed };
        if !clip.effects.is_empty() {
            image = effects::apply_effects(image, &clip.effects, ctx);
        }
        if let Some(g) = &clip.grade {
            image = effects::apply_grade(image, g, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        }
        image = crate::nodes::apply_nodes(image, &clip.grade_nodes, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        if !clip.masks.is_empty() {
            mask::apply(&mut image, &clip.masks);
        }
        // Puppet and corner pins bend the picture; it may grow past its
        // edges, so it gets a new origin.
        let mut origin = [0.0f32; 2];
        if let Some(w) = clip.warp.as_ref().filter(|w| !w.is_identity()) {
            let occ = w.needs_occupancy().then(|| puppet::occupancy_cpu(&image, puppet::OccupancyGrid::for_size(image.width, image.height)));
            match puppet::build_mesh(w, image.width, image.height, occ.as_deref()) {
                Some(mesh) => {
                    image = puppet::warp_cpu(&image, &mesh);
                    origin = mesh.origin;
                }
                None => image = Frame::new(1, 1),
            }
        }
        let content = Content { image: Arc::new(image), k, origin, source_size };
        let id = key.and_then(|key| self.cache.insert(key, content.clone()).1);
        Some((content, id))
    }

    /// `image` halved for the `level`-th time, from the cache when the
    /// content is kept there.
    fn halved(&mut self, image: &Frame, kept: Option<EntryId>, level: usize) -> Arc<Frame> {
        if let Some(m) = kept.and_then(|id| self.cache.mip(id, level)) {
            return m;
        }
        let half = Arc::new(image.half());
        if let Some(id) = kept {
            let _ = self.cache.add_mip(id, level, half.clone());
        }
        half
    }

    /// Builds the processed, placed picture of a layer; `camera` is what
    /// a 3D layer is seen through and `lit` whether lights fall on it.
    pub fn layer_picture(&mut self, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc, lit: bool) -> Option<LayerPicture> {
        let s = target.0 as f32 / canvas.width.max(1) as f32;
        let (Content { mut image, k, origin, source_size }, mut kept) = self.content(clip, canvas, target, t)?;
        // Source px → canvas px, then to target px; the image starts at the
        // crop origin and has k source px per image px.
        let m = layer_matrix(canvas.size(), source_size, &clip.placement, pose);
        let crop_src = crop_rect(source_size, clip.placement.crop);
        let to_source = Affine::translate(crop_src[0], crop_src[1]).then_after(&Affine::scale(k, k)).then_after(&Affine::translate(origin[0], origin[1]));
        let mut matrix = Affine::scale(s, s).then_after(&m).then_after(&to_source);
        let mut proj = space::source_plane(canvas, source_size, clip, pose, camera).map(|(plane, cam)| {
            let plane = plane.then_after(&to_source);
            let h = Homography::from_affine(&Affine::scale(s, s)).then_after(&plane.homography(&cam, canvas.size()));
            let material = clip.space.map(|sp| sp.material).unwrap_or_default();
            let depth = space::plane_depth(&cam, &plane, [0.0, 0.0, image.width as f32, image.height as f32]);
            let proj = Proj { h, plane, material, lit: lit && clip.space.is_some() && material.accepts_lights, depth };
            ProjLayer { proj, camera: cam, scale: s, canvas: canvas.size(), id: clip.id }
        });
        let centre = [image.width as f32 * 0.5, image.height as f32 * 0.5];
        let local_scale = |matrix: &Affine, proj: &Option<ProjLayer>| match proj {
            Some(p) => p.proj.h.jacobian_at(centre).max_scale(),
            None => matrix.max_scale(),
        };
        let mut sigma_target = if pose.blur > 0.0 { pose.blur * 0.01 * target.1 as f32 * 0.5 } else { 0.0 };
        if let Some(p) = &proj
            && clip.space.is_some()
        {
            sigma_target = sigma_target.max(dof_sigma(&p.camera, p.proj.depth, s));
        }
        if sigma_target > 0.0 {
            let sigma_img = sigma_target / local_scale(&matrix, &proj).max(1e-6);
            image = Arc::new(gaussian_blur(&image, sigma_img));
            kept = None;
        }
        // Prefilter when minified so sampling does not alias.
        let mut level = 0;
        while local_scale(&matrix, &proj) < 0.5 && image.width > 2 && image.height > 2 {
            level += 1;
            let (pw, ph) = (image.width as f32, image.height as f32);
            image = self.halved(&image, kept, level);
            // An odd side loses its last pixel when halved: stretched by
            // the exact ratio, the picture still reaches its edge (a
            // backdrop drawn small would leave a line of canvas).
            let two = Affine::scale(pw / image.width as f32, ph / image.height as f32);
            matrix = matrix.then_after(&two);
            if let Some(p) = &mut proj {
                p.proj.h = p.proj.h.then_after_affine(&two);
                p.proj.plane = p.proj.plane.then_after(&two);
            }
        }
        let visible = visible_rect([image.width as f32, image.height as f32], [0.0; 4], pose.clip);
        let opacity = (clip.placement.opacity * pose.opacity).clamp(0.0, 1.0);
        Some(LayerPicture { image, matrix, visible, opacity, proj })
    }

    /// Draws a placed picture onto the canvas with a blend mode.
    pub fn draw(canvas: &mut Frame, layer: &LayerPicture, blend: crate::blend::BlendMode) {
        Self::draw_lit(canvas, layer, blend, &Lighting { lights: &[], casters: Vec::new(), caster_images: Vec::new() });
    }

    /// [`CpuCompositor::draw`] with the lights and shadows of a 3D run.
    pub fn draw_lit(canvas: &mut Frame, layer: &LayerPicture, blend: crate::blend::BlendMode, env: &Lighting) {
        if let Some(pl) = &layer.proj {
            for (d, side) in pl.slices(layer.visible) {
                let (h, plane) = pl.slice(d);
                Self::draw_proj(canvas, layer, pl, &h, &plane, side, blend, env);
            }
            return;
        }
        let Some(inv) = layer.matrix.inverse() else { return };
        if blend.is_matte() {
            // A matte works on the whole canvas: outside it, nothing covers.
            let w = canvas.width as usize;
            let (vis, opacity, img) = (layer.visible, layer.opacity.clamp(0.0, 1.0), &layer.image);
            studio_media::par::par_rows(&mut canvas.pixels, w, 8, |y, row| {
                let yc = y as f32 + 0.5;
                for (x, px) in row.iter_mut().enumerate() {
                    let p = inv.apply([x as f32 + 0.5, yc]);
                    let inside = p[0] >= vis[0] && p[0] <= vis[2] && p[1] >= vis[1] && p[1] <= vis[3];
                    let src = if inside { scale_px(img.sample(p[0], p[1]), opacity) } else { [0.0; 4] };
                    *px = composite(blend, *px, src);
                }
            });
            return;
        }
        if layer.opacity <= 0.0 {
            return;
        }
        let quad = layer_quad(&layer.matrix, layer.visible);
        let b = quad_bounds(&quad);
        let x0 = b[0].floor().max(0.0) as usize;
        let y0 = b[1].floor().max(0.0) as usize;
        let x1 = (b[2].ceil().max(0.0) as usize).min(canvas.width as usize);
        let y1 = (b[3].ceil().max(0.0) as usize).min(canvas.height as usize);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let w = canvas.width as usize;
        let vis = layer.visible;
        let opacity = layer.opacity;
        let img = &layer.image;
        let rows = &mut canvas.pixels[y0 * w..y1 * w];
        studio_media::par::par_rows(rows, w, 8, |ry, row| {
            let y = (y0 + ry) as f32 + 0.5;
            for x in x0..x1 {
                let p = inv.apply([x as f32 + 0.5, y]);
                if p[0] < vis[0] || p[0] > vis[2] || p[1] < vis[1] || p[1] > vis[3] {
                    continue;
                }
                let src = scale_px(img.sample(p[0], p[1]), opacity);
                if src[3] <= 0.0 {
                    continue;
                }
                row[x] = composite(blend, row[x], src);
            }
        });
    }

    /// One slice of a perspective layer: each output pixel is mapped back
    /// onto the picture (nothing behind the lens), lit and blended.
    #[allow(clippy::too_many_arguments)]
    fn draw_proj(canvas: &mut Frame, layer: &LayerPicture, pl: &ProjLayer, h: &Homography, plane: &space::Plane, side: f32, blend: crate::blend::BlendMode, env: &Lighting) {
        let Some(inv) = h.inverse() else { return };
        let (vis, opacity, img) = (layer.visible, layer.opacity.clamp(0.0, 1.0), &layer.image);
        let matte = blend.is_matte();
        if !matte && opacity <= 0.0 {
            return;
        }
        let w = canvas.width as usize;
        let (x0, y0, x1, y1) = if matte {
            (0, 0, w, canvas.height as usize)
        } else {
            let corners = [[vis[0], vis[1]], [vis[2], vis[1]], [vis[2], vis[3]], [vis[0], vis[3]]];
            if corners.iter().any(|c| h.apply_w(*c)[2] < NEAR) {
                (0, 0, w, canvas.height as usize)
            } else {
                let b = quad_bounds(&proj_quad(h, vis, NEAR));
                let x0 = b[0].floor().max(0.0) as usize;
                let y0 = b[1].floor().max(0.0) as usize;
                (x0, y0, (b[2].ceil().max(0.0) as usize).min(w), (b[3].ceil().max(0.0) as usize).min(canvas.height as usize))
            }
        };
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let normal = plane.normal();
        let rows = &mut canvas.pixels[y0 * w..y1 * w];
        studio_media::par::par_rows(rows, w, 8, |ry, row| {
            let y = (y0 + ry) as f32 + 0.5;
            for x in x0..x1 {
                let q = inv.apply_w([x as f32 + 0.5, y]);
                let uv = if q[2].abs() > 1e-20 { [q[0] / q[2], q[1] / q[2]] } else { [f32::NAN; 2] };
                let on = uv[0] >= vis[0] && uv[0] <= vis[2] && uv[1] >= vis[1] && uv[1] <= vis[3] && h.apply_w(uv)[2] >= NEAR;
                if !on {
                    if matte {
                        row[x] = composite(blend, row[x], [0.0; 4]);
                    }
                    continue;
                }
                let src = scale_px(img.sample(uv[0], uv[1]), opacity);
                if src[3] <= 0.0 && !matte {
                    continue;
                }
                let src = if matte { src } else { light_pixel(src, uv, plane, normal, pl, side, env) };
                row[x] = composite(blend, row[x], src);
            }
        });
    }

    fn draw_layer(&mut self, canvas_px: &mut Frame, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc) {
        if let Some(layer) = self.layer_picture(clip, pose, canvas, target, t, camera, false) {
            if layer.proj.is_none()
                && let Some(sh) = &clip.shadow
            {
                for pass in shadow_passes(&mut self.sources, clip, pose, canvas, target, camera, &layer.matrix, layer.visible, layer.opacity, sh) {
                    let shape = gaussian_blur(&shadow::silhouette(&pass, &layer.image, layer.visible, sh.colour), pass.sigma);
                    let [x0, y0, w, h] = pass.region;
                    let patch = LayerPicture { image: Arc::new(shape), matrix: Affine::translate(x0 as f32, y0 as f32), visible: [0.0, 0.0, w as f32, h as f32], opacity: 1.0, proj: None };
                    Self::draw(canvas_px, &patch, crate::blend::BlendMode::Normal);
                }
            }
            Self::draw(canvas_px, &layer, clip.blend);
        }
    }

    /// Draws a run of 3D layers: all pictures first (shadows need every
    /// caster), then far to near with lights and shadows.
    fn draw_run(&mut self, out: &mut Frame, run: &[(&ClipDesc, &Pose)], canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc, lights: &[LightDesc]) {
        let lit = !lights.is_empty();
        let mut pics: Vec<(LayerPicture, crate::blend::BlendMode)> = run.iter().filter_map(|(c, p)| self.layer_picture(c, p, canvas, target, t, camera, lit).map(|l| (l, c.blend))).collect();
        pics.sort_by(|a, b| {
            let d = |l: &LayerPicture| l.proj.map_or(f32::NEG_INFINITY, |p| p.proj.depth);
            d(&b.0).partial_cmp(&d(&a.0)).unwrap_or(std::cmp::Ordering::Equal)
        });
        let shadows = lit && lights.iter().any(|l| l.casts_shadows);
        let mut casters = Vec::new();
        let mut caster_images = Vec::new();
        if shadows {
            for (l, _) in &pics {
                if let Some(pl) = &l.proj
                    && pl.proj.material.casts_shadows
                    && casters.len() < space::MAX_CASTERS
                    && let Some(c) = Caster::new(&pl.proj.plane, l.visible, pl.id)
                {
                    casters.push(c);
                    caster_images.push(&*l.image);
                }
            }
        }
        let env = Lighting { lights, casters, caster_images };
        for (l, blend) in &pics {
            Self::draw_lit(out, l, *blend, &env);
        }
    }

    fn apply_adjustment(&mut self, canvas_px: &mut Frame, clip: &ClipDesc, canvas: &Canvas, t: Micros) {
        let rel = (t - clip.start).max(0);
        let ctx = EffectCtx { time: rel as f32 / SECOND as f32, frame_rate: canvas.frame_rate as f32, seed: clip.seed };
        let mut processed = canvas_px.clone();
        if !clip.effects.is_empty() {
            processed = effects::apply_effects(processed, &clip.effects, ctx);
        }
        if let Some(g) = &clip.grade {
            processed = effects::apply_grade(processed, g, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        }
        processed = crate::nodes::apply_nodes(processed, &clip.grade_nodes, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        let opacity = clip.placement.opacity.clamp(0.0, 1.0);
        let masks: Vec<PreparedMask> = clip.masks.iter().map(|m| PreparedMask::new(m, canvas_px.width, canvas_px.height)).collect();
        let init = mask::initial(&clip.masks);
        let w = canvas_px.width as usize;
        studio_media::par::par_rows(&mut canvas_px.pixels, w, 8, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let c = if masks.is_empty() { 1.0 } else { mask::coverage_at(&masks, init, x as f32 + 0.5, y as f32 + 0.5) } * opacity;
                if c <= 0.0 {
                    continue;
                }
                let q = processed.pixels[y * w + x];
                for i in 0..4 {
                    px[i] += (q[i] - px[i]) * c;
                }
            }
        });
    }

    /// Renders one node alone on a transparent canvas (for transitions).
    fn render_alone(&mut self, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc) -> Frame {
        let mut f = Frame::new(target.0, target.1);
        self.draw_layer(&mut f, clip, pose, canvas, target, t, camera);
        f
    }

    /// Draws a resolved list at a target size.
    pub fn render_list(&mut self, list: &RenderList, target: (u32, u32)) -> Frame {
        let canvas = &list.canvas;
        let t = list.time;
        let camera = list.camera();
        let mut out = Frame::filled(target.0, target.1, canvas.background);
        let mut i = 0;
        while i < list.nodes.len() {
            if in_space(&list.nodes[i]) {
                let mut run = Vec::new();
                while i < list.nodes.len() && in_space(&list.nodes[i]) {
                    if let Node::Layer { clip, pose } = &list.nodes[i] {
                        run.push((clip, pose));
                    }
                    i += 1;
                }
                self.draw_run(&mut out, &run, canvas, target, t, &camera, &list.lights);
                continue;
            }
            match &list.nodes[i] {
                Node::Layer { clip, pose } => self.draw_layer(&mut out, clip, pose, canvas, target, t, &camera),
                Node::Adjustment { clip } => self.apply_adjustment(&mut out, clip, canvas, t),
                Node::Transition { desc, left_pose, right_pose } => {
                    let a = self.render_alone(&desc.left, left_pose, canvas, target, t, &camera);
                    let b = self.render_alone(&desc.right, right_pose, canvas, target, t, &camera);
                    let mixed = effects::transition(&desc.kind, a, b, desc.progress, &desc.params, desc.seed);
                    let layer = LayerPicture { image: Arc::new(mixed), matrix: Affine::IDENTITY, visible: [0.0, 0.0, target.0 as f32, target.1 as f32], opacity: 1.0, proj: None };
                    Self::draw(&mut out, &layer, crate::blend::BlendMode::Normal);
                }
            }
            i += 1;
        }
        out
    }

    /// Renders a scene at a time and size.
    pub fn render_scene(&mut self, scene: &dyn Scene, t: Micros, target: (u32, u32)) -> Frame {
        if self.depth == 0 {
            self.cache.begin_frame();
        }
        let list = resolve(scene, t);
        self.render_list(&list, target)
    }
}

/// Applies a view transform to a finished frame (CPU).
pub fn apply_view(frame: Frame, view: crate::view::ViewTransform, space: crate::output::OutputSpace) -> Frame {
    match crate::view::view_grade(view, space) {
        Some(g) => {
            let mut img = frame.into_colour();
            g.apply(&mut img, studio_colour::FrameInfo { frame: 0 });
            Frame::from_colour(img)
        }
        None => frame,
    }
}

impl FrameRenderer for CpuCompositor {
    fn render_encoded(&mut self, scene: &dyn Scene, t: Micros, target: Option<(u32, u32)>, settings: &OutputSettings, depth: OutputDepth, frame_index: u32) -> EncodedFrame {
        let canvas = scene.canvas();
        let size = output_size(&canvas, target, settings);
        let frame = apply_view(self.render_scene(scene, t, size), canvas.view, settings.space);
        encode(&frame, settings, depth, frame_index)
    }

    fn render(&mut self, scene: &dyn Scene, t: Micros, target: Option<(u32, u32)>) -> Frame {
        let canvas = scene.canvas();
        let size = target_size(&canvas, target);
        apply_view(self.render_scene(scene, t, size), canvas.view, canvas.output)
    }

    fn sources(&mut self) -> &mut Sources {
        &mut self.sources
    }

    fn name(&self) -> &'static str {
        "cpu"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{GeneratorDesc, GeneratorKind, Param, Params, StaticScene};
    use crate::source::{DecodeCache, SourceRegistry};

    fn compositor() -> CpuCompositor {
        CpuCompositor::new(Sources::new(SourceRegistry::new(), DecodeCache::new(1 << 24)))
    }

    fn solid(id: u64, order: i32, rgba: [f64; 4]) -> ClipDesc {
        let mut params = Params::new();
        params.insert("colour".into(), Param::Colour(rgba));
        ClipDesc::new(id, order, 0, SECOND, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params, text: vec![] }))
    }

    #[test]
    fn target_size_keeps_aspect_and_caps() {
        let c = Canvas::new(1920, 1080, 30.0);
        assert_eq!(target_size(&c, None), (1920, 1080));
        assert_eq!(target_size(&c, Some((960, 1000))), (960, 540));
        assert_eq!(target_size(&c, Some((4000, 4000))), (1920, 1080));
        assert_eq!(target_size(&c, Some((100, 30))), (53, 30));
        // Export may draw above the canvas; the preview never does.
        let up = OutputSettings { allow_upscale: true, ..Default::default() };
        assert_eq!(output_size(&c, Some((3840, 2160)), &up), (3840, 2160));
        assert_eq!(output_size(&c, Some((3840, 2160)), &OutputSettings::default()), (1920, 1080));
        assert_eq!(output_size(&c, Some((960, 540)), &up), (960, 540));
    }

    #[test]
    fn solid_covers_canvas_and_half_scale_quarter() {
        let scene = StaticScene::new(Canvas::new(64, 32, 30.0), SECOND).with_clip(solid(1, 0, [1.0, 0.0, 0.0, 1.0]));
        let mut c = compositor();
        let f = c.render(&scene, 0, None);
        assert_eq!(f.get(0, 0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(f.get(63, 31), [1.0, 0.0, 0.0, 1.0]);
        let mut half = solid(2, 1, [0.0, 1.0, 0.0, 1.0]);
        half.placement.scale = [0.5, 0.5];
        let scene = scene.with_clip(half);
        let f = c.render(&scene, 0, Some((32, 16)));
        assert_eq!((f.width, f.height), (32, 16));
        assert_eq!(f.get(16, 8), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(f.get(1, 1), [1.0, 0.0, 0.0, 1.0]);
    }

    /// A picture that fills the frame, drawn small: halved four times, its
    /// odd sides lose a pixel at every level, and it must still reach the
    /// right and bottom edges (not leave a line of canvas there).
    #[test]
    fn a_picture_drawn_small_reaches_its_edges() {
        let (w, h) = (1023u32, 575u32);
        let registry = SourceRegistry::new();
        let still = crate::source::ProceduralSource::new(w, h, 30.0, 1, move |_| crate::yuv::DecodedPicture::from_rgba8(w, h, vec![255; (w * h * 4) as usize]));
        registry.register(crate::scene::MediaKey(7), crate::source::SourceSpec::Procedural(still));
        let mut c = CpuCompositor::new(Sources::new(registry, DecodeCache::new(1 << 26)));
        let mut canvas = Canvas::new(w, h, 30.0);
        canvas.background = [0.0; 4];
        let scene = StaticScene::new(canvas, SECOND).with_clip(ClipDesc::new(1, 0, 0, SECOND, SourceDesc::Media { media: crate::scene::MediaKey(7), time: 0 }));
        let f = c.render(&scene, 0, Some((48, 27)));
        let (fw, fh) = (f.width, f.height);
        for (x, y) in [(fw - 1, fh / 2), (fw / 2, fh - 1), (fw - 1, fh - 1), (0, 0)] {
            assert!(f.get(x, y)[3] > 0.95, "({x}, {y}) of {fw}x{fh}: {:?}", f.get(x, y));
        }
    }

    #[test]
    fn opacity_and_background() {
        let mut canvas = Canvas::new(8, 8, 30.0);
        canvas.background = [0.0, 0.0, 1.0, 1.0];
        let mut clip = solid(1, 0, [1.0, 1.0, 1.0, 1.0]);
        clip.placement.opacity = 0.25;
        let scene = StaticScene::new(canvas, SECOND).with_clip(clip);
        let f = compositor().render(&scene, 0, None);
        let p = f.get(4, 4);
        assert!((p[0] - 0.25).abs() < 1e-6 && (p[2] - 1.0).abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn nested_scene_renders() {
        let inner = StaticScene::new(Canvas::new(16, 16, 30.0), SECOND).with_clip(solid(1, 0, [0.0, 1.0, 0.0, 1.0]));
        let outer = StaticScene::new(Canvas::new(32, 16, 30.0), SECOND)
            .with_clip(ClipDesc::new(5, 0, 0, SECOND, SourceDesc::Nested { scene: std::sync::Arc::new(inner), time: 0 }));
        let f = compositor().render(&outer, 0, None);
        // The square inner canvas is fitted and centred: green in the middle,
        // background at the sides.
        assert_eq!(f.get(16, 8), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(f.get(2, 8), [0.0, 0.0, 0.0, 1.0]);
    }
}
