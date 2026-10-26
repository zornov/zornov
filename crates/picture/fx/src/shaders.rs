// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! WGSL compute shaders for the per-pixel effects and transitions. Each
//! constant is a complete module: the shared prelude followed by the
//! effect body. Binding layout is described in `wgsl/common.wgsl`.

/// What Studio puts in front of every effect shader body (bindings,
/// uniforms and helpers); plugins supply only `fn effect`.
pub const EFFECT_PRELUDE: &str = include_str!("wgsl/common.wgsl");

/// What Studio puts in front of every transition shader body; plugins
/// supply only `fn transition`.
pub const TRANSITION_PRELUDE: &str = include_str!("wgsl/transition_common.wgsl");

macro_rules! effect_shader {
    ($($name:ident = $file:literal;)*) => {
        $(
            pub const $name: &str = concat!(
                include_str!("wgsl/common.wgsl"),
                "\n",
                include_str!(concat!("wgsl/", $file)),
            );
        )*
    };
}

effect_shader! {
    DIRECTIONAL_BLUR = "directional_blur.wgsl";
    SPIN_BLUR = "spin_blur.wgsl";
    ZOOM_BLUR = "zoom_blur.wgsl";
    POSTERISE = "posterise.wgsl";
    HALFTONE = "halftone.wgsl";
    EMBOSS = "emboss.wgsl";
    PIXELATE = "pixelate.wgsl";
    KALEIDOSCOPE = "kaleidoscope.wgsl";
    MIRROR = "mirror.wgsl";
    EDGE_DETECT = "edge_detect.wgsl";
    WAVE = "wave.wgsl";
    RIPPLE = "ripple.wgsl";
    TWIRL = "twirl.wgsl";
    BULGE = "bulge.wgsl";
    FISHEYE = "fisheye.wgsl";
    CHROMATIC_ABERRATION = "chromatic_aberration.wgsl";
    RGB_SPLIT = "rgb_split.wgsl";
    CHROMA_KEY = "chroma_key.wgsl";
    LUMA_KEY = "luma_key.wgsl";
    SOLID = "solid.wgsl";
    GRADIENT = "gradient.wgsl";
    GRID = "grid.wgsl";
    CHECKERBOARD = "checkerboard.wgsl";
    FILM = "film.wgsl";
    CAMERA_SHAKE = "camera_shake.wgsl";
    PARTICLES = "particles.wgsl";
    WATER = "water.wgsl";
}

macro_rules! transition_shader {
    ($($name:ident = $($file:literal)+;)*) => {
        $(
            pub const $name: &str = concat!(
                include_str!("wgsl/transition_common.wgsl"),
                $("\n", include_str!(concat!("wgsl/", $file)),)+
            );
        )*
    };
}

transition_shader! {
    T_DISSOLVE = "t_dissolve.wgsl";
    T_DIP = "t_dip.wgsl";
    T_WIPE = "t_wipe.wgsl";
    T_CLOCK_WIPE = "t_clock_wipe.wgsl";
    T_SLIDE = "t_slide_layers.wgsl" "t_slide.wgsl";
    T_PUSH = "t_slide_layers.wgsl" "t_push.wgsl";
    T_COVER = "t_slide_layers.wgsl" "t_cover.wgsl";
    T_REVEAL = "t_slide_layers.wgsl" "t_reveal.wgsl";
    T_LUMA_FADE = "t_luma_fade.wgsl";
}
