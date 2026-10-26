// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Phosphor icons (MIT, see `assets/icons/Phosphor-LICENSE.txt`), bundled as
//! SVG and rasterised to textures at the exact pixel size they are drawn at,
//! so 16 px and 20 px icons stay crisp at any display scale.

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

macro_rules! icons {
    ($($variant:ident => $file:literal / $fill:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($variant,)* }

        impl Icon {
            pub const ALL: &'static [Icon] = &[$(Icon::$variant,)*];
            fn svg(self, fill: bool) -> &'static str {
                match (self, fill) {
                    $((Icon::$variant, false) => include_str!(concat!("../assets/icons/", $file, ".svg")),
                      (Icon::$variant, true) => include_str!(concat!("../assets/icons/", $fill, ".svg")),)*
                }
            }
            pub fn name(self) -> &'static str {
                match self { $(Icon::$variant => $file,)* }
            }
        }
    };
}

icons! {
    Aperture => "aperture" / "aperture-fill",
    ArrowCounterClockwise => "arrow-counter-clockwise" / "arrow-counter-clockwise-fill",
    ArrowSquareOut => "arrow-square-out" / "arrow-square-out-fill",
    ArrowUUpLeft => "arrow-u-up-left" / "arrow-u-up-left-fill",
    ArrowUUpRight => "arrow-u-up-right" / "arrow-u-up-right-fill",
    ArrowsHorizontal => "arrows-horizontal" / "arrows-horizontal-fill",
    ArrowsOutCardinal => "arrows-out-cardinal" / "arrows-out-cardinal-fill",
    ArrowsOutLineHorizontal => "arrows-out-line-horizontal" / "arrows-out-line-horizontal-fill",
    Bell => "bell" / "bell-fill",
    BracketsSquare => "brackets-square" / "brackets-square-fill",
    CaretDown => "caret-down" / "caret-down-fill",
    CaretLeft => "caret-left" / "caret-left-fill",
    CaretRight => "caret-right" / "caret-right-fill",
    CaretUp => "caret-up" / "caret-up-fill",
    Check => "check" / "check-fill",
    CheckCircle => "check-circle" / "check-circle-fill",
    Circle => "circle" / "circle-fill",
    CirclesThree => "circles-three" / "circles-three-fill",
    Clock => "clock" / "clock-fill",
    Copy => "copy" / "copy-fill",
    CornersIn => "corners-in" / "corners-in-fill",
    CornersOut => "corners-out" / "corners-out-fill",
    Cpu => "cpu" / "cpu-fill",
    Crop => "crop" / "crop-fill",
    Cursor => "cursor" / "cursor-fill",
    DeviceMobile => "device-mobile" / "device-mobile-fill",
    Diamond => "diamond" / "diamond-fill",
    DotsSixVertical => "dots-six-vertical" / "dots-six-vertical-fill",
    DotsThree => "dots-three" / "dots-three-fill",
    DotsThreeVertical => "dots-three-vertical" / "dots-three-vertical-fill",
    DownloadSimple => "download-simple" / "download-simple-fill",
    DropHalf => "drop-half" / "drop-half-fill",
    Export => "export" / "export-fill",
    Eye => "eye" / "eye-fill",
    EyeSlash => "eye-slash" / "eye-slash-fill",
    Eyedropper => "eyedropper" / "eyedropper-fill",
    Faders => "faders" / "faders-fill",
    FileVideo => "file-video" / "file-video-fill",
    FilmSlate => "film-slate" / "film-slate-fill",
    FilmStrip => "film-strip" / "film-strip-fill",
    Flag => "flag" / "flag-fill",
    Folder => "folder" / "folder-fill",
    FolderOpen => "folder-open" / "folder-open-fill",
    FrameCorners => "frame-corners" / "frame-corners-fill",
    Gauge => "gauge" / "gauge-fill",
    GearSix => "gear-six" / "gear-six-fill",
    GridFour => "grid-four" / "grid-four-fill",
    HardDrives => "hard-drives" / "hard-drives-fill",
    Headphones => "headphones" / "headphones-fill",
    House => "house" / "house-fill",
    Image => "image" / "image-fill",
    Info => "info" / "info-fill",
    Intersect => "intersect" / "intersect-fill",
    Keyboard => "keyboard" / "keyboard-fill",
    LinkSimple => "link-simple" / "link-simple-fill",
    List => "list" / "list-fill",
    LockSimple => "lock-simple" / "lock-simple-fill",
    LockSimpleOpen => "lock-simple-open" / "lock-simple-open-fill",
    MagicWand => "magic-wand" / "magic-wand-fill",
    Magnet => "magnet" / "magnet-fill",
    MagnetStraight => "magnet-straight" / "magnet-straight-fill",
    MagnifyingGlass => "magnifying-glass" / "magnifying-glass-fill",
    MagnifyingGlassMinus => "magnifying-glass-minus" / "magnifying-glass-minus-fill",
    MagnifyingGlassPlus => "magnifying-glass-plus" / "magnifying-glass-plus-fill",
    Microphone => "microphone" / "microphone-fill",
    Minus => "minus" / "minus-fill",
    Monitor => "monitor" / "monitor-fill",
    MonitorPlay => "monitor-play" / "monitor-play-fill",
    Moon => "moon" / "moon-fill",
    MusicNote => "music-note" / "music-note-fill",
    MusicNotes => "music-notes" / "music-notes-fill",
    Palette => "palette" / "palette-fill",
    Path => "path" / "path-fill",
    Pause => "pause" / "pause-fill",
    Play => "play" / "play-fill",
    Plus => "plus" / "plus-fill",
    PlusCircle => "plus-circle" / "plus-circle-fill",
    Question => "question" / "question-fill",
    Rectangle => "rectangle" / "rectangle-fill",
    Repeat => "repeat" / "repeat-fill",
    Scissors => "scissors" / "scissors-fill",
    Selection => "selection" / "selection-fill",
    Shapes => "shapes" / "shapes-fill",
    SidebarSimple => "sidebar-simple" / "sidebar-simple-fill",
    SkipBack => "skip-back" / "skip-back-fill",
    SkipForward => "skip-forward" / "skip-forward-fill",
    Smiley => "smiley" / "smiley-fill",
    Sparkle => "sparkle" / "sparkle-fill",
    SpeakerHigh => "speaker-high" / "speaker-high-fill",
    SpeakerSlash => "speaker-slash" / "speaker-slash-fill",
    SpinnerGap => "spinner-gap" / "spinner-gap-fill",
    Square => "square" / "square-fill",
    SquareSplitHorizontal => "square-split-horizontal" / "square-split-horizontal-fill",
    SquaresFour => "squares-four" / "squares-four-fill",
    Stack => "stack" / "stack-fill",
    Star => "star" / "star-fill",
    Sticker => "sticker" / "sticker-fill",
    Subtitles => "subtitles" / "subtitles-fill",
    Sun => "sun" / "sun-fill",
    TextT => "text-t" / "text-t-fill",
    Timer => "timer" / "timer-fill",
    Trash => "trash" / "trash-fill",
    UploadSimple => "upload-simple" / "upload-simple-fill",
    Warning => "warning" / "warning-fill",
    Waveform => "waveform" / "waveform-fill",
    X => "x" / "x-fill",
    XCircle => "x-circle" / "x-circle-fill",
    DiamondFill => "diamond-fill" / "diamond-fill",
    PlayFill => "play-fill" / "play-fill",
    PauseFill => "pause-fill" / "pause-fill",
    FlagFill => "flag-fill" / "flag-fill",
}

/// Icon size in controls.
pub const SIZE: f32 = 16.0;
/// Icon size in the tool rail.
pub const RAIL: f32 = 20.0;

#[derive(Default)]
struct Cache {
    textures: HashMap<(Icon, u32, bool), TextureHandle>,
}

fn cache(ctx: &egui::Context) -> Arc<Mutex<Cache>> {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(egui::Id::unique("studio-icon-cache"), || {
            Arc::new(Mutex::new(Cache::default()))
        })
        .clone()
    })
}

/// Rasterise an icon at `px` × `px` pixels, white on transparent (tinted when
/// drawn).
pub fn rasterise(icon: Icon, px: u32, fill: bool) -> ColorImage {
    let svg = icon.svg(fill).replace("currentColor", "#FFFFFF");
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
        .expect("bundled icon parses");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px).expect("non-zero icon size");
    let scale = px as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    ColorImage::from_rgba_premultiplied([px as usize, px as usize], pixmap.data())
}

/// Texture for `icon` drawn at `size` points on the current display, in
/// the regular or the fill weight.
pub fn texture(ctx: &egui::Context, icon: Icon, size: f32, fill: bool) -> egui::TextureId {
    let px = (size * ctx.pixels_per_point()).round().max(1.0) as u32;
    let cache = cache(ctx);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .textures
        .entry((icon, px, fill))
        .or_insert_with(|| ctx.load_texture(format!("icon-{}-{px}-{fill}", icon.name()), rasterise(icon, px, fill), TextureOptions::LINEAR))
        .id()
}

/// Warm the cache for the two standard sizes so the first frame does not
/// stutter while icons rasterise.
pub fn preload(ctx: &egui::Context) {
    for &icon in Icon::ALL {
        texture(ctx, icon, SIZE, false);
        texture(ctx, icon, RAIL, false);
    }
}

/// Paint `icon` centred in `rect` at `size` points, tinted `color`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, size: f32, color: Color32) {
    paint_weight(painter, rect, icon, size, color, false);
}

/// Paint in the fill weight (active states).
pub fn paint_fill(painter: &egui::Painter, rect: Rect, icon: Icon, size: f32, color: Color32) {
    paint_weight(painter, rect, icon, size, color, true);
}

fn paint_weight(painter: &egui::Painter, rect: Rect, icon: Icon, size: f32, color: Color32, fill: bool) {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    // Snap to the pixel grid so strokes do not blur.
    let c = rect.center();
    let half = size / 2.0;
    let min = Pos2::new(((c.x - half) * ppp).round() / ppp, ((c.y - half) * ppp).round() / ppp);
    let r = Rect::from_min_size(min, Vec2::splat(size));
    painter.image(
        texture(ctx, icon, size, fill),
        r,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterises() {
        for &icon in Icon::ALL {
            for fill in [false, true] {
                let img = rasterise(icon, 32, fill);
                assert!(img.pixels.iter().any(|p| p.a() > 0), "{} is empty", icon.name());
            }
        }
    }
}
