// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pipeline schedule for animated (or generated) work: every shot goes
//! through layout, animation, lighting, comp and sound, each department has
//! so many artists, and the schedule says when each shot clears each stage
//! and when the film is done.

use crate::Production;
use crate::breakdown::{Category, facts};
use crate::date::Date;
use serde::{Deserialize, Serialize};
use crate::story::SECOND;
use crate::story::{CameraMove, Story};

/// One department of the pipeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    /// Short key, also the task step (`layout`).
    pub key: String,
    /// Artists working in parallel.
    #[serde(default = "one")]
    pub artists: u32,
    /// Days a shot always takes here.
    pub base_days: f64,
    /// Further days for each second of shot.
    pub days_per_second: f64,
    /// Stages that must be finished first (keys).
    #[serde(default)]
    pub after: Vec<String>,
    /// Harder shots (many subjects, camera moves, effects) take longer.
    #[serde(default)]
    pub scales: bool,
}

fn one() -> u32 {
    1
}

/// The stages and who does them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// In order; a stage's `after` may only name earlier ones.
    pub stages: Vec<Stage>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig::preset("animation").unwrap()
    }
}

impl PipelineConfig {
    /// `animation` (layout, animation, lighting, comp, sound) or `ai`
    /// (boards, generate, review, edit, sound).
    pub fn preset(name: &str) -> Option<PipelineConfig> {
        let st = |key: &str, artists, base, per_s, after: &[&str], scales| Stage { key: key.into(), artists, base_days: base, days_per_second: per_s, after: after.iter().map(|s| s.to_string()).collect(), scales };
        Some(PipelineConfig {
            stages: match name {
                "animation" => vec![
                    st("layout", 2, 0.5, 0.1, &[], false),
                    st("animation", 3, 1.0, 0.5, &["layout"], true),
                    st("lighting", 2, 0.5, 0.2, &["animation"], true),
                    st("comp", 2, 0.5, 0.2, &["lighting"], true),
                    st("sound", 1, 0.5, 0.1, &["animation"], false),
                ],
                "ai" => vec![
                    st("boards", 1, 0.25, 0.05, &[], false),
                    st("generate", 2, 0.25, 0.1, &["boards"], true),
                    st("review", 1, 0.25, 0.05, &["generate"], false),
                    st("edit", 1, 0.25, 0.05, &["review"], false),
                    st("sound", 1, 0.25, 0.05, &["review"], false),
                ],
                _ => return None,
            },
        })
    }
}

/// When one shot passes one stage.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// Stage key.
    pub stage: String,
    /// Working day it starts (0-based, fractional).
    pub start: f64,
    /// Working day it ends.
    pub end: f64,
    /// Which artist lane (0-based).
    pub artist: u32,
}

/// One shot's way through the pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotPlan {
    /// Shot id.
    pub shot: String,
    /// Scene id.
    pub scene: String,
    /// Difficulty multiplier from subjects, camera and effects.
    pub complexity: f64,
    /// Each stage.
    pub spans: Vec<Span>,
    /// When it is finished (working days from the start).
    pub finish: f64,
}

/// The whole pipeline plan.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelinePlan {
    /// Per shot, in cut order.
    pub shots: Vec<ShotPlan>,
    /// Working days to finish everything.
    pub total_days: f64,
    /// The date of the last day, when there is a start date.
    pub end_date: Option<String>,
    /// The stage with most queueing: where more artists would help.
    pub bottleneck: Option<String>,
    /// Working days of the stage's artists spent waiting for work, per stage.
    pub idle: Vec<(String, f64)>,
}

fn half_days(d: f64) -> f64 {
    (d * 2.0).ceil().max(1.0) / 2.0
}

impl Production {
    /// Plans every shot of the story through the pipeline.
    pub fn pipeline_plan(&self, story: &Story) -> Result<PipelinePlan, String> {
        let cfg = &self.pipeline;
        for (i, s) in cfg.stages.iter().enumerate() {
            for a in &s.after {
                if !cfg.stages[..i].iter().any(|p| p.key == *a) {
                    return Err(format!("stage {} comes after {a}, which is not an earlier stage", s.key));
                }
            }
        }
        let fx = facts(story);
        let mut shots: Vec<ShotPlan> = Vec::new();
        let mut order: Vec<(usize, usize)> = Vec::new();
        for (si, sc) in story.scenes.iter().enumerate() {
            for (hi, _) in sc.shots.iter().enumerate() {
                order.push((si, hi));
            }
        }
        // Per stage, when each artist lane is next free.
        let mut free: Vec<Vec<f64>> = cfg.stages.iter().map(|s| vec![0.0; s.artists.max(1) as usize]).collect();
        let mut idle = vec![0.0f64; cfg.stages.len()];
        let mut waiting = vec![0.0f64; cfg.stages.len()];
        for (si, hi) in order {
            let sc = &story.scenes[si];
            let sh = &sc.shots[hi];
            let effects = self.in_scene(&sc.id).iter().filter(|(e, _)| matches!(e.category, Category::Vfx | Category::Sfx | Category::Stunts)).count() as f64;
            let secs = sh.duration as f64 / SECOND as f64;
            let _ = &fx;
            let complexity = 1.0 + 0.15 * (sh.subjects.len().saturating_sub(1)) as f64 + if sh.camera == CameraMove::Static { 0.0 } else { 0.15 } + 0.1 * effects.min(5.0);
            let mut spans: Vec<Span> = Vec::new();
            for (k, stage) in cfg.stages.iter().enumerate() {
                let ready = stage.after.iter().filter_map(|a| spans.iter().find(|s| s.stage == *a)).map(|s| s.end).fold(0.0, f64::max);
                let dur = half_days((stage.base_days + stage.days_per_second * secs) * if stage.scales { complexity } else { 1.0 });
                // The artist who frees up first takes the shot.
                let (lane, lane_free) = free[k].iter().copied().enumerate().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
                let start = ready.max(lane_free);
                if lane_free > ready {
                    waiting[k] += lane_free - ready;
                } else {
                    idle[k] += ready - lane_free;
                }
                free[k][lane] = start + dur;
                spans.push(Span { stage: stage.key.clone(), start, end: start + dur, artist: lane as u32 });
            }
            let finish = spans.iter().map(|s| s.end).fold(0.0, f64::max);
            shots.push(ShotPlan { shot: sh.id.clone(), scene: sc.id.clone(), complexity, spans, finish });
        }
        let total = shots.iter().map(|s| s.finish).fold(0.0, f64::max);
        let bottleneck = waiting.iter().enumerate().filter(|(_, w)| **w > 0.0).max_by(|a, b| a.1.total_cmp(b.1)).map(|(k, _)| cfg.stages[k].key.clone());
        let end_date = self.settings.start_date.as_deref().and_then(Date::parse).map(|d| self.settings.working_day(d, (total.ceil() as usize).saturating_sub(1)).to_string());
        let idle = cfg.stages.iter().zip(&idle).map(|(s, i)| (s.key.clone(), *i)).collect();
        Ok(PipelinePlan { shots, total_days: total, end_date, bottleneck, idle })
    }

    /// The calendar date a (fractional) working day falls on.
    pub fn pipeline_date(&self, day: f64) -> Option<String> {
        let d = self.settings.start_date.as_deref().and_then(Date::parse)?;
        Some(self.settings.working_day(d, day.floor().max(0.0) as usize).to_string())
    }

    /// Selects a preset or sets a stage's artists.
    pub fn set_pipeline(&mut self, preset: Option<&str>, artists: &[(String, u32)]) -> Result<(), String> {
        if let Some(p) = preset {
            self.pipeline = PipelineConfig::preset(p).ok_or_else(|| format!("no pipeline preset \"{p}\" (animation, ai)"))?;
        }
        for (k, n) in artists {
            let s = self.pipeline.stages.iter_mut().find(|s| s.key.eq_ignore_ascii_case(k)).ok_or_else(|| format!("no stage \"{k}\""))?;
            s.artists = (*n).max(1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{Scene, Shot};

    fn story(n: usize, secs: i64) -> Story {
        let mut st = Story::default();
        st.scenes.push(Scene { id: "1".into(), shots: (0..n).map(|i| Shot { id: format!("1{}", (b'A' + i as u8) as char), duration: secs * SECOND, ..Default::default() }).collect(), ..Default::default() });
        st
    }

    #[test]
    fn every_shot_clears_the_stages_in_order() {
        let st = story(4, 4);
        let mut p = Production::default();
        p.settings.start_date = Some("2026-11-02".into());
        let plan = p.pipeline_plan(&st).unwrap();
        assert_eq!(plan.shots.len(), 4);
        for s in &plan.shots {
            let at = |k: &str| s.spans.iter().find(|x| x.stage == k).unwrap();
            assert!(at("animation").start >= at("layout").end);
            assert!(at("lighting").start >= at("animation").end);
            assert!(at("comp").start >= at("lighting").end);
            assert!(at("sound").start >= at("animation").end, "sound follows picture lock of animation");
        }
        assert!(plan.total_days > 0.0);
        assert!(plan.end_date.is_some());
        assert!(plan.bottleneck.is_some(), "four shots through two layout artists queue somewhere");
    }

    #[test]
    fn more_artists_finish_sooner() {
        let st = story(8, 5);
        let mut p = Production::default();
        let slow = p.pipeline_plan(&st).unwrap().total_days;
        p.set_pipeline(None, &[("animation".into(), 8), ("layout".into(), 4), ("lighting".into(), 4), ("comp".into(), 4)]).unwrap();
        let fast = p.pipeline_plan(&st).unwrap().total_days;
        assert!(fast < slow, "{fast} < {slow}");
        assert!(p.set_pipeline(None, &[("nope".into(), 1)]).is_err());
        assert!(p.set_pipeline(Some("ai"), &[]).is_ok());
        assert_eq!(p.pipeline.stages[1].key, "generate");
    }

    #[test]
    fn a_bad_dependency_is_refused() {
        let mut p = Production::default();
        p.pipeline.stages[0].after = vec!["comp".into()];
        assert!(p.pipeline_plan(&story(1, 2)).unwrap_err().contains("not an earlier stage"));
    }
}
