// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text drawing hook. The `text` crate (shaping, layout, styles) is being
//! written separately; until it lands the renderer draws text through this
//! trait, with a stub that draws nothing.
//!
//! The `text` crate implements [`TextRenderer`] and the application
//! installs it with [`set_text_renderer`]; the compositor then rasterises
//! text generators at the final pixel size (so glyphs stay crisp at any
//! zoom) and places the result like any other layer.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::frame::Frame;
use crate::scene::{Params, TextRun};

/// Rasterises styled text into a premultiplied linear frame.
pub trait TextRenderer: Send + Sync {
    /// Draws `runs` with the generator's `style` into a `width × height`
    /// picture (the layer's pixel size at the canvas scale). Returns `None`
    /// when there is nothing to draw.
    fn raster(&self, runs: &[TextRun], style: &Params, width: u32, height: u32) -> Option<Frame>;
}

/// Draws nothing. Stands in until the `text` crate is available.
pub struct NoText;

impl TextRenderer for NoText {
    fn raster(&self, _runs: &[TextRun], _style: &Params, _width: u32, _height: u32) -> Option<Frame> {
        None
    }
}

static RENDERER: RwLock<Option<Arc<dyn TextRenderer>>> = RwLock::new(None);

/// Bumped whenever a renderer is installed, so text pictures kept from
/// an earlier one are not reused.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Installs the text renderer used by every compositor from now on.
pub fn set_text_renderer(r: Arc<dyn TextRenderer>) {
    *RENDERER.write().unwrap() = Some(r);
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The installed text renderer, or the stub.
pub fn text_renderer() -> Arc<dyn TextRenderer> {
    RENDERER.read().unwrap().clone().unwrap_or_else(|| Arc::new(NoText))
}

/// Changes whenever [`set_text_renderer`] installs a renderer.
pub fn text_generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}
