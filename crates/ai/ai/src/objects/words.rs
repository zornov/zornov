// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The answers, in words: short by default, with the detail on request. A
//! line a thing; the list is cut to the most prominent and says how many
//! more there are. Positions are canvas pixels from the centre (y down,
//! like everything the editor takes), times are the timeline's.

use super::geometry::length_words;
use super::relations::{self, Rel};
use super::space::{place_words, Free};
use super::{BBox, Kind, Movement, Object, ObjectMap, TextItem};

/// How the answers are placed in the editor.
pub struct View<'a> {
    /// A point of the footage (0..1 each way) as canvas pixels from the
    /// centre.
    pub canvas: &'a dyn Fn(f32, f32) -> (f32, f32),
    /// Seconds into the footage as seconds on the timeline.
    pub time: &'a dyn Fn(f32) -> f32,
    /// The inverse: timeline seconds into the footage.
    pub footage_time: &'a dyn Fn(f32) -> f32,
}

impl View<'_> {
    pub fn rect(&self, b: &BBox) -> [i32; 4] {
        let (x0, y0) = (self.canvas)(b.x, b.y);
        let (x1, y1) = (self.canvas)(b.right(), b.bottom());
        [x0.min(x1).round() as i32, y0.min(y1).round() as i32, x0.max(x1).round() as i32, y0.max(y1).round() as i32]
    }
    pub fn centre(&self, b: &BBox) -> [i32; 2] {
        let (x, y) = (self.canvas)(b.cx(), b.cy());
        [x.round() as i32, y.round() as i32]
    }
    pub fn t(&self, secs: f32) -> String {
        format!("{:.1}", (self.time)(secs))
    }
}

/// What to say.
#[derive(Clone, Copy, Debug)]
pub struct ListOptions {
    /// Lines at most (the most prominent first).
    pub max: usize,
    /// Everything known about each object, not just the first line.
    pub detail: bool,
    /// Only objects present at this moment (footage seconds).
    pub at: Option<f32>,
}

impl Default for ListOptions {
    fn default() -> Self {
        ListOptions { max: 12, detail: false, at: None }
    }
}

/// "left", "top right", "centre" for where a box sits.
fn where_words(b: &BBox) -> String {
    place_words(b)
}

/// "woman" for a person described as one, else the label.
pub fn name_of(o: &Object) -> String {
    match (&o.descriptor, o.kind) {
        (Some(d), Kind::Person) => d.clone(),
        _ => o.label.clone(),
    }
}

fn percent(x: f32) -> u32 {
    (x * 100.0).round().clamp(0.0, 99.0) as u32
}

fn direction(m: &Movement) -> &'static str {
    let a = m.heading.rem_euclid(360.0);
    match ((a + 22.5) / 45.0) as u32 % 8 {
        0 => "right",
        1 => "down and right",
        2 => "down",
        3 => "down and left",
        4 => "left",
        5 => "up and left",
        6 => "up",
        _ => "up and right",
    }
}

/// How the object moves, apart from the camera.
fn movement_words(m: &Movement) -> String {
    let mut s = if m.still {
        "still in the place".to_string()
    } else {
        let pace = if m.speed > 0.4 { "fast" } else if m.speed > 0.12 { "steadily" } else { "slowly" };
        format!("moving {} {pace} ({:.2} frame widths/s)", direction(m), m.speed)
    };
    if let Some(v) = m.depth_speed {
        if v.abs() > 0.3 {
            s += &format!(", {} the camera at {:.1} m/s", if v < 0.0 { "toward" } else { "away from" }, v.abs());
        }
    }
    s
}

/// One object's first line.
fn line(o: &Object, v: &View, t: Option<f32>, detail: bool, map: &ObjectMap) -> String {
    let ob = t.and_then(|t| o.at(t)).or_else(|| o.at((o.enter + o.leave) / 2.0)).unwrap_or_else(|| o.obs[0].clone());
    let mut parts: Vec<String> = Vec::new();
    let mut head = format!("#{} {}", o.id, name_of(o));
    if o.kind == Kind::Person && o.descriptor.is_some() {
        head = format!("#{} person ({})", o.id, name_of(o));
    }
    head += &format!(" {}%", percent(o.score));
    parts.push(head);
    parts.push(where_words(&ob.bbox));
    if let Some(z) = ob.z {
        parts.push(format!("{} away", length_words(z)));
    }
    let span = o.leave - o.enter;
    let whole = !o.enters && !o.leaves;
    parts.push(if whole { "whole shot".to_string() } else if span < 0.1 { format!("at {}s", v.t(o.enter)) } else { format!("{}-{}s", v.t(o.enter), v.t(o.leave)) });
    if detail {
        if let Some(sz) = o.size {
            let big = sz[0].max(sz[1]);
            parts.push(format!("~{} {}", length_words(big), if sz[1] >= sz[0] { "tall" } else { "wide" }));
        }
        if !o.colours.is_empty() {
            parts.push(o.colours.join(", "));
        }
        if let Some(f) = &o.facing {
            parts.push(f.clone());
        }
        if let Some(m) = &o.movement {
            parts.push(movement_words(m));
        }
        if ob.face.is_some() && o.kind == Kind::Person {
            parts.push("face found".into());
        }
        if !o.texts.is_empty() {
            parts.push(format!("text {}", o.texts.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ")));
        }
        if !o.also.is_empty() {
            parts.push(format!("also called {}", o.also.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", ")));
        }
        let tt = t.unwrap_or((o.enter + o.leave) / 2.0);
        let rels: Vec<String> = relations::all(map, tt)
            .iter()
            .filter(|r| r.a == o.id)
            .filter_map(|r| Some(format!("{} #{}", r.rel.words(), r.b)))
            .take(3)
            .collect();
        if !rels.is_empty() {
            parts.push(rels.join(", "));
        }
        let r = v.rect(&ob.bbox);
        parts.push(format!("box [{}, {}]-[{}, {}]", r[0], r[1], r[2], r[3]));
    } else if let Some(m) = &o.movement {
        if !m.still && m.speed > 0.06 {
            parts.push(format!("moving {}", direction(m)));
        }
    }
    parts.join(" · ")
}

/// The header: how long, how many, what the camera does.
fn header(map: &ObjectMap, shown: usize, total: usize, v: &View) -> String {
    let plural = if total == 1 { "" } else { "s" };
    let mut h = if map.duration < 0.2 { format!("one frame, {total} object{plural}") } else { format!("{:.1}s of footage, {total} object{plural}", map.duration) };
    if shown < total {
        h += &format!(" (the {shown} most prominent)");
    }
    if map.duration > 0.4 && !map.camera.summary.is_empty() {
        h += &format!(", camera {}", map.camera.summary);
    }
    if map.scale.anchors == 0 && map.objects.iter().any(|o| o.obs.iter().any(|b| b.z.is_some())) {
        h += "; distances and sizes are rough (nothing of known size in view)";
    }
    let _ = v;
    h
}

/// What is in the footage.
pub fn list(map: &ObjectMap, v: &View, o: &ListOptions) -> String {
    let mut objs: Vec<&Object> = map.objects.iter().filter(|x| o.at.is_none_or(|t| x.visible(t))).collect();
    objs.sort_by(|a, b| b.prominence().total_cmp(&a.prominence()));
    let total = objs.len();
    objs.truncate(o.max.max(1));
    if objs.is_empty() {
        return match o.at {
            Some(t) => format!("nothing found at {}s", v.t(t)),
            None => "nothing found".into(),
        };
    }
    let mut out = vec![header(map, objs.len(), total, v)];
    for x in &objs {
        out.push(line(x, v, o.at, o.detail, map));
    }
    if total > objs.len() {
        out.push(format!("... {} more (ask with detail or max, or for one by name)", total - objs.len()));
    }
    out.join("\n")
}

/// One short sentence for `see`: "people ×2, dog, sofa, table".
pub fn brief(map: &ObjectMap, max: usize) -> String {
    let mut counts: Vec<(String, usize, f32)> = Vec::new();
    for o in map.objects.iter().filter(|o| o.score >= 0.25 && o.kind != Kind::Scenery || o.score >= 0.4) {
        let name = o.label.clone();
        let p = o.prominence();
        match counts.iter_mut().find(|c| c.0 == name) {
            Some(c) => {
                c.1 += 1;
                c.2 = c.2.max(p);
            }
            None => counts.push((name, 1, p)),
        }
    }
    counts.sort_by(|a, b| b.2.total_cmp(&a.2));
    let total = counts.len();
    counts.truncate(max);
    if counts.is_empty() {
        return String::new();
    }
    let mut s = counts.iter().map(|(n, c, _)| if *c > 1 { format!("{n} x{c}") } else { n.clone() }).collect::<Vec<_>>().join(", ");
    if total > counts.len() {
        s += &format!(", +{} more", total - counts.len());
    }
    s
}

/// Where one object is at `t` (footage seconds).
pub fn where_at(map: &ObjectMap, o: &Object, t: f32, v: &View) -> String {
    let Some(ob) = o.at(t) else {
        return format!("#{} {} is not in view at {}s; it is in view {}-{}s", o.id, name_of(o), v.t(t), v.t(o.enter), v.t(o.leave));
    };
    let r = v.rect(&ob.bbox);
    let c = v.centre(&ob.bbox);
    let mut s = format!(
        "#{} {} at {}s: {} of the frame, box [{}, {}]-[{}, {}] ({} x {} px), centre [{}, {}]",
        o.id,
        name_of(o),
        v.t(t),
        where_words(&ob.bbox),
        r[0],
        r[1],
        r[2],
        r[3],
        r[2] - r[0],
        r[3] - r[1],
        c[0],
        c[1]
    );
    if let Some(z) = ob.z {
        let cam = super::geometry::Camera::new(map.width as f32, map.height as f32, map.scale.hfov_deg);
        let p = cam.unproject(ob.bbox.cx() * cam.width, ob.bbox.cy() * cam.height, z);
        s += &format!(
            "; {} from the camera, 3D [x {:.1}, y {:.1}, z {:.1}] m (right, up, forward of the lens)",
            length_words(z),
            p[0],
            p[1],
            p[2]
        );
        if let Some(sz) = o.size {
            s += &format!(", about {} x {} (wide x tall)", length_words(sz[0]), length_words(sz[1]));
        }
        if map.scale.anchors == 0 {
            s += " (rough: nothing of known size in view)";
        }
    }
    if let Some(m) = &o.movement {
        s += &format!("; {}", movement_words(m));
    }
    if let Some(f) = ob.face {
        let r = v.rect(&f);
        s += &format!("; face [{}, {}]-[{}, {}]", r[0], r[1], r[2], r[3]);
    }
    if let Some(h) = ob.head {
        let r = v.rect(&h);
        s += &format!("; head [{}, {}]-[{}, {}]", r[0], r[1], r[2], r[3]);
    }
    if let Some(f) = &o.facing {
        s += &format!("; {f}");
    }
    if !o.colours.is_empty() {
        s += &format!("; {}", o.colours.join(", "));
    }
    if !o.texts.is_empty() {
        s += &format!("; text {}", o.texts.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" "));
    }
    s
}

/// When the objects of that name are in view.
pub fn when(objs: &[&Object], map: &ObjectMap, v: &View) -> String {
    if objs.is_empty() {
        return "not found in this footage".into();
    }
    let mut out = Vec::new();
    for o in objs.iter().take(8) {
        let whole = !o.enters && !o.leaves;
        let mut s = format!("#{} {}: ", o.id, name_of(o));
        if whole {
            s += &format!("in view the whole time ({}-{}s)", v.t(0.0), v.t(map.duration));
        } else {
            s += &format!("in view {}-{}s", v.t(o.enter), v.t(o.leave));
            if o.enters {
                s += &format!(", comes in at {}s", v.t(o.enter));
            }
            if o.leaves {
                s += &format!(", leaves at {}s", v.t(o.leave));
            }
        }
        out.push(s);
    }
    if objs.len() > 8 {
        out.push(format!("... {} more", objs.len() - 8));
    }
    out.join("\n")
}

/// What is around one object at `t`.
pub fn around(map: &ObjectMap, o: &Object, t: f32, v: &View) -> String {
    let rel = relations::around(map, t, o.id);
    let mut out = vec![format!("around #{} {} at {}s:", o.id, name_of(o), v.t(t))];
    if rel.is_empty() {
        out.push("  nothing else is on screen with it".into());
    }
    let mut seen: Vec<(u32, Rel)> = Vec::new();
    for (id, r) in rel {
        if seen.contains(&(id, r)) {
            continue;
        }
        seen.push((id, r));
        if seen.len() > 10 {
            break;
        }
        let Some(other) = map.object(id) else { continue };
        let z = other.at(t).and_then(|b| b.z).map(|z| format!(", {} away", length_words(z))).unwrap_or_default();
        out.push(format!("  {} #{} {}{z}", r.words(), id, name_of(other)));
    }
    let front = relations::depth_neighbours(map, t, o.id, true);
    let behind = relations::depth_neighbours(map, t, o.id, false);
    let names = |ids: &[u32]| ids.iter().filter_map(|&i| map.object(i)).map(|x| format!("#{} {}", x.id, name_of(x))).collect::<Vec<_>>().join(", ");
    if !front.is_empty() {
        out.push(format!("  in front of it (nearest first): {}", names(&front)));
    }
    if !behind.is_empty() {
        out.push(format!("  behind it (nearest first): {}", names(&behind)));
    }
    out.join("\n")
}

/// Free rectangles for titles and placed things.
pub fn space(free: &[Free], v: &View, from: f32, to: f32) -> String {
    if free.is_empty() {
        return "no clear space of that size".into();
    }
    let mut out = vec![format!("free space between {}s and {}s (canvas px from the centre; nothing in it but background):", v.t(from), v.t(to))];
    for f in free {
        let r = v.rect(&f.rect);
        out.push(format!("  {} · [{}, {}]-[{}, {}] · {} x {} px · {:.0}% of the frame", place_words(&f.rect), r[0], r[1], r[2], r[3], r[2] - r[0], r[3] - r[1], f.share * 100.0));
    }
    out.join("\n")
}

/// Text read in the picture.
pub fn texts(map: &ObjectMap, v: &View) -> String {
    let mut seen: Vec<(String, &TextItem)> = Vec::new();
    for t in &map.texts {
        let key = t.text.to_lowercase();
        if !seen.iter().any(|s| s.0 == key) {
            seen.push((key, t));
        }
    }
    if seen.is_empty() {
        return "no text found".into();
    }
    let mut out = vec![format!("text in the picture ({}):", seen.len())];
    for (_, t) in seen.iter().take(12) {
        let r = v.rect(&t.bbox);
        let on = t.object.and_then(|i| map.object(i)).map(|o| format!(" on #{} {}", o.id, name_of(o))).unwrap_or_default();
        out.push(format!("  \"{}\" at {}s [{}, {}]-[{}, {}]{on}", t.text, v.t(t.t), r[0], r[1], r[2], r[3]));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn view() -> View<'static> {
        View { canvas: &|x, y| ((x - 0.5) * 1920.0, (y - 0.5) * 1080.0), time: &|t| t + 10.0, footage_time: &|t| t - 10.0 }
    }

    fn crowd(n: usize) -> ObjectMap {
        let mut objs = Vec::new();
        for i in 0..n {
            let mut o = object(i as u32 + 1, ["person", "dog", "chair", "lamp", "cup"][i % 5], Kind::Thing, vec![obs(0.0, (i % 10) as f32 * 0.08, (i / 10) as f32 * 0.1, 0.05 + 0.002 * i as f32, 0.1), obs(5.0, (i % 10) as f32 * 0.08, (i / 10) as f32 * 0.1, 0.05, 0.1)]);
            o.colours = vec!["red".into(), "white".into()];
            o.size = Some([0.5, 1.2]);
            o.texts = vec!["SALE".into()];
            o.also = vec![("thing".into(), 0.3)];
            o.obs[0].z = Some(3.2);
            o.score = 0.5 + (i as f32 % 7.0) * 0.05;
            objs.push(o);
        }
        map(objs)
    }

    #[test]
    fn the_default_list_is_short_however_many_there_are() {
        let m = crowd(60);
        let s = list(&m, &view(), &ListOptions::default());
        assert!(s.lines().count() <= 14, "{s}");
        assert!(s.len() < 1_800, "{} chars", s.len());
        assert!(s.contains("60 objects") && s.contains("more"), "{s}");
        // Times are the timeline's, not the footage's.
        assert!(s.contains("10.0-15.0s") || s.contains("whole shot"), "{s}");
    }

    #[test]
    fn detail_costs_more_but_stays_bounded() {
        let m = crowd(60);
        let brief_len = list(&m, &view(), &ListOptions::default()).len();
        let detail = list(&m, &view(), &ListOptions { detail: true, max: 12, at: None });
        assert!(detail.len() > brief_len);
        assert!(detail.len() < 4_500, "{} chars", detail.len());
        assert!(detail.contains("box [") && detail.contains("\"SALE\""));
    }

    #[test]
    fn the_brief_for_see_is_a_line() {
        let m = crowd(30);
        let b = brief(&m, 3);
        assert!(b.len() < 120, "{b}");
        assert!(b.contains("+2 more") && b.contains(" x6"), "{b}");
        assert!(!brief(&m, 6).contains("more"));
        assert_eq!(brief(&map(vec![]), 6), "");
    }

    #[test]
    fn where_gives_canvas_pixels_and_3d() {
        let mut o = object(3, "person", Kind::Person, vec![obs(0.0, 0.4, 0.2, 0.2, 0.6), obs(4.0, 0.6, 0.2, 0.2, 0.6)]);
        o.obs[0].z = Some(3.0);
        o.obs[1].z = Some(3.0);
        o.descriptor = Some("woman".into());
        let m = map(vec![o]);
        let s = where_at(&m, m.object(3).unwrap(), 2.0, &view());
        assert!(s.contains("#3 woman at 12.0s"), "{s}");
        assert!(s.contains("box [0, -324]-[384, 324]"), "{s}");
        assert!(s.contains("3D [x") && s.contains("rough"), "{s}");
        let gone = where_at(&m, m.object(3).unwrap(), 5.0, &view());
        assert!(gone.contains("not in view"), "{gone}");
    }

    #[test]
    fn when_says_enter_and_leave() {
        let mut o = object(1, "dog", Kind::Animal, vec![obs(2.0, 0.0, 0.0, 0.1, 0.1), obs(4.0, 0.0, 0.0, 0.1, 0.1)]);
        o.enters = true;
        o.leaves = true;
        let mut m = map(vec![o]);
        m.duration = 6.0;
        let s = when(&m.objects.iter().collect::<Vec<_>>(), &m, &view());
        assert!(s.contains("comes in at 12.0s") && s.contains("leaves at 14.0s"), "{s}");
        assert_eq!(when(&[], &m, &view()), "not found in this footage");
    }

    #[test]
    fn free_space_and_text_read_well() {
        let f = [Free { rect: BBox::new(0.05, 0.05, 0.4, 0.2), share: 0.08 }];
        let s = space(&f, &view(), 0.0, 3.0);
        assert!(s.contains("top left") && s.contains("[-864, -486]-[-96, -270]"), "{s}");
        assert_eq!(space(&[], &view(), 0.0, 1.0), "no clear space of that size");
        let mut m = map(vec![]);
        m.texts.push(TextItem { t: 1.0, text: "OPEN".into(), bbox: BBox::new(0.1, 0.1, 0.2, 0.05), score: 0.9, object: None });
        m.texts.push(TextItem { t: 2.0, text: "open".into(), bbox: BBox::new(0.1, 0.1, 0.2, 0.05), score: 0.9, object: None });
        let s = texts(&m, &view());
        assert_eq!(s.lines().count(), 2, "{s}");
    }

    #[test]
    fn around_names_the_relations() {
        let table = object(1, "table", Kind::Thing, vec![obs(0.0, 0.2, 0.55, 0.6, 0.3)]);
        let cup = object(2, "cup", Kind::Thing, vec![obs(0.0, 0.45, 0.45, 0.06, 0.12)]);
        let m = map(vec![table, cup]);
        let s = around(&m, m.object(2).unwrap(), 0.0, &view());
        assert!(s.contains("on #1 table"), "{s}");
        let s = around(&m, m.object(1).unwrap(), 0.0, &view());
        assert!(s.contains("under #2 cup"), "{s}");
    }
}
