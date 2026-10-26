// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lessons: what the AI editor learned by doing a job, kept for the next
//! one. After a job (or after the person corrects it) the AI saves a short
//! lesson tagged to a skill (`skill {do: "learn"}`): what worked, what
//! failed and how it was fixed. When the skill loads, its lessons are
//! appended to it.
//!
//! * Lessons live in the person's Studio data folder (`AI/lessons.json`),
//!   never in a project or the repository: they are about how this person
//!   works with the AI, and they follow the person across projects.
//! * Newest first, deduplicated: the same lesson said again (or one that
//!   shares nearly all its words) is not stored twice, it is **confirmed**:
//!   its count goes up and its date moves to now.
//! * Capped: a skill shows at most [`SHOW_MAX`] lessons in [`SHOW_CHARS`]
//!   characters, and the file keeps at most [`PER_SKILL`] per skill and
//!   [`TOTAL`] in all.
//! * Ageing: a lesson seen once and not confirmed for [`ONE_OFF_DAYS`] days
//!   is dropped. A lesson confirmed more than once is kept, and so is any
//!   lesson the person wrote or edited.
//! * The person can see, edit and delete every lesson (Settings ▸ AI).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The longest lesson, in characters: one or two sentences.
pub const LESSON_MAX: usize = 320;
/// The shortest worth saving.
pub const LESSON_MIN: usize = 12;
/// Most lessons kept for one skill.
pub const PER_SKILL: usize = 30;
/// Most lessons kept in all.
pub const TOTAL: usize = 400;
/// Most lessons appended to a skill when it loads.
pub const SHOW_MAX: usize = 8;
/// Most characters of lessons appended to a skill.
pub const SHOW_CHARS: usize = 1_800;
/// Days a lesson seen once survives without being confirmed.
pub const ONE_OFF_DAYS: u64 = 60;
/// The tag for a lesson that fits no one skill; it rides along with all.
pub const GENERAL: &str = "general";
/// How many general lessons ride along with a skill.
const GENERAL_SHOW: usize = 3;

const DAY: u64 = 86_400;

/// What kind of lesson.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An approach that worked.
    Worked,
    /// Something that failed, and the fix.
    Fixed,
    /// The person corrected the AI.
    Correction,
}

impl Kind {
    pub fn parse(s: &str) -> Kind {
        match s.trim().to_lowercase().as_str() {
            "fixed" | "failed" | "fail" | "failure" | "mistake" | "fix" => Kind::Fixed,
            "correction" | "corrected" | "user" | "person" => Kind::Correction,
            _ => Kind::Worked,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Worked => "worked",
            Kind::Fixed => "fixed",
            Kind::Correction => "correction",
        }
    }
}

/// One lesson.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: u64,
    /// The skill it is tagged to (or [`GENERAL`]).
    pub skill: String,
    pub text: String,
    pub kind: Kind,
    /// Times it was learned or confirmed (1 = a one-off).
    pub seen: u32,
    /// Unix seconds when first saved and when last confirmed.
    pub first: u64,
    pub last: u64,
    /// The person wrote or edited it: it never ages out.
    #[serde(default)]
    pub mine: bool,
}

impl Lesson {
    /// Kept for good: confirmed more than once, or the person's own.
    pub fn kept(&self) -> bool {
        self.seen >= 2 || self.mine
    }
}

/// What `learn` did.
#[derive(Debug, PartialEq)]
pub struct Learned {
    pub id: u64,
    /// The lesson was already known and is now confirmed.
    pub confirmed: bool,
    pub seen: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    next: u64,
    #[serde(default)]
    lessons: Vec<Lesson>,
}

/// The lessons file in a folder.
pub struct Store {
    path: PathBuf,
}

/// One read-modify-write at a time in this process (a crew of agents may
/// all learn at once).
static LOCK: Mutex<()> = Mutex::new(());

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Store {
    /// The lessons kept in `dir`.
    pub fn at(dir: &Path) -> Store {
        Store { path: dir.join("lessons.json") }
    }

    /// The person's own: the AI folder of the Studio data folder.
    pub fn user() -> Store {
        Store::at(&super::taste::ai_dir())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> File {
        std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn write(&self, f: &File) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("lessons folder: {e}"))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?).map_err(|e| format!("lessons: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("lessons: {e}"))
    }

    /// Every lesson, newest first (after ageing, which is saved).
    pub fn all(&self, now: u64) -> Vec<Lesson> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.read();
        let before = f.lessons.len();
        age(&mut f.lessons, now);
        if f.lessons.len() != before {
            let _ = self.write(&f);
        }
        newest_first(f.lessons)
    }

    /// Saves a lesson, or confirms the one it repeats.
    pub fn learn(&self, skill: &str, text: &str, kind: Kind, now: u64) -> Result<Learned, String> {
        let text = clean(text)?;
        let skill = skill.trim().to_lowercase();
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.read();
        age(&mut f.lessons, now);
        if let Some(l) = f.lessons.iter_mut().find(|l| l.skill == skill && same(&l.text, &text)) {
            l.seen += 1;
            l.last = now;
            // The newer wording is kept when it says more.
            if text.len() > l.text.len() && !l.mine {
                l.text = text;
            }
            let out = Learned { id: l.id, confirmed: true, seen: l.seen };
            self.write(&f)?;
            return Ok(out);
        }
        f.next = f.next.max(f.lessons.iter().map(|l| l.id).max().unwrap_or(0)) + 1;
        let id = f.next;
        f.lessons.push(Lesson { id, skill, text, kind, seen: 1, first: now, last: now, mine: false });
        cap(&mut f.lessons);
        self.write(&f)?;
        Ok(Learned { id, confirmed: false, seen: 1 })
    }

    /// Says an existing lesson proved right again.
    pub fn confirm(&self, id: u64, now: u64) -> Result<u32, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.read();
        let l = f.lessons.iter_mut().find(|l| l.id == id).ok_or_else(|| format!("no lesson {id}"))?;
        l.seen += 1;
        l.last = now;
        let seen = l.seen;
        self.write(&f)?;
        Ok(seen)
    }

    /// The person edits a lesson's words (and so keeps it for good).
    pub fn edit(&self, id: u64, text: &str, now: u64) -> Result<(), String> {
        let text = clean(text)?;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.read();
        let l = f.lessons.iter_mut().find(|l| l.id == id).ok_or_else(|| format!("no lesson {id}"))?;
        l.text = text;
        l.mine = true;
        l.last = now;
        self.write(&f)
    }

    /// Deletes a lesson.
    pub fn forget(&self, id: u64) -> Result<(), String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.read();
        let n = f.lessons.len();
        f.lessons.retain(|l| l.id != id);
        if f.lessons.len() == n {
            return Err(format!("no lesson {id}"));
        }
        self.write(&f)
    }

    /// Deletes every lesson (or one skill's).
    pub fn clear(&self, skill: Option<&str>) -> Result<usize, String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.read();
        let n = f.lessons.len();
        f.lessons.retain(|l| skill.is_some_and(|s| l.skill != s.trim().to_lowercase()));
        let gone = n - f.lessons.len();
        self.write(&f)?;
        Ok(gone)
    }

    /// The lessons that ride along when `skill` loads: its own plus a few
    /// general ones, newest first, within [`SHOW_MAX`] and [`SHOW_CHARS`].
    /// When they do not all fit, the confirmed ones stay and the one-offs go.
    pub fn for_skill(&self, skill: &str, now: u64) -> Vec<Lesson> {
        pick(self.all(now), skill)
    }

    /// The appended text for a skill (empty with no lessons).
    pub fn section(&self, skill: &str, now: u64) -> String {
        render(&self.for_skill(skill, now))
    }
}

/// Cleans a lesson's words: one paragraph, within the limits.
fn clean(text: &str) -> Result<String, String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() < LESSON_MIN {
        return Err(format!("a lesson says what worked or what to do instead, in a sentence ({LESSON_MIN} characters or more)"));
    }
    if t.chars().count() > LESSON_MAX {
        return Err(format!("a lesson is one or two sentences ({LESSON_MAX} characters at most): keep the part that changes what you do next time"));
    }
    Ok(t)
}

/// The words of a lesson that carry its meaning.
fn words(s: &str) -> std::collections::BTreeSet<String> {
    const STOP: &[&str] = &["the", "a", "an", "and", "or", "to", "of", "in", "on", "for", "with", "is", "it", "was", "be", "that", "this", "then", "so", "at", "as", "by", "when", "not", "are"];
    s.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| w.len() > 1 && !STOP.contains(w)).map(str::to_owned).collect()
}

/// Whether two lessons say the same thing: the same words, or nearly all
/// of the shorter one's words in the other.
pub fn same(a: &str, b: &str) -> bool {
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return a.trim().eq_ignore_ascii_case(b.trim());
    }
    let common = wa.intersection(&wb).count() as f32;
    let (small, big) = (wa.len().min(wb.len()) as f32, wa.len().max(wb.len()) as f32);
    // Mostly the same words, and not one lesson swallowing a much longer one.
    common / small >= 0.85 && common / big >= 0.6
}

/// Drops the lessons that aged out.
fn age(lessons: &mut Vec<Lesson>, now: u64) {
    lessons.retain(|l| l.kept() || now.saturating_sub(l.last) <= ONE_OFF_DAYS * DAY);
}

fn newest_first(mut v: Vec<Lesson>) -> Vec<Lesson> {
    v.sort_by(|a, b| b.last.cmp(&a.last).then(b.id.cmp(&a.id)));
    v
}

/// How worth keeping: confirmed ones first, then the newer.
fn worth(l: &Lesson) -> (bool, u32, u64) {
    (l.kept(), l.seen, l.last)
}

/// Keeps [`PER_SKILL`] per skill and [`TOTAL`] in all, dropping the least
/// worth keeping.
fn cap(lessons: &mut Vec<Lesson>) {
    let mut skills: Vec<String> = lessons.iter().map(|l| l.skill.clone()).collect();
    skills.sort();
    skills.dedup();
    for s in skills {
        drop_weakest(lessons, |l| l.skill == s, PER_SKILL);
    }
    drop_weakest(lessons, |_| true, TOTAL);
}

fn drop_weakest(lessons: &mut Vec<Lesson>, of: impl Fn(&Lesson) -> bool, keep: usize) {
    let mut ids: Vec<(u64, (bool, u32, u64))> = lessons.iter().filter(|l| of(l)).map(|l| (l.id, worth(l))).collect();
    if ids.len() <= keep {
        return;
    }
    ids.sort_by(|a, b| b.1.cmp(&a.1));
    let gone: Vec<u64> = ids[keep..].iter().map(|x| x.0).collect();
    lessons.retain(|l| !gone.contains(&l.id));
}

/// The lessons to show with `skill`.
fn pick(all: Vec<Lesson>, skill: &str) -> Vec<Lesson> {
    let skill = skill.trim().to_lowercase();
    let mut pool: Vec<Lesson> = all.iter().filter(|l| l.skill == skill).cloned().collect();
    if skill != GENERAL {
        pool.extend(all.iter().filter(|l| l.skill == GENERAL).take(GENERAL_SHOW).cloned());
    }
    // Priority: confirmed (most seen, newest) before one-offs (newest).
    pool.sort_by(|a, b| worth(b).cmp(&worth(a)));
    let (mut chosen, mut chars) = (Vec::new(), 0);
    for l in pool {
        let cost = l.text.len() + 6;
        if chosen.len() >= SHOW_MAX || chars + cost > SHOW_CHARS {
            continue;
        }
        chars += cost;
        chosen.push(l);
    }
    newest_first(chosen)
}

/// Lessons as the text appended to a skill.
pub fn render(lessons: &[Lesson]) -> String {
    if lessons.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n## Lessons from earlier jobs (newest first; follow them, and confirm one that proves right again with `skill {do: \"confirm\", id}`)\n");
    for l in lessons {
        let tag = match l.kind {
            Kind::Worked => "",
            Kind::Fixed => "fixed: ",
            Kind::Correction => "the person said: ",
        };
        let seen = if l.seen > 1 { format!(" (seen {} times)", l.seen) } else { String::new() };
        s.push_str(&format!("- [{}] {tag}{}{seen}\n", l.id, l.text));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-lessons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    const T0: u64 = 1_800_000_000;

    #[test]
    fn a_lesson_is_stored_and_read_back() {
        let dir = scratch("store");
        let s = Store::at(&dir);
        let r = s.learn("story-film", "Frame the middle of every line before calling a scene done.", Kind::Worked, T0).unwrap();
        assert!(!r.confirmed && r.seen == 1);
        let all = Store::at(&dir).all(T0 + 5);
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].skill.as_str(), all[0].kind, all[0].seen), ("story-film", Kind::Worked, 1));
        // A file in the folder, and nothing half-written left behind.
        assert!(dir.join("lessons.json").is_file() && !dir.join("lessons.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_lesson_is_confirmed_not_stored_twice() {
        let dir = scratch("dedupe");
        let s = Store::at(&dir);
        let a = s.learn("sound-design", "Lay a quiet bed under every place, 20 dB below the voice.", Kind::Worked, T0).unwrap();
        // Same words, different case and spacing and a stop word.
        let b = s.learn("sound-design", "lay a quiet bed under every place,   20 dB below the voice", Kind::Worked, T0 + 10).unwrap();
        assert_eq!(a.id, b.id);
        assert!(b.confirmed && b.seen == 2);
        // Another skill is another lesson.
        let c = s.learn("music-beat-sync", "Lay a quiet bed under every place, 20 dB below the voice.", Kind::Worked, T0 + 20).unwrap();
        assert_ne!(a.id, c.id);
        // A different lesson of the same skill stays separate.
        let d = s.learn("sound-design", "Duck music 12 dB under dialogue and let it back up in the gaps.", Kind::Worked, T0 + 30).unwrap();
        assert!(!d.confirmed);
        assert_eq!(s.all(T0 + 40).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn near_duplicates_match_but_different_advice_does_not() {
        assert!(same("Run check after every edit and fix each note", "run check after every edit, then fix each note"));
        assert!(!same("Run check after every edit", "Never run check on a film without sound"));
        assert!(!same("Use scale 110 on punch-ins", "Use scale 110 on punch-ins and keep every title between eight and seventy-eight percent of the height"));
    }

    #[test]
    fn a_lesson_must_be_short_and_say_something() {
        let dir = scratch("limits");
        let s = Store::at(&dir);
        assert!(s.learn("general", "ok", Kind::Worked, T0).is_err());
        assert!(s.learn("general", &"word ".repeat(100), Kind::Worked, T0).is_err());
        assert!(s.all(T0).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_offs_age_out_but_confirmed_and_own_lessons_stay() {
        let dir = scratch("age");
        let s = Store::at(&dir);
        s.learn("colour-grading", "Match the shot's white balance before any look is applied.", Kind::Worked, T0).unwrap();
        let rep = s.learn("colour-grading", "Check the scopes after a LUT, not before it.", Kind::Fixed, T0).unwrap();
        s.learn("colour-grading", "Check the scopes after a LUT, not before it.", Kind::Fixed, T0 + 100).unwrap();
        let mine = s.learn("colour-grading", "Never push saturation past the skin tone line on faces.", Kind::Correction, T0).unwrap();
        s.edit(mine.id, "Never push saturation past the skin tone line on faces!", T0 + 5).unwrap();
        let later = T0 + (ONE_OFF_DAYS + 5) * DAY;
        let left = s.all(later);
        let ids: Vec<u64> = left.iter().map(|l| l.id).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(ids.contains(&rep.id) && ids.contains(&mine.id));
        // Ageing was saved: a fresh store sees the same.
        assert_eq!(Store::at(&dir).all(later).len(), 2);
        // A one-off confirmed in time does not age.
        let t = Store::at(&scratch("age2"));
        let l = t.learn("general", "Name every layer for what it shows.", Kind::Worked, T0).unwrap();
        t.confirm(l.id, T0 + 40 * DAY).unwrap();
        assert_eq!(t.all(T0 + 200 * DAY).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_is_capped_per_skill_and_keeps_the_confirmed() {
        let dir = scratch("cap");
        let s = Store::at(&dir);
        let keep = s.learn("vector-drawing", "Close every path before filling it.", Kind::Fixed, T0).unwrap();
        s.confirm(keep.id, T0 + 1).unwrap();
        for i in 0..(PER_SKILL + 10) {
            s.learn("vector-drawing", &format!("One-off lesson number {i} alpha{i} beta{i} gamma{i}"), Kind::Worked, T0 + 10 + i as u64).unwrap();
        }
        let all = s.all(T0 + 1000);
        assert_eq!(all.len(), PER_SKILL);
        assert!(all.iter().any(|l| l.id == keep.id), "the confirmed lesson stays");
        // The newest one-off stays, the oldest went.
        assert!(all.iter().any(|l| l.text.contains(&format!("alpha{}", PER_SKILL + 9))));
        assert!(!all.iter().any(|l| l.text.contains("alpha0 ")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skill_loads_its_lessons_newest_first_within_the_size_cap() {
        let dir = scratch("load");
        let s = Store::at(&dir);
        s.learn("export-delivery", "Export a short test range first when the master is long.", Kind::Worked, T0).unwrap();
        s.learn("export-delivery", "Loudness is measured on the export, so read it from the export reply.", Kind::Fixed, T0 + 100).unwrap();
        s.learn("general", "Report what you saw separately from what you set.", Kind::Correction, T0 + 50).unwrap();
        s.learn("story-film", "Unrelated lesson that belongs to another skill entirely.", Kind::Worked, T0 + 60).unwrap();
        let text = s.section("export-delivery", T0 + 200);
        assert!(text.contains("Lessons from earlier jobs"));
        let (a, b, c) = (text.find("Loudness").unwrap(), text.find("Report what").unwrap(), text.find("short test range").unwrap());
        assert!(a < b && b < c, "newest first: {text}");
        assert!(!text.contains("Unrelated"));
        assert!(text.contains("fixed: ") && text.contains("the person said: "));
        assert!(s.section("colour-grading", T0 + 200).contains("Report what"), "general lessons ride along");
        assert_eq!(Store::at(&scratch("empty")).section("story-film", T0), "");

        // Too many: the size cap holds and the confirmed stay over one-offs.
        let big = Store::at(&scratch("loadcap"));
        let sure = big.learn("sound-design", "The one lesson confirmed again and again over many jobs.", Kind::Worked, T0).unwrap();
        for _ in 0..3 {
            big.confirm(sure.id, T0 + 1).unwrap();
        }
        for i in 0..20 {
            big.learn("sound-design", &format!("Fresh one-off lesson text that fills the room a little, with its own words ua{i} ub{i} uc{i} ud{i} ue{i} uf{i} and a long tail of filler to take up space in the loaded skill, so a crowd of these cannot all fit at once"), Kind::Worked, T0 + 100 + i).unwrap();
        }
        let shown = big.for_skill("sound-design", T0 + 500);
        assert!(shown.len() <= SHOW_MAX);
        assert!(render(&shown).len() <= SHOW_CHARS + 400);
        assert!(shown.iter().any(|l| l.id == sure.id), "the confirmed lesson is shown over newer one-offs");
        assert!(shown.windows(2).all(|w| w[0].last >= w[1].last), "newest first");
    }

    #[test]
    fn the_person_edits_and_deletes_lessons() {
        let dir = scratch("edit");
        let s = Store::at(&dir);
        let a = s.learn("titles-typography", "Keep titles between eight and seventy-eight percent of the height.", Kind::Worked, T0).unwrap();
        let b = s.learn("titles-typography", "Two fonts at most, one accent colour over neutrals.", Kind::Worked, T0 + 1).unwrap();
        s.edit(a.id, "Keep vertical titles inside the safe area.", T0 + 2).unwrap();
        let all = s.all(T0 + 3);
        let a2 = all.iter().find(|l| l.id == a.id).unwrap();
        assert!(a2.mine && a2.text.starts_with("Keep vertical"));
        assert!(s.edit(a.id, "no", T0).is_err());
        s.forget(b.id).unwrap();
        assert_eq!(s.all(T0 + 4).len(), 1);
        assert!(s.forget(b.id).is_err());
        assert_eq!(s.clear(None).unwrap(), 1);
        assert!(s.all(T0 + 5).is_empty());
        // Ids are not reused.
        let c = s.learn("general", "A fresh lesson after clearing everything out.", Kind::Worked, T0 + 6).unwrap();
        assert!(c.id > b.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_treated_as_empty() {
        let dir = scratch("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lessons.json"), "{ not json").unwrap();
        let s = Store::at(&dir);
        assert!(s.all(T0).is_empty());
        assert!(s.learn("general", "Lessons can be saved again after a bad file.", Kind::Worked, T0).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
