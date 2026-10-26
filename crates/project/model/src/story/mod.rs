// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The story behind the cut: a screenplay or narration broken into beats,
//! scenes and shots, the places scenes happen in, and the cast (the
//! character bible) every shot stays true to.
//!
//! The story lives in the project ([`crate::Project::story`]) and changes
//! through [`crate::Edit::SetStory`], so it undoes with everything else.
//! A shot is tied to the timeline by the clips that show it
//! ([`Shot::clips`]); [`Story::sync`] reads their place back after the cut
//! is trimmed or moved so the shot list and the timeline agree.
//!
//! [`fountain`] reads and writes Fountain screenplays (and plain text);
//! [`check`](Story::check) finds what a director would send back: lines no
//! shot shows, scenes with no establishing shot, the same framing twice
//! in a row, crossing the line, eyelines that miss, shots all one length.

mod check;
mod edit;
pub mod fountain;
pub mod import;
mod plan;
pub mod times;

pub use check::{Issue, Severity};
pub use times::{LineTime, TimeSource};

use crate::id::ClipId;
use crate::sequence::Sequence;
use crate::time::{Micros, SECOND};
use crate::value::Extra;
use serde::{Deserialize, Serialize};

/// The whole production plan. Empty for a project that has none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Story {
    /// Title of the film.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The film in a sentence.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logline: String,
    /// The beats (story turns) in order; scenes name the beat they serve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub beats: Vec<Beat>,
    /// The scenes in story order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<Scene>,
    /// Where scenes happen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<Place>,
    /// The character bible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cast: Vec<Character>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// A turn of the story (set-up, the call, the low point, the climax…).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    /// Short id scenes refer to (`"setup"`, `"b1"`).
    pub id: String,
    /// What happens, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// What it does for the story or how it should feel.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub purpose: String,
    /// When the beat begins, from the start of the film, if said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Micros>,
    /// How long the beat runs, if said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Micros>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// One scene: a place, a time, who is there, what is said and the shots
/// that tell it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Scene number or id (`"1"`, `"4A"`).
    pub id: String,
    /// The slug line, e.g. `EXT. MEADOW - DAWN`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub heading: String,
    /// The beat this scene serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<String>,
    /// The place (a [`Place::id`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Time of day here, when it differs from the place's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<TimeOfDay>,
    /// Who is in the scene ([`Character::id`]s).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cast: Vec<String>,
    /// What happens, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// When the scene begins, from the start of the film, if the writer
    /// said (lines without a time of their own follow from here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Micros>,
    /// Notes on the scene for the crew (not part of the script).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The scene's text in order: action, dialogue, narration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<Line>,
    /// The shots, in cut order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shots: Vec<Shot>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// What kind of text a line is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// What we see happen (screen direction).
    #[default]
    Action,
    /// A character speaks on screen.
    Dialogue,
    /// The narrator (or a character's voice-over) over the pictures.
    Narration,
}

/// A line of the scene.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Action, dialogue or narration.
    #[serde(default)]
    pub kind: LineKind,
    /// Who speaks (a [`Character::id`], or `"narrator"`); none for action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// The words.
    pub text: String,
    /// How it is said (`whispering`, `to herself`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    /// When the line starts, from the start of the film, if it was said
    /// (typed `[0:12]`, read from SRT, set by hand or by the AI). Without
    /// it the line follows its shot on the timeline, or is estimated from
    /// reading speed ([`Story::line_times`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Micros>,
    /// How long the line runs, if said (else estimated from its words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Micros>,
    /// A note on the line for the crew (not spoken).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Line {
    /// Whether the line is heard (dialogue or narration) and so needs time.
    pub fn is_spoken(&self) -> bool {
        matches!(self.kind, LineKind::Dialogue | LineKind::Narration)
    }

    /// Seconds it takes to say, at an easy storytelling pace (2.6 words a
    /// second); action lines take none of their own.
    pub fn spoken_secs(&self) -> f64 {
        if !self.is_spoken() {
            return 0.0;
        }
        if let Some(d) = self.duration {
            return d as f64 / SECOND as f64;
        }
        let words = self.text.split_whitespace().count() as f64;
        words / 2.6 + 0.3
    }
}

/// Shot sizes and purposes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotKind {
    /// Where we are: the whole place, small figures (opens a scene).
    Establishing,
    /// Whole figures in their surroundings.
    Wide,
    /// From the waist (or the shoulders of an animal) up.
    #[default]
    Medium,
    /// A face fills the frame.
    CloseUp,
    /// Eyes only, or one small detail of a face.
    ExtremeCloseUp,
    /// A character listening, reacting to what was just said or done.
    Reaction,
    /// Past one character's shoulder onto the other.
    OverTheShoulder,
    /// A detail: a paw, a door, a letter.
    Insert,
    /// What a character sees, from their eyes.
    Pov,
    /// Two characters together.
    TwoShot,
}

impl ShotKind {
    /// Every kind, widest first.
    pub const ALL: [ShotKind; 10] = [
        ShotKind::Establishing,
        ShotKind::Wide,
        ShotKind::TwoShot,
        ShotKind::Medium,
        ShotKind::OverTheShoulder,
        ShotKind::CloseUp,
        ShotKind::ExtremeCloseUp,
        ShotKind::Reaction,
        ShotKind::Insert,
        ShotKind::Pov,
    ];

    /// The snake_case name used in files and by agents.
    pub fn name(self) -> &'static str {
        match self {
            ShotKind::Establishing => "establishing",
            ShotKind::Wide => "wide",
            ShotKind::Medium => "medium",
            ShotKind::CloseUp => "close_up",
            ShotKind::ExtremeCloseUp => "extreme_close_up",
            ShotKind::Reaction => "reaction",
            ShotKind::OverTheShoulder => "over_the_shoulder",
            ShotKind::Insert => "insert",
            ShotKind::Pov => "pov",
            ShotKind::TwoShot => "two_shot",
        }
    }

    /// The short form a shot list prints (`EST`, `WS`, `MS`, `CU`…).
    pub fn abbrev(self) -> &'static str {
        match self {
            ShotKind::Establishing => "EST",
            ShotKind::Wide => "WS",
            ShotKind::Medium => "MS",
            ShotKind::CloseUp => "CU",
            ShotKind::ExtremeCloseUp => "ECU",
            ShotKind::Reaction => "REACT",
            ShotKind::OverTheShoulder => "OTS",
            ShotKind::Insert => "INS",
            ShotKind::Pov => "POV",
            ShotKind::TwoShot => "2S",
        }
    }

    /// Reads a name, an abbreviation or a common way of saying it.
    pub fn parse(s: &str) -> Option<ShotKind> {
        let k = s.trim().to_lowercase().replace(['-', ' ', '.'], "_");
        Some(match k.as_str() {
            "establishing" | "est" | "establish" | "els" | "extreme_wide" | "master" => ShotKind::Establishing,
            "wide" | "ws" | "ls" | "long" | "full" | "fs" | "wide_shot" | "long_shot" | "full_shot" => ShotKind::Wide,
            "medium" | "ms" | "mid" | "medium_shot" | "mcu" | "medium_close_up" | "cowboy" => ShotKind::Medium,
            "close_up" | "closeup" | "cu" | "close" => ShotKind::CloseUp,
            "extreme_close_up" | "ecu" | "xcu" | "extreme_closeup" => ShotKind::ExtremeCloseUp,
            "reaction" | "react" | "reaction_shot" => ShotKind::Reaction,
            "over_the_shoulder" | "ots" | "o_s" | "over_shoulder" => ShotKind::OverTheShoulder,
            "insert" | "ins" | "detail" | "cutaway" => ShotKind::Insert,
            "pov" | "point_of_view" => ShotKind::Pov,
            "two_shot" | "2s" | "two" | "2_shot" => ShotKind::TwoShot,
            _ => return None,
        })
    }

    /// How tight the framing is: 0 for an establishing shot up to 5 for an
    /// extreme close-up. Inserts and POVs count as close.
    pub fn tightness(self) -> u8 {
        match self {
            ShotKind::Establishing => 0,
            ShotKind::Wide => 1,
            ShotKind::TwoShot => 2,
            ShotKind::Medium | ShotKind::OverTheShoulder | ShotKind::Pov => 3,
            ShotKind::CloseUp | ShotKind::Reaction | ShotKind::Insert => 4,
            ShotKind::ExtremeCloseUp => 5,
        }
    }
}

/// How the camera moves during the shot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMove {
    /// Locked off.
    #[default]
    Static,
    /// Turns left or right.
    Pan,
    /// Turns up or down.
    Tilt,
    /// Moves in toward the subject.
    PushIn,
    /// Moves back away from the subject.
    PullOut,
    /// Travels alongside a moving subject.
    Track,
    /// Rises or falls.
    Crane,
    /// Hand-held, alive and unsteady.
    Handheld,
    /// The lens zooms.
    Zoom,
}

impl CameraMove {
    /// Every move.
    pub const ALL: [CameraMove; 9] =
        [CameraMove::Static, CameraMove::Pan, CameraMove::Tilt, CameraMove::PushIn, CameraMove::PullOut, CameraMove::Track, CameraMove::Crane, CameraMove::Handheld, CameraMove::Zoom];

    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            CameraMove::Static => "static",
            CameraMove::Pan => "pan",
            CameraMove::Tilt => "tilt",
            CameraMove::PushIn => "push_in",
            CameraMove::PullOut => "pull_out",
            CameraMove::Track => "track",
            CameraMove::Crane => "crane",
            CameraMove::Handheld => "handheld",
            CameraMove::Zoom => "zoom",
        }
    }

    /// Reads a name or a common way of saying it.
    pub fn parse(s: &str) -> Option<CameraMove> {
        let k = s.trim().to_lowercase().replace(['-', ' '], "_");
        Some(match k.as_str() {
            "static" | "still" | "locked" | "locked_off" | "none" | "fixed" => CameraMove::Static,
            "pan" | "pan_left" | "pan_right" => CameraMove::Pan,
            "tilt" | "tilt_up" | "tilt_down" => CameraMove::Tilt,
            "push_in" | "push" | "dolly_in" | "in" | "slow_push" => CameraMove::PushIn,
            "pull_out" | "pull" | "dolly_out" | "out" | "pull_back" => CameraMove::PullOut,
            "track" | "tracking" | "truck" | "follow" | "dolly" => CameraMove::Track,
            "crane" | "jib" | "boom" | "rise" | "crane_up" | "crane_down" => CameraMove::Crane,
            "handheld" | "hand_held" | "shaky" => CameraMove::Handheld,
            "zoom" | "zoom_in" | "zoom_out" => CameraMove::Zoom,
            _ => return None,
        })
    }
}

/// Times of day, for light and ambience.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeOfDay {
    /// First light.
    Dawn,
    /// Morning.
    Morning,
    /// Full day.
    #[default]
    Day,
    /// Late, low sun.
    GoldenHour,
    /// Sunset into dusk.
    Dusk,
    /// After dark.
    Night,
}

impl TimeOfDay {
    /// Every time of day.
    pub const ALL: [TimeOfDay; 6] = [TimeOfDay::Dawn, TimeOfDay::Morning, TimeOfDay::Day, TimeOfDay::GoldenHour, TimeOfDay::Dusk, TimeOfDay::Night];

    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            TimeOfDay::Dawn => "dawn",
            TimeOfDay::Morning => "morning",
            TimeOfDay::Day => "day",
            TimeOfDay::GoldenHour => "golden_hour",
            TimeOfDay::Dusk => "dusk",
            TimeOfDay::Night => "night",
        }
    }

    /// Reads a name or what a slug line says (`DAY`, `SUNSET`, `LATER`…).
    pub fn parse(s: &str) -> Option<TimeOfDay> {
        let k = s.trim().to_lowercase().replace(['-', ' '], "_");
        Some(match k.as_str() {
            "dawn" | "sunrise" | "first_light" => TimeOfDay::Dawn,
            "morning" => TimeOfDay::Morning,
            "day" | "noon" | "afternoon" | "midday" | "daytime" => TimeOfDay::Day,
            "golden_hour" | "golden" | "late_afternoon" | "magic_hour" => TimeOfDay::GoldenHour,
            "dusk" | "sunset" | "evening" | "twilight" => TimeOfDay::Dusk,
            "night" | "midnight" | "nighttime" => TimeOfDay::Night,
            _ => return None,
        })
    }
}

/// Which way a character looks or faces on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenSide {
    /// Toward frame left.
    Left,
    /// Toward frame right.
    Right,
    /// Straight at (or away from) the camera.
    Camera,
}

impl ScreenSide {
    /// Reads `left`, `right`, `camera` (`l`, `r`, `lens`, `center`).
    pub fn parse(s: &str) -> Option<ScreenSide> {
        Some(match s.trim().to_lowercase().as_str() {
            "left" | "l" | "frame_left" | "screen_left" => ScreenSide::Left,
            "right" | "r" | "frame_right" | "screen_right" => ScreenSide::Right,
            "camera" | "lens" | "center" | "centre" | "front" => ScreenSide::Camera,
            _ => return None,
        })
    }

    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            ScreenSide::Left => "left",
            ScreenSide::Right => "right",
            ScreenSide::Camera => "camera",
        }
    }
}

/// One shot of a scene.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    /// Shot id within the film (`"1A"`, `"4C"`).
    pub id: String,
    /// Size and purpose.
    #[serde(default)]
    pub kind: ShotKind,
    /// Who or what is in frame, in order of importance ([`Character::id`]s
    /// or plain words for a thing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<String>,
    /// Framing beyond the size: angle and composition (`low angle, fox
    /// frame left, the hill behind`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub framing: String,
    /// How the camera moves.
    #[serde(default)]
    pub camera: CameraMove,
    /// How long the shot runs.
    pub duration: Micros,
    /// The scene's lines this shot plays under (indices into
    /// [`Scene::lines`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<usize>,
    /// What happens in the shot (the acting, the business).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
    /// Which side of the line of action the camera stands on, `"a"` or
    /// `"b"`, for the 180° rule. A shot that moves across the line says so
    /// with `crosses`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// The camera crosses the line in view during this shot (a move the
    /// audience follows), so the next shot may take the other side.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crosses: bool,
    /// Which way the main subject looks on screen (eyelines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looks: Option<ScreenSide>,
    /// Timeline start, once the shot is built (kept by [`Story::sync`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Micros>,
    /// The clips that show this shot on the timeline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipId>,
    /// Anything else for the crew.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Where a scene happens.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// Short id scenes refer to (`"meadow"`).
    pub id: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Inside (INT.) rather than outside (EXT.).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interior: bool,
    /// The backdrop: a media name or file in the project, or words to make
    /// one from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop: Option<String>,
    /// The sound of the place: an ambience bed (`stream`, `forest`,
    /// `night`…) or a library sound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambience: Option<String>,
    /// The light: direction, quality, colour (`low warm sun from the left`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub light: String,
    /// The usual time of day here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day: Option<TimeOfDay>,
    /// What the ground is (`grass`, `gravel`, `wood`, `snow`, `water`…):
    /// footsteps are matched to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    /// Anything else (what is in it, where things stand).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// A character in the bible: how they look, sound, move and behave, so
/// every shot keeps them the same.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Character {
    /// Short id lines and shots refer to (`"fox"`).
    pub id: String,
    /// Display name (`"Fern the Fox"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// What they look like, in words precise enough to draw from again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub look: String,
    /// Reference pictures: media names or files of this character.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    /// The cut-out or cast-library item used for them on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast_item: Option<String>,
    /// The text-to-speech voice id they speak with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Height relative to the others (1.0 = the reference character).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub size: f64,
    /// Who they are: temperament, wants, fears.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub personality: String,
    /// How they move: the puppet acts that suit them (`hop`, `sneak`…),
    /// their usual gait first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moves: Vec<String>,
    /// Which way they face when first seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<ScreenSide>,
    /// Anything else.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    (*v - 1.0).abs() < 1e-9
}

/// Where a shot sits in the planned cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShotTime {
    /// Index into [`Story::scenes`].
    pub scene: usize,
    /// Index into that scene's shots.
    pub shot: usize,
    /// Planned start on the timeline.
    pub start: Micros,
    /// Planned length.
    pub duration: Micros,
}

impl Story {
    /// Whether there is nothing in it.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.logline.is_empty() && self.beats.is_empty() && self.scenes.is_empty() && self.places.is_empty() && self.cast.is_empty() && self.extra.is_empty()
    }

    /// A scene by id.
    pub fn scene(&self, id: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id.eq_ignore_ascii_case(id))
    }

    /// A place by id or name.
    pub fn place(&self, id: &str) -> Option<&Place> {
        self.places.iter().find(|p| p.id.eq_ignore_ascii_case(id) || (!p.name.is_empty() && p.name.eq_ignore_ascii_case(id)))
    }

    /// A character by id or name.
    pub fn character(&self, id: &str) -> Option<&Character> {
        self.cast.iter().find(|c| c.id.eq_ignore_ascii_case(id) || (!c.name.is_empty() && c.name.eq_ignore_ascii_case(id)))
    }

    /// The scene and shot index of a shot id.
    pub fn find_shot(&self, id: &str) -> Option<(usize, usize)> {
        self.scenes.iter().enumerate().find_map(|(si, s)| s.shots.iter().position(|sh| sh.id.eq_ignore_ascii_case(id)).map(|i| (si, i)))
    }

    /// Every shot, in cut order, with where it falls: its built start when
    /// it has one, else right after the shot before.
    pub fn timings(&self) -> Vec<ShotTime> {
        let mut out = Vec::new();
        let mut t = 0;
        for (si, s) in self.scenes.iter().enumerate() {
            for (i, sh) in s.shots.iter().enumerate() {
                let start = sh.start.unwrap_or(t);
                out.push(ShotTime { scene: si, shot: i, start, duration: sh.duration });
                t = start + sh.duration;
            }
        }
        out
    }

    /// Planned running time.
    pub fn duration(&self) -> Micros {
        self.timings().last().map(|t| t.start + t.duration).unwrap_or(0)
    }

    /// The place a scene happens in, if it names one the story has.
    pub fn place_of(&self, scene: &Scene) -> Option<&Place> {
        scene.place.as_deref().and_then(|p| self.place(p))
    }

    /// The scene and shot showing timeline time `t`.
    pub fn at(&self, t: Micros) -> Option<ShotTime> {
        self.timings().into_iter().find(|x| x.start <= t && t < x.start + x.duration)
    }

    /// A shot id not used yet in scene `scene` (`"3A"`, `"3B"`…).
    pub fn next_shot_id(&self, scene: usize) -> String {
        let s = &self.scenes[scene];
        let base = if s.id.is_empty() { (scene + 1).to_string() } else { s.id.clone() };
        for n in 0.. {
            let letters = if n < 26 { ((b'A' + n as u8) as char).to_string() } else { format!("{}{}", (b'A' + (n / 26 - 1) as u8) as char, (b'A' + (n % 26) as u8) as char) };
            let id = format!("{base}{letters}");
            if self.find_shot(&id).is_none() {
                return id;
            }
        }
        unreachable!()
    }

    /// Reads where the built shots now sit on `seq`: each shot's start and
    /// length follow its main-lane clips (or, with none there, all of its
    /// clips), the earliest start to the latest end, and clips that are
    /// gone are dropped. Returns the shots that changed.
    pub fn sync(&mut self, seq: &Sequence) -> Vec<String> {
        let mut changed = Vec::new();
        for s in &mut self.scenes {
            for sh in &mut s.shots {
                let before = (sh.start, sh.duration, sh.clips.len());
                sh.clips.retain(|c| seq.clip(*c).is_some());
                // The main-lane picture sets the shot's time; overlays
                // (a title, a character) only when it has none.
                let clips: Vec<&crate::clip::Clip> = sh.clips.iter().filter_map(|c| seq.clip(*c)).collect();
                let main: Vec<&&crate::clip::Clip> = clips.iter().filter(|c| seq.lane(c.lane).is_some_and(|l| l.kind == crate::sequence::LaneKind::Spine)).collect();
                let spans: Vec<(Micros, Micros)> = if main.is_empty() { clips.iter().map(|c| (c.start, c.end())).collect() } else { main.iter().map(|c| (c.start, c.end())).collect() };
                if let (Some(a), Some(b)) = (spans.iter().map(|s| s.0).min(), spans.iter().map(|s| s.1).max()) {
                    sh.start = Some(a);
                    sh.duration = b - a;
                } else if before.2 > 0 {
                    // Every clip was deleted: back to a plan.
                    sh.start = None;
                }
                if (sh.start, sh.duration, sh.clips.len()) != before {
                    changed.push(sh.id.clone());
                }
            }
        }
        changed
    }

    /// Seconds the spoken lines of `shot` need.
    pub fn spoken_secs(scene: &Scene, shot: &Shot) -> f64 {
        shot.lines.iter().filter_map(|i| scene.lines.get(*i)).map(Line::spoken_secs).sum()
    }

    /// The shot list as plain text, one line a shot under each scene.
    pub fn shot_list(&self) -> String {
        let mut out = String::new();
        if !self.title.is_empty() {
            out.push_str(&format!("{}\n", self.title));
        }
        let times = self.timings();
        for (si, s) in self.scenes.iter().enumerate() {
            let place = self.place_of(s).map(|p| if p.name.is_empty() { p.id.clone() } else { p.name.clone() }).unwrap_or_default();
            out.push_str(&format!("\nSCENE {} {}{}\n", s.id, if s.heading.is_empty() { &place } else { &s.heading }, s.beat.as_ref().map(|b| format!("  [{b}]")).unwrap_or_default()));
            for (i, sh) in s.shots.iter().enumerate() {
                let t = times.iter().find(|t| t.scene == si && t.shot == i).map(|t| t.start).unwrap_or(0);
                let lines = if sh.lines.is_empty() { String::new() } else { format!(" lines {}", sh.lines.iter().map(|l| (l + 1).to_string()).collect::<Vec<_>>().join(",")) };
                out.push_str(&format!(
                    "  {:<5} {:>7.2}s {:>5.2}s  {:<5} {:<8} {}{}{}\n",
                    sh.id,
                    t as f64 / SECOND as f64,
                    sh.duration as f64 / SECOND as f64,
                    sh.kind.abbrev(),
                    sh.camera.name(),
                    sh.subjects.join("+"),
                    if sh.action.is_empty() { String::new() } else { format!(": {}", sh.action) },
                    lines
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Clip;
    use crate::id::LaneId;

    fn shot(id: &str, kind: ShotKind, secs: i64) -> Shot {
        Shot { id: id.into(), kind, duration: secs * SECOND, ..Default::default() }
    }

    #[test]
    fn shots_follow_one_another_and_the_cut() {
        let mut st = Story::default();
        st.scenes.push(Scene { id: "1".into(), shots: vec![shot("1A", ShotKind::Establishing, 4), shot("1B", ShotKind::Medium, 3)], ..Default::default() });
        st.scenes.push(Scene { id: "2".into(), shots: vec![shot("2A", ShotKind::Wide, 5)], ..Default::default() });
        let t = st.timings();
        assert_eq!(t.iter().map(|x| x.start / SECOND).collect::<Vec<_>>(), vec![0, 4, 7]);
        assert_eq!(st.duration(), 12 * SECOND);
        assert_eq!(st.at(5 * SECOND).map(|x| (x.scene, x.shot)), Some((0, 1)));
        assert_eq!(st.next_shot_id(0), "1C");

        // Built: 1B's clip is trimmed and moved; 2A's clip is deleted.
        let mut seq = Sequence::new(crate::id::SequenceId(1), "s", LaneId(2));
        let mut c = Clip::new(ClipId(10), LaneId(2), 4 * SECOND, 2 * SECOND, crate::clip::ClipSource::Gap);
        c.start = 4 * SECOND;
        seq.clips.push(c);
        st.scenes[0].shots[1].clips = vec![ClipId(10)];
        st.scenes[1].shots[0].clips = vec![ClipId(11)];
        st.scenes[1].shots[0].start = Some(6 * SECOND);
        let changed = st.sync(&seq);
        assert_eq!(changed, vec!["1B".to_owned(), "2A".to_owned()]);
        assert_eq!(st.scenes[0].shots[1].duration, 2 * SECOND);
        assert_eq!(st.scenes[1].shots[0].start, None);
        assert_eq!(st.timings()[2].start, 6 * SECOND);
    }

    #[test]
    fn the_story_is_saved_with_the_project_and_undoes() {
        let mut p = crate::Project::default();
        let mut h = crate::History::new();
        let mut st = Story { title: "Fern".into(), ..Default::default() };
        st.cast.push(Character { id: "fern".into(), size: 0.8, ..Default::default() });
        h.apply(&mut p, "Story", crate::Edit::SetStory { story: Box::new(st.clone()) }).unwrap();
        assert_eq!(p.story, st);
        let json = serde_json::to_string(&p).unwrap();
        let back: crate::Project = serde_json::from_str(&json).unwrap();
        assert!(back.content_eq(&p));
        h.undo(&mut p).unwrap();
        assert!(p.story.is_empty());
        assert!(!serde_json::to_string(&p).unwrap().contains("\"story\""));
    }

    #[test]
    fn names_are_read_loosely() {
        assert_eq!(ShotKind::parse("Close-up"), Some(ShotKind::CloseUp));
        assert_eq!(ShotKind::parse("OTS"), Some(ShotKind::OverTheShoulder));
        assert_eq!(CameraMove::parse("dolly in"), Some(CameraMove::PushIn));
        assert_eq!(TimeOfDay::parse("SUNSET"), Some(TimeOfDay::Dusk));
        for k in ShotKind::ALL {
            assert_eq!(ShotKind::parse(k.name()), Some(k));
            assert_eq!(ShotKind::parse(k.abbrev()), Some(k), "{k:?}");
        }
        for m in CameraMove::ALL {
            assert_eq!(CameraMove::parse(m.name()), Some(m));
        }
    }

    #[test]
    fn an_empty_story_is_not_written() {
        let st = Story::default();
        assert!(st.is_empty());
        let mut c = Character { id: "fox".into(), size: 1.0, ..Default::default() };
        assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({"id": "fox"}));
        c.size = 0.5;
        let back: Character = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let plain: Character = serde_json::from_str(r#"{"id":"owl"}"#).unwrap();
        assert_eq!(plain.size, 1.0);
    }
}
