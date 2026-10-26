// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The production office's view of the story: just the parts of the
//! project's script, scenes and shots that breakdowns, schedules and
//! budgets read, with the same JSON shape as the project's own story, so
//! `serde_json::from_value(serde_json::to_value(project_story))` turns the
//! project's story into this one. Nothing here is ever written back.

use serde::{Deserialize, Serialize};

/// Microseconds, as in the project model.
pub type Micros = i64;
/// One second.
pub const SECOND: Micros = 1_000_000;

/// A clip on the timeline (its project id).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipId(pub u64);

/// The film's plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Story {
    /// Title.
    #[serde(default)]
    pub title: String,
    /// Scenes in story order.
    #[serde(default)]
    pub scenes: Vec<Scene>,
    /// Places.
    #[serde(default)]
    pub places: Vec<Place>,
    /// The character bible.
    #[serde(default)]
    pub cast: Vec<Character>,
}

/// Times of day.
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
    /// Low sun.
    GoldenHour,
    /// Dusk.
    Dusk,
    /// After dark.
    Night,
}

impl TimeOfDay {
    /// Reads a name or what a slug line says.
    pub fn parse(s: &str) -> Option<TimeOfDay> {
        Some(match s.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
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

/// A scene.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Scene id.
    #[serde(default)]
    pub id: String,
    /// Slug line.
    #[serde(default)]
    pub heading: String,
    /// The place id.
    #[serde(default)]
    pub place: Option<String>,
    /// Time of day.
    #[serde(default)]
    pub time_of_day: Option<TimeOfDay>,
    /// Character ids.
    #[serde(default)]
    pub cast: Vec<String>,
    /// One line of what happens.
    #[serde(default)]
    pub summary: String,
    /// Action, dialogue, narration.
    #[serde(default)]
    pub lines: Vec<Line>,
    /// Shots in cut order.
    #[serde(default)]
    pub shots: Vec<Shot>,
}

/// Kinds of line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// What we see.
    #[default]
    Action,
    /// Spoken on screen.
    Dialogue,
    /// Narration.
    Narration,
}

/// A line of a scene.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Kind.
    #[serde(default)]
    pub kind: LineKind,
    /// Who speaks.
    #[serde(default)]
    pub who: Option<String>,
    /// The words.
    #[serde(default)]
    pub text: String,
}

impl Line {
    /// Dialogue or narration.
    pub fn is_spoken(&self) -> bool {
        matches!(self.kind, LineKind::Dialogue | LineKind::Narration)
    }

    /// Seconds it takes to say (2.6 words a second); none for action.
    pub fn spoken_secs(&self) -> f64 {
        if !self.is_spoken() {
            return 0.0;
        }
        self.text.split_whitespace().count() as f64 / 2.6 + 0.3
    }
}

/// How the camera moves (the schedule only needs to know whether it does).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMove {
    /// Locked off.
    #[default]
    Static,
    /// Any move.
    #[serde(other)]
    Moving,
}

/// A shot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    /// Shot id.
    #[serde(default)]
    pub id: String,
    /// Who or what is in frame.
    #[serde(default)]
    pub subjects: Vec<String>,
    /// Framing.
    #[serde(default)]
    pub framing: String,
    /// Camera move.
    #[serde(default)]
    pub camera: CameraMove,
    /// Length.
    #[serde(default)]
    pub duration: Micros,
    /// What happens.
    #[serde(default)]
    pub action: String,
    /// Notes.
    #[serde(default)]
    pub notes: String,
    /// Timeline start once built.
    #[serde(default)]
    pub start: Option<Micros>,
    /// Clips that show it.
    #[serde(default)]
    pub clips: Vec<ClipId>,
}

/// A place.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// Id.
    #[serde(default)]
    pub id: String,
    /// Name.
    #[serde(default)]
    pub name: String,
    /// Inside.
    #[serde(default)]
    pub interior: bool,
    /// Usual time of day.
    #[serde(default)]
    pub time_of_day: Option<TimeOfDay>,
    /// Notes (address, access).
    #[serde(default)]
    pub notes: String,
}

/// A character of the bible.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Character {
    /// Id.
    #[serde(default)]
    pub id: String,
    /// Name.
    #[serde(default)]
    pub name: String,
}

impl Story {
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

    /// The place a scene happens in.
    pub fn place_of(&self, scene: &Scene) -> Option<&Place> {
        scene.place.as_deref().and_then(|p| self.place(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_projects_story_json_and_ignores_the_rest() {
        let j = r#"{"title":"T","logline":"x","beats":[{"id":"b"}],"scenes":[{"id":"1","heading":"EXT. A - DAY","time_of_day":"golden_hour","lines":[{"kind":"dialogue","who":"fern","text":"hi there","how":"softly"}],"shots":[{"id":"1A","kind":"close_up","camera":"push_in","duration":2500000,"clips":[7],"side":"a"}]}],"cast":[{"id":"fern","look":"red"}]}"#;
        let s: Story = serde_json::from_str(j).unwrap();
        assert_eq!(s.scenes[0].time_of_day, Some(TimeOfDay::GoldenHour));
        assert_eq!(s.scenes[0].shots[0].camera, CameraMove::Moving);
        assert_eq!(s.scenes[0].shots[0].clips, vec![ClipId(7)]);
        assert!(s.scenes[0].lines[0].is_spoken());
        assert_eq!(s.find_shot("1a"), Some((0, 0)));
        assert!(s.character("FERN").is_some());
    }
}
