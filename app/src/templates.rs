// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project templates and guided practice projects.
//!
//! A template is a canvas size and frame rate, the lanes that kind of
//! production needs, and markers laying out its structure, with placeholder
//! titles to type over. Everything people read (marker notes, title text,
//! names) comes from the message catalogue, in the language they use. Lane
//! names are kept in English in the project file (some features look lanes
//! up by name) and translated when they are drawn.
//!
//! The guided projects are the demo project with markers that say what to
//! try, so a new person can practise on something that already works.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::view::{self, ClipKind, Lane, LaneKind, Marker, Project, SEC};
use eframe::egui::Color32;
use studio_i18n::{tr_in, Lang};

/// A lane: its English name, kind and colour.
type L = (&'static str, LaneKind, Color32);

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const TITLES: L = ("Titles", LaneKind::Visual, rgb(0xB4, 0x8C, 0xF0));
const BROLL: L = ("B-roll", LaneKind::Visual, rgb(0x7F, 0xA8, 0xE8));
const MAIN: L = ("Main", LaneKind::Spine, rgb(0x8B, 0x6C, 0xFF));
const VOICE: L = ("Voice", LaneKind::Audio, rgb(0x6C, 0xC6, 0xA0));
const MUSIC: L = ("Music", LaneKind::Audio, rgb(0xE8, 0xB8, 0x6A));

/// A placeholder title: lane index, start and length in seconds, message
/// suffix, text size.
struct T(usize, f64, f64, &'static str, f32);

pub struct Template {
    pub id: &'static str,
    pub icon: Icon,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    lanes: &'static [L],
    /// Seconds and message suffix (`tpl-<id>-<suffix>`).
    markers: &'static [(f64, &'static str)],
    titles: &'static [T],
    /// Colours of the tile.
    pub tint: (Color32, Color32),
}

pub const ALL: &[Template] = &[
    Template {
        id: "youtube",
        icon: Icon::MonitorPlay,
        width: 1920,
        height: 1080,
        fps: 30.0,
        lanes: &[TITLES, BROLL, MAIN, VOICE, MUSIC],
        markers: &[(0.0, "m-hook"), (8.0, "m-intro"), (30.0, "m-ch1"), (90.0, "m-ch2"), (150.0, "m-ch3"), (170.0, "m-end")],
        titles: &[T(0, 0.5, 4.0, "t-title", 84.0), T(0, 172.0, 8.0, "t-thanks", 72.0)],
        tint: (rgb(0xD0, 0x30, 0x30), rgb(0x60, 0x10, 0x20)),
    },
    Template {
        id: "short-film",
        icon: Icon::FilmSlate,
        width: 1920,
        height: 804,
        fps: 24.0,
        lanes: &[TITLES, MAIN, ("Dialogue", LaneKind::Audio, rgb(0x6C, 0xC6, 0xA0)), ("Foley and effects", LaneKind::Audio, rgb(0x6C, 0xA0, 0xC6)), ("Score", LaneKind::Audio, rgb(0xE8, 0xB8, 0x6A))],
        markers: &[(0.0, "m-open"), (60.0, "m-setup"), (180.0, "m-inciting"), (420.0, "m-midpoint"), (720.0, "m-climax"), (840.0, "m-resolution"), (870.0, "m-credits")],
        titles: &[T(0, 2.0, 5.0, "t-title", 90.0), T(0, 872.0, 10.0, "t-credits", 56.0)],
        tint: (rgb(0x3A, 0x4A, 0x6A), rgb(0x10, 0x14, 0x20)),
    },
    Template {
        id: "music-video",
        icon: Icon::MusicNotes,
        width: 1920,
        height: 1080,
        fps: 30.0,
        lanes: &[TITLES, BROLL, MAIN, MUSIC],
        markers: &[(0.0, "m-intro"), (8.0, "m-verse1"), (24.0, "m-chorus"), (40.0, "m-verse2"), (56.0, "m-chorus2"), (72.0, "m-bridge"), (80.0, "m-final"), (96.0, "m-outro")],
        titles: &[T(0, 1.0, 5.0, "t-title", 84.0)],
        tint: (rgb(0xC0, 0x40, 0xC0), rgb(0x30, 0x10, 0x50)),
    },
    Template {
        id: "animated-story",
        icon: Icon::Sparkle,
        width: 1920,
        height: 1080,
        fps: 24.0,
        lanes: &[TITLES, ("Characters", LaneKind::Visual, rgb(0xE8, 0x9C, 0x7F)), ("Backgrounds", LaneKind::Visual, rgb(0x7F, 0xA8, 0xE8)), MAIN, ("Dialogue", LaneKind::Audio, rgb(0x6C, 0xC6, 0xA0)), ("Sound effects", LaneKind::Audio, rgb(0x6C, 0xA0, 0xC6)), MUSIC],
        markers: &[(0.0, "m-scene1"), (20.0, "m-scene2"), (40.0, "m-scene3"), (60.0, "m-scene4"), (80.0, "m-scene5")],
        titles: &[T(0, 0.5, 4.0, "t-title", 90.0), T(0, 95.0, 5.0, "t-end", 90.0)],
        tint: (rgb(0xF0, 0xA0, 0x30), rgb(0x80, 0x30, 0x40)),
    },
    Template {
        id: "documentary",
        icon: Icon::FilmStrip,
        width: 1920,
        height: 1080,
        fps: 25.0,
        lanes: &[("Titles", LaneKind::Visual, rgb(0xB4, 0x8C, 0xF0)), BROLL, ("Interview", LaneKind::Spine, rgb(0x8B, 0x6C, 0xFF)), ("Narration", LaneKind::Audio, rgb(0x6C, 0xC6, 0xA0)), ("Ambience", LaneKind::Audio, rgb(0x6C, 0xA0, 0xC6)), MUSIC],
        markers: &[(0.0, "m-cold"), (30.0, "m-title"), (60.0, "m-part1"), (600.0, "m-part2"), (1200.0, "m-part3"), (1700.0, "m-closing")],
        titles: &[T(0, 5.0, 5.0, "t-lower", 56.0), T(0, 30.0, 6.0, "t-title", 84.0)],
        tint: (rgb(0x5A, 0x7A, 0x5A), rgb(0x18, 0x24, 0x1C)),
    },
    Template {
        id: "podcast",
        icon: Icon::Microphone,
        width: 1920,
        height: 1080,
        fps: 30.0,
        lanes: &[TITLES, ("Camera 2", LaneKind::Visual, rgb(0x7F, 0xA8, 0xE8)), ("Camera 1", LaneKind::Spine, rgb(0x8B, 0x6C, 0xFF)), ("Host", LaneKind::Audio, rgb(0x6C, 0xC6, 0xA0)), ("Guest", LaneKind::Audio, rgb(0x6C, 0xA0, 0xC6)), MUSIC],
        markers: &[(0.0, "m-intro"), (60.0, "m-sponsor"), (120.0, "m-seg1"), (1200.0, "m-seg2"), (2400.0, "m-outro")],
        titles: &[T(0, 1.0, 7.0, "t-title", 80.0), T(0, 2390.0, 10.0, "t-follow", 64.0)],
        tint: (rgb(0x30, 0x90, 0xB0), rgb(0x10, 0x20, 0x40)),
    },
    Template {
        id: "ad",
        icon: Icon::DeviceMobile,
        width: 1080,
        height: 1920,
        fps: 30.0,
        lanes: &[TITLES, ("Product", LaneKind::Visual, rgb(0x7F, 0xA8, 0xE8)), MAIN, VOICE, MUSIC],
        markers: &[(0.0, "m-hook"), (3.0, "m-problem"), (8.0, "m-solution"), (16.0, "m-proof"), (24.0, "m-cta")],
        titles: &[T(0, 0.3, 3.0, "t-headline", 96.0), T(0, 24.0, 6.0, "t-cta", 96.0)],
        tint: (rgb(0xF0, 0x70, 0x30), rgb(0x70, 0x20, 0x20)),
    },
];

impl Template {
    pub fn by_id(id: &str) -> Option<&'static Template> {
        ALL.iter().find(|t| t.id == id)
    }

    /// Message ids this template uses (for the catalogue tests).
    pub fn message_ids(&self) -> Vec<String> {
        let mut v = vec![format!("tpl-{}-name", self.id), format!("tpl-{}-blurb", self.id)];
        v.extend(self.markers.iter().map(|(_, k)| format!("tpl-{}-{k}", self.id)));
        v.extend(self.titles.iter().map(|t| format!("tpl-{}-{}", self.id, t.3)));
        v
    }

    pub fn name(&self, lang: Lang) -> String {
        tr_in(lang, &format!("tpl-{}-name", self.id), &[])
    }

    pub fn blurb(&self, lang: Lang) -> String {
        tr_in(lang, &format!("tpl-{}-blurb", self.id), &[])
    }

    pub fn ratio(&self) -> String {
        let g = gcd(self.width, self.height);
        let (a, b) = (self.width / g, self.height / g);
        // 1920×804 is the 2.39:1 "scope" frame, not 160:67.
        if self.width as f32 / self.height as f32 > 2.3 { "2.39:1".into() } else { format!("{a}:{b}") }
    }

    /// The new project, with text in `lang`.
    pub fn build(&self, lang: Lang) -> Project {
        let mut p = view::empty();
        p.name = self.name(lang);
        p.width = self.width;
        p.height = self.height;
        p.fps = self.fps;
        p.lanes = self.lanes.iter().enumerate().map(|(i, (name, kind, colour))| view::lane(i as u32 + 1, name, *kind, *colour)).collect();
        p.markers = self.markers.iter().map(|(s, k)| Marker { id: 0, time: (s * SEC as f64) as i64, note: tr_in(lang, &format!("tpl-{}-{k}", self.id), &[]) }).collect();
        let mut id = 100;
        for T(lane, start, dur, key, size) in self.titles {
            id += 1;
            let text = tr_in(lang, &format!("tpl-{}-{key}", self.id), &[]);
            let mut c = view::clip(id, p.lanes[*lane].id, *start, *dur, ClipKind::Title, &text, None);
            c.text = text;
            c.extra.text_size = *size;
            if *key == "t-lower" {
                c.transform = view::Transform { x: -560.0, y: 380.0, scale: 70.0, ..Default::default() };
            }
            p.clips.push(c);
        }
        p
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Practice projects that use the built-in sample footage.
pub const GUIDED: &[(&str, Icon)] = &[("basics", Icon::Cursor), ("colour", Icon::Palette), ("titles", Icon::TextT), ("audio", Icon::Faders)];

/// Message ids the guided projects use.
pub fn guided_message_ids() -> Vec<String> {
    let mut v = Vec::new();
    for (g, _) in GUIDED {
        v.push(format!("guide-{g}-name"));
        v.push(format!("guide-{g}-blurb"));
        for i in 1..=guided_steps(g) {
            v.push(format!("guide-{g}-step{i}"));
        }
    }
    v
}

fn guided_steps(kind: &str) -> usize {
    match kind {
        "basics" => 5,
        "colour" => 3,
        "titles" => 3,
        "audio" => 3,
        _ => 0,
    }
}

/// A guided project: the sample vlog with the first steps as markers.
pub fn build_guided(kind: &str, lang: Lang) -> Project {
    let mut p = view::demo();
    p.name = tr_in(lang, &format!("guide-{kind}-name"), &[]);
    p.markers.clear();
    p.transitions.clear();
    p.range = None;
    // Steps sit where they make sense on the sample's timeline.
    let at: &[f64] = match kind {
        "basics" => &[0.0, 3.0, 6.0, 12.0, 20.0],
        "colour" => &[0.5, 6.5, 14.5],
        "titles" => &[0.5, 7.5, 47.0],
        _ => &[0.5, 6.5, 20.0],
    };
    for (i, s) in at.iter().enumerate() {
        p.markers.push(Marker { id: 0, time: (s * SEC as f64) as i64, note: tr_in(lang, &format!("guide-{kind}-step{}", i + 1), &[]) });
    }
    if kind == "colour" {
        // Make the shots clearly different so there is something to fix.
        for (i, c) in p.clips.iter_mut().filter(|c| c.kind == ClipKind::Video && c.lane == 1).take(3).enumerate() {
            c.grade.temperature = [0.0, -35.0, 30.0][i];
            c.grade.exposure = [0.0, -0.6, 0.5][i];
            c.look = None;
        }
    }
    if kind == "audio" {
        for c in p.clips.iter_mut().filter(|c| c.kind == ClipKind::Audio && c.name == "Easy morning") {
            c.audio.volume = 0.0;
            c.keys.clear();
        }
    }
    p
}

// --- the Start screen row ---------------------------------------------------------

/// "Start from a template" and "Learn by doing" rows. Returns the project
/// to open, if a tile was clicked.
pub fn show_rows(app: &mut StudioApp, ui: &mut eframe::egui::Ui) -> Option<Project> {
    use crate::kit;
    use eframe::egui::{self, Align2, Rect, Vec2};
    let t = crate::theme::tokens(ui.ctx());
    let lang = crate::i18n::lang();
    let mut chosen = None;

    let heading = |ui: &mut egui::Ui, id: &str| {
        ui.label(egui::RichText::new(crate::i18n::tr(id)).font(crate::theme::section_title()).color(t.text));
        ui.add_space(8.0);
    };

    heading(ui, "home-templates");
    let names: Vec<String> = ALL.iter().map(|x| x.name(lang)).collect();
    let ratios: Vec<String> = ALL.iter().map(|x| x.ratio()).collect();
    let blurbs: Vec<String> = ALL.iter().map(|x| x.blurb(lang)).collect();
    let out = kit::TileGrid::new("templates").min_width(168.0).surface(t.ground).show(
        ui,
        ALL.len(),
        |i| kit::TileInfo { label: names[i].as_str(), badge: Some(ratios[i].as_str()), sub: None, selected: false, addable: false, progress: None, tip: Some(blurbs[i].as_str()), added: false },
        |i| i,
        &mut |p, r: Rect, i, hover, _| {
            let tpl = &ALL[i];
            // A miniature of the timeline the template sets up: its lanes in
            // their colours, cut into clips, under its markers.
            let ink = |c: Color32| if t.dark { c } else { crate::theme::mix(c, Color32::BLACK, 0.22) };
            p.rect_filled(r, 0, crate::theme::mix(t.sunken, tpl.tint.0, 0.08 + 0.04 * hover));
            let area = r.shrink2(Vec2::new(14.0, 12.0));
            crate::icons::paint(p, Rect::from_min_size(area.min, Vec2::splat(15.0)), tpl.icon, 15.0, t.text_2);
            let body = Rect::from_min_max(egui::pos2(area.left(), area.top() + 24.0), egui::pos2(area.right(), area.bottom() - 4.0));
            let n = tpl.lanes.len().max(1) as f32;
            let lane_h = ((body.height() - (n - 1.0) * 3.0) / n).min(9.0);
            let end = tpl.markers.iter().map(|m| m.0).fold(1.0_f64, f64::max) * 1.08;
            let top = body.bottom() - n * lane_h - (n - 1.0) * 3.0;
            for &(at, _) in tpl.markers {
                let x = body.left() + (at / end) as f32 * body.width();
                p.rect_filled(Rect::from_center_size(egui::pos2(x, top - 6.0), Vec2::new(2.0, 5.0)), 1, ink(MUSIC.2));
            }
            for (li, lane) in tpl.lanes.iter().enumerate() {
                let y = top + li as f32 * (lane_h + 3.0);
                // Clip cuts from a small hash of the template and lane, so
                // each tile keeps its own steady rhythm.
                let mut h = (i as u32 + 1).wrapping_mul(2654435761) ^ (li as u32 + 7).wrapping_mul(40503);
                let mut x = body.left() + if lane.1 == LaneKind::Visual { ((h >> 3) % 40) as f32 } else { 0.0 };
                while x < body.right() - 6.0 {
                    h = h.wrapping_mul(1664525).wrapping_add(1013904223);
                    let w = (18 + (h >> 24) % 60) as f32;
                    let x1 = (x + w).min(body.right());
                    p.rect_filled(Rect::from_min_max(egui::pos2(x, y), egui::pos2(x1, y + lane_h)), 2, ink(lane.2));
                    x = x1 + if lane.1 == LaneKind::Visual { (4 + (h >> 20) % 30) as f32 } else { 2.0 };
                }
            }
            let _ = Align2::CENTER_CENTER;
        },
    );
    if let Some(i) = out.clicked.or(out.double_clicked) {
        chosen = Some(ALL[i].build(lang));
    }

    ui.add_space(20.0);
    heading(ui, "home-learn");
    let gnames: Vec<String> = GUIDED.iter().map(|(g, _)| tr_in(lang, &format!("guide-{g}-name"), &[])).collect();
    let gblurbs: Vec<String> = GUIDED.iter().map(|(g, _)| tr_in(lang, &format!("guide-{g}-blurb"), &[])).collect();
    let out = kit::TileGrid::new("guided").min_width(168.0).surface(t.ground).show(
        ui,
        GUIDED.len(),
        |i| kit::TileInfo { label: gnames[i].as_str(), badge: None, sub: None, selected: false, addable: false, progress: None, tip: Some(gblurbs[i].as_str()), added: false },
        |i| i,
        &mut |p, r: Rect, i, hover, _| {
            p.rect_filled(r, 0, crate::theme::mix(t.raised, t.accent, 0.12 + 0.08 * hover));
            crate::icons::paint(p, Rect::from_center_size(r.center(), Vec2::splat(28.0)), GUIDED[i].1, 28.0, t.text);
        },
    );
    if let Some(i) = out.clicked.or(out.double_clicked) {
        chosen = Some(build_guided(GUIDED[i].0, lang));
    }
    ui.add_space(20.0);
    let _ = app;
    chosen
}

/// Open a built project as a new, saved one.
pub fn open(app: &mut StudioApp, ctx: &eframe::egui::Context, project: Project) {
    app.replace(ctx, crate::files::Replace::New(project));
}

#[allow(dead_code)]
fn _unused(_: &Lane) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_templates_for_the_seven_kinds() {
        let ids: Vec<_> = ALL.iter().map(|t| t.id).collect();
        assert_eq!(ids, ["youtube", "short-film", "music-video", "animated-story", "documentary", "podcast", "ad"]);
    }

    #[test]
    fn every_template_builds_a_valid_project() {
        for t in ALL {
            let p = t.build(Lang::En);
            assert_eq!((p.width, p.height), (t.width, t.height));
            assert_eq!(p.lanes.iter().filter(|l| l.kind == LaneKind::Spine).count(), 1, "{}: one main lane", t.id);
            let mut ids: Vec<u32> = p.lanes.iter().map(|l| l.id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), p.lanes.len(), "{}: lane ids are unique", t.id);
            assert!(p.markers.windows(2).all(|w| w[0].time < w[1].time), "{}: markers in order", t.id);
            assert!(p.markers.iter().all(|m| !m.note.is_empty() && !m.note.starts_with("tpl-")), "{}: marker notes come from the catalogue", t.id);
            for c in &p.clips {
                assert!(p.lane(c.lane).is_some_and(|l| l.kind == LaneKind::Visual), "{}: titles sit on a free visual lane", t.id);
                assert!(!c.text.is_empty() && !c.text.starts_with("tpl-"), "{}: title text", t.id);
            }
        }
    }

    #[test]
    fn templates_are_complete_in_every_language() {
        for t in ALL {
            for l in Lang::ALL {
                for id in t.message_ids() {
                    assert!(studio_i18n::catalogue(l).contains(&id) || studio_i18n::is_pending(&id), "{} missing in {}", id, l.tag());
                }
            }
        }
        for l in Lang::ALL {
            for id in guided_message_ids() {
                assert!(studio_i18n::catalogue(l).contains(&id) || studio_i18n::is_pending(&id), "{} missing in {}", id, l.tag());
            }
        }
    }

    #[test]
    fn project_text_follows_the_language() {
        let en = Template::by_id("ad").unwrap().build(Lang::En);
        let fr = Template::by_id("ad").unwrap().build(Lang::Fr);
        if !studio_i18n::is_pending("tpl-ad-name") {
            assert_ne!(en.name, fr.name);
        }
        assert_eq!(en.lanes[2].name, fr.lanes[2].name, "lane names stay English in the file");
    }

    #[test]
    fn ratios_read_naturally() {
        assert_eq!(Template::by_id("youtube").unwrap().ratio(), "16:9");
        assert_eq!(Template::by_id("ad").unwrap().ratio(), "9:16");
        assert_eq!(Template::by_id("short-film").unwrap().ratio(), "2.39:1");
    }

    #[test]
    fn guided_projects_have_their_steps() {
        for (g, _) in GUIDED {
            let p = build_guided(g, Lang::En);
            assert_eq!(p.markers.len(), guided_steps(g), "{g}");
            assert!(!p.clips.is_empty());
            assert!(p.markers.windows(2).all(|w| w[0].time < w[1].time), "{g}");
        }
        // The colour practice really has mismatched shots.
        let p = build_guided("colour", Lang::En);
        let temps: Vec<f32> = p.clips.iter().filter(|c| c.kind == ClipKind::Video && c.lane == 1).take(3).map(|c| c.grade.temperature).collect();
        assert_eq!(temps, [0.0, -35.0, 30.0]);
    }
}
