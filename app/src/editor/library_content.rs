// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Assets and Audio panels backed by the content library: stickers, emoji,
//! shapes, frames, overlays, backgrounds, photos, green-screen and motion
//! clips, sounds and music. Items become media in the project when added.

use super::library::{LibDrag, RailItem};
use crate::app::StudioApp;
use crate::content::{self, Entry};
use crate::icons::Icon;
use crate::kit::{self, TileGrid, TileInfo};
use crate::theme;
use crate::thumbs::{Key, Pic};
use crate::view::{self, ClipKind, LaneKind, MediaKind, SEC, Time};
use eframe::egui::{self, Align2, Color32, Painter, Rect, Vec2};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use studio_content::item::Kind;
use crate::kit::ShowNoted;

pub const ASSET_GROUPS: &[&str] = &["All", "Stickers", "Emoji", "Shapes", "Text art", "Frames", "Overlays", "Backgrounds", "Photos", "Green screen", "Motion"];
pub const AUDIO_GROUPS: &[&str] = &["All", "Music", "Sound effects"];

fn kinds(rail: RailItem, chip: usize) -> &'static [Kind] {
    match (rail, chip) {
        (RailItem::Audio, 1) => &[Kind::Music],
        (RailItem::Audio, 2) => &[Kind::Sound],
        (RailItem::Audio, _) => &[Kind::Music, Kind::Sound],
        (_, 1) => &[Kind::Sticker],
        (_, 2) => &[Kind::Emoji],
        (_, 3) => &[Kind::Shape],
        (_, 4) => &[Kind::TextArt],
        (_, 5) => &[Kind::Frame],
        (_, 6) => &[Kind::Overlay],
        (_, 7) => &[Kind::Background],
        (_, 8) => &[Kind::Photo],
        (_, 9) => &[Kind::GreenScreen],
        (_, 10) => &[Kind::Motion],
        // "All" leads with the made-for-Studio kinds; the big icon and emoji
        // sets have their own chips.
        _ => &[Kind::Sticker, Kind::TextArt, Kind::Frame, Kind::Overlay, Kind::Background, Kind::Photo, Kind::GreenScreen, Kind::Motion],
    }
}

/// Indices into `Content::entries` for a panel, chip and search (cached:
/// the library has thousands of items).
fn filtered(c: &content::Content, rail: RailItem, chip: usize, query: &str) -> Arc<Vec<u32>> {
    type Cache = Mutex<HashMap<(RailItem, usize, String), Arc<Vec<u32>>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (rail, chip, query.to_owned());
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let ks = kinds(rail, chip);
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let v: Vec<u32> = c
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| ks.contains(&e.kind))
        .filter(|(_, e)| {
            words.iter().all(|w| e.name.to_lowercase().contains(w) || e.category.to_lowercase().contains(w) || e.set.to_lowercase().contains(w) || e.tags.iter().any(|t| t.starts_with(w.as_str())))
        })
        .map(|(i, _)| i as u32)
        .collect();
    let v = Arc::new(v);
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 64 {
        g.clear();
    }
    g.insert(key, v.clone());
    v
}

/// Draw the panel body. Returns false when the library is not installed,
/// so the caller shows its built-in samples instead.
pub fn panel(app: &mut StudioApp, ui: &mut egui::Ui, rail: RailItem, chip: usize, query: &str, body: Rect) -> bool {
    let Some(c) = content::get() else {
        if content::find_dir().is_some() {
            // Still loading: say so in place.
            let t = theme::tokens(ui.ctx());
            let p = ui.painter();
            kit::spinner(p, body.center() - Vec2::new(0.0, 12.0), 8.0, t.text_2);
            p.text(body.center() + Vec2::new(0.0, 12.0), Align2::CENTER_CENTER, crate::i18n::shown("Loading the library…"), theme::body(), t.text_2);
            return true;
        }
        return false;
    };
    let t = theme::tokens(ui.ctx());
    let items = filtered(&c, rail, chip, query);
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    if items.is_empty() {
        let mut eui = bui.new_child(egui::UiBuilder::new().max_rect(body.shrink(theme::space::PANEL_PAD)));
        if kit::empty_state(&mut eui, Icon::MagnifyingGlass, "Nothing matches that search.", Some(("Clear search", Icon::X))) {
            app.editor.library_query.clear();
        }
        return true;
    }
    let audio = rail == RailItem::Audio;
    let pad = theme::space::PANEL_PAD;
    let mut added = None;
    let mut hovered_audio = None;
    egui::ScrollArea::vertical().id_salt(("content-scroll", rail as u8, chip)).auto_shrink([false, false]).show_v(&mut bui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: pad as i8, right: pad as i8, top: 2, bottom: 8 }).show(ui, |ui| {
            let entries = &c.entries;
            let badges: HashMap<u32, String> = HashMap::new();
            let _ = badges;
            let info = |k: usize| {
                let e = &entries[items[k] as usize];
                TileInfo { label: e.name.as_str(), badge: None, sub: None, selected: false, addable: true, progress: None, tip: Some(e.set.as_str()), added: false }
            };
            let thumbs = &mut app.thumbs;
            let mut paint = |p: &Painter, r: Rect, k: usize, _hover: f32, live: bool| {
                let index = items[k];
                let e = &entries[index as usize];
                let px = (((r.width().max(r.height()) * p.ctx().pixels_per_point()) / 64.0).ceil() * 64.0).clamp(64.0, 256.0) as u16;
                let px = if px <= 128 { 128 } else { 256 };
                if audio {
                    p.rect_filled(r, 0, theme::mix(t.sunken, t.audio, if t.dark { 0.45 } else { 0.6 }));
                } else if e.kind.is_cutout() || matches!(e.kind, Kind::Shape | Kind::Emoji | Kind::TextArt | Kind::GreenScreen) {
                    // Cut-outs sit on a quiet card so their edges read.
                    p.rect_filled(r, 0, if t.dark { theme::mix(t.sunken, t.raised, 0.6) } else { theme::mix(t.sunken, Color32::WHITE, 0.5) });
                }
                if let Some((tex, size)) = thumbs.get_sized(Key { pic: Pic::Content { index }, w: px, h: px }) {
                    let inset = if audio { Vec2::new(6.0, 10.0) } else if matches!(e.kind, Kind::Photo | Kind::Background | Kind::Overlay | Kind::Motion) { Vec2::ZERO } else { Vec2::splat(r.width() * 0.12) };
                    let area = r.shrink2(inset);
                    let fit = if matches!(e.kind, Kind::Photo | Kind::Background | Kind::Overlay) {
                        // Photos fill the tile.
                        (area.width() / size.x).max(area.height() / size.y)
                    } else {
                        (area.width() / size.x).min(area.height() / size.y)
                    };
                    let dst = Rect::from_center_size(area.center(), size * fit);
                    let tint = if audio { Color32::from_white_alpha(220) } else { Color32::WHITE };
                    p.image(tex, dst, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
                }
                if audio {
                    if let Some(d) = e.duration {
                        let label = view::format_short((d * SEC as f64) as Time);
                        p.text(r.right_bottom() - Vec2::new(6.0, 5.0), Align2::RIGHT_BOTTOM, label, theme::caption(), Color32::WHITE);
                    }
                    if live {
                        // Playing: a sweep shows where the preview is.
                        let k = ((kit::clock(p.ctx()) % 3.0) / 3.0) as f32;
                        let x = egui::lerp(r.left()..=r.right(), k);
                        p.rect_filled(Rect::from_min_max(r.min, egui::pos2(x, r.bottom())), 0, Color32::from_white_alpha(24));
                        p.vline(x, r.y_range(), egui::Stroke::new(1.5, t.accent));
                    }
                }
            };
            let grid = TileGrid::new(if audio { "content-audio" } else { "content-assets" }).aspect(if audio { 2.4 } else { 1.0 }).min_width(if audio { 140.0 } else { 84.0 }).labels(audio || chip > 4);
            let out = grid.show(ui, items.len(), info, |k| LibDrag { rail, index: items[k] as usize }, &mut paint);
            if let Some(k) = out.clicked.or(out.added) {
                added = Some(items[k] as usize);
            }
            if audio {
                hovered_audio = out.hovered.map(|k| entries[items[k] as usize].path.clone());
            }
        });
    });
    match hovered_audio {
        Some(p) => crate::audio_preview::play(&p),
        None => crate::audio_preview::stop(),
    }
    if let Some(i) = added {
        let ctx = ui.ctx().clone();
        add_content(app, &ctx, i, None, None);
    }
    true
}

/// The media kind and file a content item becomes in a project. Vector
/// shapes and green-screen stills are turned into cut-out PNGs first.
fn materialise(e: &Entry) -> Option<(MediaKind, std::path::PathBuf, (u32, u32))> {
    let c = content::get()?;
    match e.kind {
        Kind::Sound | Kind::Music => Some((MediaKind::Audio, e.path.clone(), (0, 0))),
        Kind::Motion => Some((MediaKind::Video, e.path.clone(), e.size.unwrap_or((1920, 1080)))),
        Kind::Shape | Kind::GreenScreen | Kind::Emoji => {
            let t = c.thumbnailer.as_ref()?;
            let img = t.thumbnail(&e.path, e.kind, 1024).ok()?;
            let path = t.cache_path(&e.path, e.kind, 1024).ok()?;
            Some((MediaKind::Image, path, (img.width, img.height)))
        }
        _ => Some((MediaKind::Image, e.path.clone(), e.size.unwrap_or((1024, 1024)))),
    }
}

/// Add content item `index` at `at` (or the playhead), on `lane` (or the
/// natural lane for its kind).
pub fn add_content(app: &mut StudioApp, ctx: &egui::Context, index: usize, at: Option<Time>, lane: Option<u32>) {
    let Some(c) = content::get() else { return };
    let Some(e) = c.entries.get(index).cloned() else { return };
    let Some((kind, path, (w, h))) = materialise(&e) else {
        kit::push(ctx, crate::i18n::tf(format!("Could not add {}", e.name)), None, Some(Icon::Warning));
        return;
    };
    let t = at.unwrap_or(app.editor.playhead);
    let dur = match kind {
        MediaKind::Audio | MediaKind::Video => ((e.duration.unwrap_or(3.0)) * SEC as f64) as Time,
        MediaKind::Image => 4 * SEC,
    };
    let media_id = app.doc.alloc_id() as u32;
    let clip_id = app.doc.alloc_id() as u32;
    let p = &app.doc.project;
    let lane = lane.or_else(|| {
        if kind == MediaKind::Audio {
            p.lanes.iter().filter(|l| l.kind == LaneKind::Audio).last().map(|l| l.id)
        } else {
            p.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.id)
        }
    });
    let Some(lane) = lane else { return };
    let mi = view::MediaItem { id: media_id, name: e.name.clone(), kind, dur, width: w, height: h, fps: 30.0, seed: media_id, path: Some(path) };
    let mut clip = super::library::blank_clip(lane, t, dur, if kind == MediaKind::Audio { ClipKind::Audio } else { ClipKind::Video }, &e.name, Some(media_id));
    clip.id = clip_id;
    // Cut-outs arrive at a size that leaves room around them.
    if matches!(e.kind, Kind::Sticker | Kind::Emoji | Kind::Shape | Kind::TextArt | Kind::GreenScreen) {
        clip.transform.scale = 40.0;
    }
    let name = e.name.clone();
    if app.doc.edit(&crate::i18n::tf(format!("Add {name}")), |p| {
        p.media.push(mi);
        p.clips.push(clip);
        p.resolve_overlap(clip_id);
    }) {
        app.editor.selection = vec![clip_id];
        app.editor.undo_toast = Some(kit::push(ctx, crate::i18n::tf(format!("Added {name}")), Some("Undo"), Some(Icon::Plus)));
    }
}

/// Clip kind a content drag becomes (for the timeline's drop ghost).
pub fn drag_kind(index: usize) -> ClipKind {
    match content::get().and_then(|c| c.entries.get(index).map(|e| e.kind)) {
        Some(Kind::Sound | Kind::Music) => ClipKind::Audio,
        _ => ClipKind::Video,
    }
}

pub fn drag_dur(index: usize) -> Time {
    let e = content::get().and_then(|c| c.entries.get(index).cloned());
    match e {
        Some(e) if matches!(e.kind, Kind::Sound | Kind::Music | Kind::Motion) => (e.duration.unwrap_or(3.0) * SEC as f64) as Time,
        _ => 4 * SEC,
    }
}

