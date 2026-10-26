// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test fixture: a small project built through edits.
#![allow(dead_code)]

use studio_model::*;

pub const S: Micros = SECOND;

pub struct Fx {
    pub p: Project,
    pub seq: SequenceId,
    pub spine: LaneId,
    pub v1: LaneId,
    pub a1: LaneId,
    /// 10 s video with sound.
    pub video: MediaId,
    /// 20 s sound.
    pub music: MediaId,
    /// A still image.
    pub still: MediaId,
    /// Spine: a (video 1–4 s), b (video 5–7 s), c (still, 2 s).
    pub a: ClipId,
    pub b: ClipId,
    pub c: ClipId,
    /// Title on v1 at 1 s for 2 s.
    pub title: ClipId,
    /// Music on a1 at 0 for 5 s.
    pub song: ClipId,
    /// Dissolve between a and b.
    pub dissolve: TransitionId,
}

pub fn video_info(duration: Micros) -> MediaInfo {
    MediaInfo {
        kind: MediaKind::Video,
        container: "mp4".into(),
        duration: Some(duration),
        video: Some(VideoInfo { codec: "h264".into(), width: 1920, height: 1080, frame_rate: Some(FrameRate::FPS_30), ..Default::default() }),
        audio: Some(AudioInfo { codec: "aac".into(), sample_rate: 48_000, channels: 2, layout: "stereo".into() }),
        extra: Default::default(),
    }
}

pub fn audio_info(duration: Micros) -> MediaInfo {
    MediaInfo { kind: MediaKind::Audio, container: "wav".into(), duration: Some(duration), audio: Some(AudioInfo { codec: "pcm_s16le".into(), sample_rate: 48_000, channels: 2, layout: "stereo".into() }), ..Default::default() }
}

pub fn image_info() -> MediaInfo {
    MediaInfo { kind: MediaKind::Image, container: "png".into(), video: Some(VideoInfo { codec: "png".into(), width: 800, height: 600, ..Default::default() }), ..Default::default() }
}

fn add_media(p: &mut Project, path: &str, info: MediaInfo) -> MediaId {
    let id = MediaId(p.alloc_id());
    p.apply(Edit::InsertMedia { item: Box::new(MediaItem::new(id, path, info)), index: None }).unwrap();
    id
}

fn append(p: &mut Project, seq: SequenceId, lane: Option<LaneId>, mut clip: Clip) -> ClipId {
    let id = ClipId(p.alloc_id());
    clip.id = id;
    p.apply(Edit::Append { seq, lane, clip: Box::new(clip) }).unwrap();
    id
}

pub fn fixture() -> Fx {
    let mut p = Project::default();
    let seq = p.active_sequence;
    let spine = p.active().spine_id();
    let video = add_media(&mut p, "/media/beach.mp4", video_info(10 * S));
    let music = add_media(&mut p, "/media/song.wav", audio_info(20 * S));
    let still = add_media(&mut p, "/media/logo.png", image_info());

    let v1 = LaneId(p.alloc_id());
    p.apply(Edit::AddLane { seq, lane: Lane::new(v1, LaneKind::Visual, "Titles"), index: None }).unwrap();
    let a1 = LaneId(p.alloc_id());
    p.apply(Edit::AddLane { seq, lane: Lane::new(a1, LaneKind::Audio, "Music"), index: None }).unwrap();

    let mut ca = Clip::media(ClipId(0), spine, 0, video, S, 3 * S);
    ca.name = "a".into();
    let a = append(&mut p, seq, None, ca);
    let mut cb = Clip::media(ClipId(0), spine, 0, video, 5 * S, 2 * S);
    cb.name = "b".into();
    let b = append(&mut p, seq, None, cb);
    let mut cc = Clip::media(ClipId(0), spine, 0, still, 0, 2 * S);
    cc.name = "c".into();
    let c = append(&mut p, seq, None, cc);

    let title = ClipId(p.alloc_id());
    let mut t = Clip::new(title, v1, 0, 2 * S, ClipSource::Generator(Generator::text("Hello")));
    t.name = "title".into();
    p.apply(Edit::Overwrite { seq, lane: Some(v1), at: S, clip: Box::new(t) }).unwrap();
    let song = append(&mut p, seq, Some(a1), Clip::media(ClipId(0), a1, 0, music, 0, 5 * S));

    let dissolve = TransitionId(p.alloc_id());
    p.apply(Edit::AddTransition {
        seq,
        transition: Transition { id: dissolve, left: a, right: ClipId(0), kind: "dissolve".into(), duration: S / 2, align: TransitionAlign::Centre, params: Default::default(), extra: Default::default() },
    })
    .unwrap();
    p.check_invariants().unwrap();
    Fx { p, seq, spine, v1, a1, video, music, still, a, b, c, title, song, dissolve }
}

impl Fx {
    pub fn clip(&self, id: ClipId) -> &Clip {
        self.p.sequence(self.seq).unwrap().clip(id).unwrap_or_else(|| panic!("clip {id} missing"))
    }

    pub fn seq(&self) -> &Sequence {
        self.p.sequence(self.seq).unwrap()
    }

    /// `(name, start, duration)` of spine clips, gaps shown as `"gap"`.
    pub fn spine_layout(&self) -> Vec<(String, Micros, Micros)> {
        self.seq().spine_clips().iter().map(|c| (if c.is_gap() { "gap".into() } else { c.name.clone() }, c.start, c.duration)).collect()
    }
}

/// Applies `edit`, checks invariants, undoes (expecting the original
/// content), redoes (expecting the edited content), and returns the
/// project in its edited state.
pub fn round_trip(p: &mut Project, edit: Edit) {
    let before = p.clone();
    let undo = p.apply(edit.clone()).unwrap_or_else(|e| panic!("{edit:?} failed: {e}"));
    p.check_invariants().unwrap();
    let after = p.clone();
    assert!(!undo.is_empty() || before.content_eq(&after), "non-empty change with empty inverse");
    let redo = p.apply(undo).expect("undo applies");
    assert!(p.content_eq(&before), "undo did not restore the project for {edit:?}");
    p.apply(redo).expect("redo applies");
    assert!(p.content_eq(&after), "redo did not restore the edit for {edit:?}");
}
