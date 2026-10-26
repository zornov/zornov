// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every edit: behaviour checks plus an undo/redo round trip.

mod common;
use common::*;
use studio_model::*;

fn new_clip(f: &Fx, media: MediaId, source_in: Micros, dur: Micros, name: &str) -> Clip {
    let mut c = Clip::media(ClipId(0), f.spine, 0, media, source_in, dur);
    c.name = name.into();
    c
}

fn names(f: &Fx) -> Vec<String> {
    f.spine_layout().into_iter().map(|x| x.0).collect()
}

#[test]
fn fixture_layout() {
    let f = fixture();
    assert_eq!(f.spine_layout(), vec![("a".into(), 0, 3 * S), ("b".into(), 3 * S, 2 * S), ("c".into(), 5 * S, 2 * S)]);
    assert_eq!(f.clip(f.title).start, S);
    assert_eq!(f.seq().lane_position(f.v1), Some(1));
    assert_eq!(f.seq().lane_position(f.a1), Some(-1));
}

#[test]
fn primitives_round_trip() {
    let mut f = fixture();
    let seq = f.seq;
    let mut c = f.clip(f.title).clone();
    c.transform.opacity = 0.5;
    round_trip(&mut f.p, Edit::SetClip { seq, clip: Box::new(c) });
    let mut t = f.seq().transition(f.dissolve).unwrap().clone();
    t.kind = "wipe".into();
    round_trip(&mut f.p, Edit::SetTransition { seq, transition: t });
    let m = MarkerId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::InsertMarker { seq, marker: Marker::new(m, S, "beat") });
    let mut mk = f.seq().markers[0].clone();
    mk.note = "drop".into();
    round_trip(&mut f.p, Edit::SetMarker { seq, marker: mk });
    round_trip(&mut f.p, Edit::RemoveMarker { seq, id: m });
    let mut lane = f.seq().lane(f.v1).unwrap().clone();
    lane.locked = true;
    round_trip(&mut f.p, Edit::SetLane { seq, lane });
    round_trip(&mut f.p, Edit::RenameSequence { id: seq, name: "Main cut".into() });
    let mut settings = f.p.settings.clone();
    settings.width = 1080;
    settings.height = 1920;
    round_trip(&mut f.p, Edit::SetSettings { settings: Box::new(settings) });
    let mut item = f.p.media_item(f.music).unwrap().clone();
    item.folder = Some("Audio".into());
    round_trip(&mut f.p, Edit::SetMedia { item: Box::new(item) });
    let pid = PresetId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::InsertPreset { preset: Preset { id: pid, name: "Warm".into(), kind: "grade".into(), data: serde_json::json!({"exposure": 0.3}) } });
    round_trip(&mut f.p, Edit::SetPreset { preset: Preset { id: pid, name: "Warmer".into(), kind: "grade".into(), data: serde_json::json!({}) } });
    round_trip(&mut f.p, Edit::RemovePreset { id: pid });
    let sid = SequenceId(f.p.alloc_id());
    let lid = LaneId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::InsertSequence { sequence: Box::new(Sequence::new(sid, "B-roll", lid)), index: None });
    round_trip(&mut f.p, Edit::SetActiveSequence { id: sid });
    round_trip(&mut f.p, Edit::SetActiveSequence { id: seq });
    round_trip(&mut f.p, Edit::RemoveSequence { id: sid });
}

#[test]
fn bad_primitives_are_rejected_without_change() {
    let mut f = fixture();
    let seq = f.seq;
    let before = f.p.clone();
    // Removing a clip in use by a transition breaks an invariant.
    assert!(matches!(f.p.apply(Edit::RemoveClip { seq, clip: f.a }), Err(EditError::Invariant(_))));
    // Removing media still used by clips.
    assert!(matches!(f.p.apply(Edit::RemoveMedia { id: f.video }), Err(EditError::Invariant(_))));
    // Removing the active sequence.
    assert!(f.p.apply(Edit::RemoveSequence { id: seq }).is_err());
    // A batch that fails half-way leaves nothing behind.
    let mut moved = f.clip(f.title).clone();
    moved.start = 9 * S;
    let bad = Edit::Batch { edits: vec![Edit::SetClip { seq, clip: Box::new(moved) }, Edit::RemoveLane { seq, lane: f.v1 }] };
    assert!(f.p.apply(bad).is_err());
    assert_eq!(f.p, before);
}

#[test]
fn insert_ripples_spine() {
    let mut f = fixture();
    let seq = f.seq;
    let clip = new_clip(&f, f.video, 0, S, "x");
    round_trip(&mut f.p, Edit::Insert { seq, lane: None, at: 4 * S, clip: Box::new(clip) });
    // b was split at 4 s and the second half pushed right.
    assert_eq!(names(&f), vec!["a", "b", "x", "b", "c"]);
    assert_eq!(f.seq().spine_end(), 8 * S);
    let halves: Vec<&Clip> = f.seq().spine_clips().into_iter().filter(|c| c.name == "b").collect();
    assert_eq!(halves[1].source_in, 6 * S);
    // Overlay lanes are not touched.
    assert_eq!(f.clip(f.title).start, S);
}

#[test]
fn insert_past_the_end_adds_a_gap() {
    let mut f = fixture();
    let seq = f.seq;
    let clip = new_clip(&f, f.video, 0, S, "late");
    round_trip(&mut f.p, Edit::Insert { seq, lane: None, at: 10 * S, clip: Box::new(clip) });
    assert_eq!(f.spine_layout()[3], ("gap".into(), 7 * S, 3 * S));
    assert_eq!(f.seq().spine_end(), 11 * S);
}

#[test]
fn overwrite_replaces_content() {
    let mut f = fixture();
    let seq = f.seq;
    let clip = new_clip(&f, f.video, 0, 2 * S, "x");
    round_trip(&mut f.p, Edit::Overwrite { seq, lane: None, at: 2 * S, clip: Box::new(clip) });
    assert_eq!(f.spine_layout(), vec![("a".into(), 0, 2 * S), ("x".into(), 2 * S, 2 * S), ("b".into(), 4 * S, S), ("c".into(), 5 * S, 2 * S)]);
    // The dissolve's clips are no longer adjacent, so it went away.
    assert!(f.seq().transitions.is_empty());
}

#[test]
fn append_to_lanes() {
    let mut f = fixture();
    let seq = f.seq;
    let clip = new_clip(&f, f.music, 0, S, "more");
    round_trip(&mut f.p, Edit::Append { seq, lane: Some(f.a1), clip: Box::new(clip) });
    assert_eq!(f.seq().clips_on(f.a1).last().unwrap().start, 5 * S);
}

#[test]
fn split_all_lanes_and_selection() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Split { seq, at: 2 * S, clips: None });
    assert_eq!(f.seq().clips_on(f.spine).len(), 4);
    assert_eq!(f.seq().clips_on(f.v1).len(), 2);
    assert_eq!(f.seq().clips_on(f.a1).len(), 2);
    // The transition left a, so it now leaves a's right half.
    let t = f.seq().transition(f.dissolve).unwrap();
    assert_eq!(f.clip(t.left).start, 2 * S);
    let mut g = fixture();
    let seq = g.seq;
    round_trip(&mut g.p, Edit::Split { seq, at: 6 * S, clips: Some(vec![g.c]) });
    assert_eq!(g.seq().clips_on(g.spine).len(), 4);
    assert!(g.p.apply(Edit::Split { seq, at: 0, clips: None }).is_err());
}

#[test]
fn split_reversed_clip_keeps_content() {
    let mut f = fixture();
    let seq = f.seq;
    f.p.apply(Edit::Reverse { seq, clips: vec![f.b] }).unwrap();
    let before: Vec<Micros> = (0..4).map(|i| f.clip(f.b).source_time_at(3 * S + i * S / 2)).collect();
    round_trip(&mut f.p, Edit::Split { seq, at: 4 * S, clips: Some(vec![f.b]) });
    let s = f.seq();
    let after: Vec<Micros> = (0..4).map(|i| s.clip_at(f.spine, 3 * S + i * S / 2).unwrap().source_time_at(3 * S + i * S / 2)).collect();
    assert_eq!(before, after);
}

#[test]
fn trims() {
    let mut f = fixture();
    let seq = f.seq;
    // Ripple trim of b's end: c follows.
    round_trip(&mut f.p, Edit::Trim { seq, clip: f.b, edge: Edge::End, delta: -S / 2, ripple: true });
    assert_eq!(f.clip(f.c).start, 4 * S + S / 2);
    // Non-ripple trim on the spine opens a gap.
    round_trip(&mut f.p, Edit::Trim { seq, clip: f.b, edge: Edge::End, delta: -S / 2, ripple: false });
    assert_eq!(f.spine_layout()[2], ("gap".into(), 4 * S, S / 2));
    // ... which a non-ripple extension consumes again.
    round_trip(&mut f.p, Edit::Trim { seq, clip: f.b, edge: Edge::End, delta: S / 4, ripple: false });
    assert_eq!(f.spine_layout()[2], ("gap".into(), 4 * S + S / 4, S / 4));
    // Start trim with ripple keeps the clip in place and moves its source.
    round_trip(&mut f.p, Edit::Trim { seq, clip: f.a, edge: Edge::Start, delta: S / 2, ripple: true });
    assert_eq!((f.clip(f.a).start, f.clip(f.a).source_in, f.clip(f.a).duration), (0, S + S / 2, 2 * S + S / 2));
    // Non-ripple start trim on the overlay lane moves the start.
    round_trip(&mut f.p, Edit::Trim { seq, clip: f.title, edge: Edge::Start, delta: S / 2, ripple: false });
    assert_eq!(f.clip(f.title).start, S + S / 2);
    // Extending beyond the media fails and changes nothing.
    let before = f.p.clone();
    assert!(matches!(f.p.apply(Edit::Trim { seq, clip: f.a, edge: Edge::Start, delta: -5 * S, ripple: true }), Err(EditError::NoHandles(_))));
    assert_eq!(f.p, before);
    // A trim into a real neighbour without ripple is refused.
    assert!(f.p.apply(Edit::Trim { seq, clip: f.c, edge: Edge::Start, delta: -S, ripple: false }).is_err());
}

#[test]
fn trim_shrinks_transition_that_no_longer_fits() {
    let mut f = fixture();
    let seq = f.seq;
    // Leave a with only 100 ms of media after its out point.
    f.p.apply(Edit::Trim { seq, clip: f.a, edge: Edge::End, delta: 5 * S + 900_000, ripple: true }).unwrap();
    let t = f.seq().transition(f.dissolve).unwrap();
    assert_eq!(t.duration, 200_000);
}

#[test]
fn ripple_delete_and_lift() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::RippleDelete { seq, clips: vec![f.b] });
    assert_eq!(names(&f), vec!["a", "c"]);
    assert_eq!(f.clip(f.c).start, 3 * S);
    assert!(f.seq().transitions.is_empty());
    let mut g = fixture();
    let seq = g.seq;
    round_trip(&mut g.p, Edit::Lift { seq, clips: vec![g.b] });
    assert_eq!(names(&g), vec!["a", "gap", "c"]);
    // Lifting the last clip leaves no trailing gap.
    round_trip(&mut g.p, Edit::Lift { seq, clips: vec![g.c] });
    assert_eq!(names(&g), vec!["a"]);
    round_trip(&mut g.p, Edit::RippleDelete { seq, clips: vec![g.song] });
    assert!(g.seq().clips_on(g.a1).is_empty());
}

#[test]
fn moves() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Move { seq, clips: vec![f.title], delta: 2 * S, to_lane: None });
    assert_eq!(f.clip(f.title).start, 3 * S);
    // Move a spine clip up to the overlay lane: a gap stays behind.
    round_trip(&mut f.p, Edit::Move { seq, clips: vec![f.c], delta: 0, to_lane: Some(f.v1) });
    assert_eq!(f.clip(f.c).lane, f.v1);
    assert_eq!(names(&f), vec!["a", "b"]);
    // Move the title onto the spine: overwrite semantics.
    round_trip(&mut f.p, Edit::Move { seq, clips: vec![f.title], delta: -3 * S, to_lane: Some(f.spine) });
    assert_eq!(f.spine_layout()[0], ("title".into(), 0, 2 * S));
    assert!(f.p.apply(Edit::Move { seq, clips: vec![f.song], delta: -10 * S, to_lane: None }).is_err());
}

#[test]
fn reorder_on_spine() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Reorder { seq, clip: f.c, before: Some(f.a) });
    assert_eq!(names(&f), vec!["c", "a", "b"]);
    round_trip(&mut f.p, Edit::Reorder { seq, clip: f.c, before: None });
    assert_eq!(names(&f), vec!["a", "b", "c"]);
    assert!(f.p.apply(Edit::Reorder { seq, clip: f.title, before: None }).is_err());
}

#[test]
fn slip_slide_roll() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Slip { seq, clip: f.b, delta: -S });
    assert_eq!(f.clip(f.b).source_in, 4 * S);
    assert_eq!(f.clip(f.b).start, 3 * S);
    assert!(matches!(f.p.apply(Edit::Slip { seq, clip: f.b, delta: 20 * S }), Err(EditError::NoHandles(_))));
    round_trip(&mut f.p, Edit::Slide { seq, clip: f.b, delta: S / 2 });
    assert_eq!(f.clip(f.b).start, 3 * S + S / 2);
    assert_eq!(f.clip(f.a).duration, 3 * S + S / 2);
    assert_eq!((f.clip(f.c).start, f.clip(f.c).duration), (5 * S + S / 2, S + S / 2));
    assert_eq!(f.seq().spine_end(), 7 * S);
    round_trip(&mut f.p, Edit::Roll { seq, left: f.a, delta: -S });
    assert_eq!(f.clip(f.a).duration, 2 * S + S / 2);
    assert_eq!((f.clip(f.b).start, f.clip(f.b).source_in), (2 * S + S / 2, 3 * S));
    assert!(f.p.apply(Edit::Slide { seq, clip: f.a, delta: S }).is_err());
}

#[test]
fn duplicate_and_paste() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Duplicate { seq, clips: vec![f.a, f.b] });
    assert_eq!(names(&f), vec!["a", "b", "a", "b", "c"]);
    assert_eq!(f.seq().transitions.len(), 2, "the dissolve was copied too");

    let mut g = fixture();
    let seq = g.seq;
    let cb = Clipboard::copy(&g.p, seq, &[g.title, g.song]).unwrap();
    let cb = Clipboard::from_json(&cb.to_json()).unwrap();
    round_trip(&mut g.p, Edit::Paste { seq, at: 6 * S, clipboard: Box::new(cb.clone()), mode: PasteMode::Overwrite });
    assert_eq!(g.seq().clips_on(g.a1).len(), 2);
    assert_eq!(g.seq().clips_on(g.v1).last().unwrap().start, 7 * S);

    // Pasting into a fresh project brings the media along.
    let mut other = Project::default();
    let oseq = other.active_sequence;
    round_trip(&mut other, Edit::Paste { seq: oseq, at: 0, clipboard: Box::new(cb), mode: PasteMode::Insert });
    assert_eq!(other.media.len(), 1);
    assert_eq!(other.active().lanes.len(), 3, "lanes created for the pasted clips");
}

#[test]
fn paste_attributes() {
    let mut f = fixture();
    let seq = f.seq;
    let mut src = f.clip(f.a).clone();
    src.transform.rotation = 45.0;
    src.effects.push(Effect::new(EffectId(1), "glow"));
    src.keyframes.insert("effect.1.amount".into(), Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(S, 1.0, Interp::Linear)]));
    src.effects[0].params.insert("amount".into(), Value::Scalar(0.0));
    src.speed = Speed::Constant(2.0);
    round_trip(&mut f.p, Edit::PasteAttributes { seq, targets: vec![f.b], source: Box::new(src), attrs: AttrSet::ALL });
    let b = f.clip(f.b);
    assert_eq!(b.transform.rotation, 45.0);
    assert_eq!(b.effects.len(), 1);
    assert!(b.keyframes.contains_key("effect.1.amount"));
    assert_eq!(b.duration, S, "speed 2 halves the duration");
    assert_eq!(f.clip(f.c).start, 4 * S, "and the spine rippled");
}

#[test]
fn groups_and_links() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Group { seq, clips: vec![f.title, f.song], group: GroupId(0) });
    assert!(f.clip(f.title).group.is_some() && f.clip(f.title).group == f.clip(f.song).group);
    round_trip(&mut f.p, Edit::Ungroup { seq, clips: vec![f.title, f.song] });
    assert!(f.clip(f.title).group.is_none());
    round_trip(&mut f.p, Edit::Link { seq, clips: vec![f.a, f.song], link: LinkId(0) });
    // Linked clips move together.
    round_trip(&mut f.p, Edit::Move { seq, clips: vec![f.song], delta: S, to_lane: None });
    round_trip(&mut f.p, Edit::Unlink { seq, clips: vec![f.a, f.song] });
    assert!(f.clip(f.a).link.is_none());
}

#[test]
fn detach_audio_follows_ripples() {
    let mut f = fixture();
    let seq = f.seq;
    let audio = ClipId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::DetachAudio { seq, clip: f.b, new_clip: audio });
    let a = f.clip(audio);
    assert_eq!(a.range(), f.clip(f.b).range());
    assert_eq!(f.seq().lane_position(a.lane), Some(-2), "song occupies the first audio lane");
    let ClipSource::Media(r) = &f.clip(f.b).source else { panic!() };
    assert!(!r.audio && r.video);
    // Rippling the spine carries the detached sound along.
    round_trip(&mut f.p, Edit::RippleDelete { seq, clips: vec![f.a] });
    assert_eq!(f.clip(audio).start, 0);
    assert_eq!(f.clip(f.b).start, 0);
    // Trimming the picture trims the sound.
    round_trip(&mut f.p, Edit::Trim { seq, clip: f.b, edge: Edge::End, delta: -S / 2, ripple: true });
    assert_eq!(f.clip(audio).duration, S + S / 2);
    // Deleting one deletes both.
    round_trip(&mut f.p, Edit::RippleDelete { seq, clips: vec![f.b] });
    assert!(f.seq().clip(audio).is_none());
    assert!(f.p.apply(Edit::DetachAudio { seq, clip: f.c, new_clip: ClipId(0) }).is_err());
}

#[test]
fn replace_media_and_freeze() {
    let mut f = fixture();
    let seq = f.seq;
    let other = MediaId(f.p.alloc_id());
    f.p.apply(Edit::InsertMedia { item: Box::new(MediaItem::new(other, "/media/take2.mp4", video_info(4 * S))), index: None }).unwrap();
    round_trip(&mut f.p, Edit::ReplaceMedia { seq, clip: f.b, media: other });
    assert_eq!(f.clip(f.b).media_id(), Some(other));
    // b used 5–7 s, beyond the 4 s file, so it starts from 0.
    assert_eq!(f.clip(f.b).source_in, 0);
    // Sound-only media keeps only the sound stream.
    let mut g = fixture();
    round_trip(&mut g.p, Edit::ReplaceMedia { seq, clip: g.a, media: g.music });
    let ClipSource::Media(r) = &g.clip(g.a).source else { panic!() };
    assert!(r.audio && !r.video);
    let short = MediaId(f.p.alloc_id());
    f.p.apply(Edit::InsertMedia { item: Box::new(MediaItem::new(short, "/media/blip.mp4", video_info(S))), index: None }).unwrap();
    assert!(matches!(f.p.apply(Edit::ReplaceMedia { seq, clip: f.a, media: short }), Err(EditError::NoHandles(_))));

    let still = ClipId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::FreezeFrame { seq, clip: f.a, at: 2 * S, duration: S, new_clip: still });
    let s = f.clip(still);
    assert_eq!((s.start, s.duration, s.source_in), (2 * S, S, 3 * S));
    assert!(s.speed.is_freeze());
    assert_eq!(f.seq().spine_end(), 8 * S);
}

#[test]
fn reverse_and_speed() {
    let mut f = fixture();
    let seq = f.seq;
    round_trip(&mut f.p, Edit::Reverse { seq, clips: vec![f.b] });
    assert!(f.clip(f.b).reversed);
    assert_eq!(f.clip(f.b).source_time_at(3 * S), 7 * S - 1);
    round_trip(&mut f.p, Edit::SetSpeed { seq, clip: f.a, speed: Speed::Constant(0.5) });
    assert_eq!(f.clip(f.a).duration, 6 * S);
    assert_eq!(f.clip(f.b).start, 6 * S, "following clips ripple");
    let curve = Track::new(vec![Keyframe::new(0, 1.0, Interp::Linear), Keyframe::new(2 * S, 3.0, Interp::Linear)]);
    round_trip(&mut f.p, Edit::SetSpeed { seq, clip: f.a, speed: Speed::Curve(curve) });
    assert!((f.clip(f.a).source_len() - 3 * S).abs() <= 2);
    assert!(f.p.apply(Edit::SetSpeed { seq, clip: f.a, speed: Speed::Constant(-1.0) }).is_err());
}

#[test]
fn transitions() {
    let mut f = fixture();
    let seq = f.seq;
    let t = Transition { id: TransitionId(0), left: f.b, right: ClipId(0), kind: "wipe".into(), duration: S, align: TransitionAlign::EndAtCut, params: Default::default(), extra: Default::default() };
    round_trip(&mut f.p, Edit::AddTransition { seq, transition: t.clone() });
    assert_eq!(f.seq().transitions.len(), 2);
    // The still has unlimited handles; b has 1.75 s left after the dissolve at its start.
    assert_eq!(f.p.max_transition_duration(seq, f.b, TransitionAlign::Centre), Some(3 * S + S / 2 + 1));
    assert!(f.p.apply(Edit::AddTransition { seq, transition: t.clone() }).is_err(), "cut already has one");
    assert!(matches!(f.p.apply(Edit::AddTransition { seq, transition: Transition { left: f.c, ..t.clone() } }), Err(EditError::Invalid(_))));
    round_trip(&mut f.p, Edit::RemoveTransition { seq, id: f.dissolve });
    let mut g = fixture();
    let too_long = Transition { left: g.a, duration: 20 * S, ..t };
    g.p.apply(Edit::RemoveTransition { seq, id: g.dissolve }).unwrap();
    assert!(matches!(g.p.apply(Edit::AddTransition { seq, transition: too_long }), Err(EditError::NoHandles(_))));
}

#[test]
fn effects_grade_masks() {
    let mut f = fixture();
    let seq = f.seq;
    let e = EffectId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddEffect { seq, clip: f.a, stack: Stack::Video, effect: Effect::new(e, "gaussian_blur"), index: None });
    let e2 = EffectId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddEffect { seq, clip: f.a, stack: Stack::Video, effect: Effect::new(e2, "vhs"), index: Some(0) });
    assert_eq!(f.clip(f.a).effects[0].kind, "vhs");
    round_trip(&mut f.p, Edit::MoveEffect { seq, clip: f.a, effect: e2, to: 1 });
    assert_eq!(f.clip(f.a).effects[1].kind, "vhs");
    let mut changed = f.clip(f.a).effects[0].clone();
    changed.params.insert("radius".into(), Value::Scalar(8.0));
    round_trip(&mut f.p, Edit::SetEffect { seq, clip: f.a, effect: changed });
    round_trip(&mut f.p, Edit::SetKeyframe { seq, clip: f.a, property: format!("effect.{}.radius", e.0), key: Keyframe::new(0, 2.0, Interp::Ease) });
    round_trip(&mut f.p, Edit::RemoveEffect { seq, clip: f.a, effect: e });
    assert!(f.clip(f.a).keyframes.is_empty(), "tracks of a removed effect go too");
    let ae = EffectId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddEffect { seq, clip: f.song, stack: Stack::Audio, effect: Effect::new(ae, "eq"), index: None });
    assert_eq!(f.clip(f.song).audio.effects.len(), 1);

    let mut grade = Grade::default();
    grade.params.insert("exposure".into(), Value::Scalar(0.5));
    round_trip(&mut f.p, Edit::SetGrade { seq, clip: f.a, grade: Some(grade) });
    round_trip(&mut f.p, Edit::SetKeyframe { seq, clip: f.a, property: "grade.exposure".into(), key: Keyframe::new(S, 1.0, Interp::Linear) });
    round_trip(&mut f.p, Edit::SetGrade { seq, clip: f.a, grade: None });
    assert!(f.clip(f.a).keyframes.is_empty());

    let m = MaskId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddMask { seq, clip: f.title, mask: Mask::new(m, MaskShape::Ellipse), index: None });
    let mut mask = f.clip(f.title).masks[0].clone();
    mask.invert = true;
    mask.params.insert("feather".into(), Value::Scalar(12.0));
    round_trip(&mut f.p, Edit::SetMask { seq, clip: f.title, mask });
    round_trip(&mut f.p, Edit::RemoveMask { seq, clip: f.title, mask: m });
    assert!(f.clip(f.title).masks.is_empty());
}

#[test]
fn keyframes() {
    let mut f = fixture();
    let seq = f.seq;
    let path = "transform.opacity".to_string();
    round_trip(&mut f.p, Edit::SetKeyframe { seq, clip: f.title, property: path.clone(), key: Keyframe::new(0, 0.0, Interp::Linear) });
    round_trip(&mut f.p, Edit::SetKeyframe { seq, clip: f.title, property: path.clone(), key: Keyframe::new(S, 1.0, Interp::Linear) });
    assert_eq!(f.clip(f.title).transform_at(S / 2).opacity, 0.5);
    round_trip(&mut f.p, Edit::RemoveKeyframe { seq, clip: f.title, property: path.clone(), time: 0 });
    round_trip(&mut f.p, Edit::RemoveKeyframe { seq, clip: f.title, property: path.clone(), time: S });
    assert!(f.clip(f.title).keyframes.is_empty());
    let tr = Track::new(vec![Keyframe::new(0, Vec2::ZERO, Interp::Auto), Keyframe::new(S, Vec2::new(100.0, 0.0), Interp::Auto)]);
    round_trip(&mut f.p, Edit::SetTrack { seq, clip: f.title, property: "transform.position".into(), track: Some(tr) });
    round_trip(&mut f.p, Edit::SetTrack { seq, clip: f.title, property: "transform.position".into(), track: None });
    // Wrong kinds and unknown paths are refused.
    assert!(f.p.apply(Edit::SetKeyframe { seq, clip: f.title, property: path.clone(), key: Keyframe::new(0, Vec2::ZERO, Interp::Linear) }).is_err());
    assert!(f.p.apply(Edit::SetKeyframe { seq, clip: f.title, property: "transform.wobble".into(), key: Keyframe::new(0, 1.0, Interp::Linear) }).is_err());
    let bad = Track::new(vec![Keyframe::new(0, Value::Bool(true), Interp::Linear)]);
    assert!(matches!(f.p.apply(Edit::SetTrack { seq, clip: f.title, property: path, track: Some(bad) }), Err(EditError::Invariant(_))));
    // Start trims shift keys so they stay on the same frames.
    f.p.apply(Edit::SetKeyframe { seq, clip: f.a, property: "transform.scale_x".into(), key: Keyframe::new(2 * S, 2.0, Interp::Linear) }).unwrap();
    f.p.apply(Edit::Trim { seq, clip: f.a, edge: Edge::Start, delta: S, ripple: true }).unwrap();
    assert_eq!(f.clip(f.a).keyframes["transform.scale_x"].keys[0].time, S);
}

#[test]
fn markers() {
    let mut f = fixture();
    let seq = f.seq;
    let m = MarkerId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddMarker { seq, clip: None, marker: Marker::new(m, 2 * S, "chorus") });
    let mut mk = f.seq().markers[0].clone();
    mk.colour = Rgba::new(1.0, 0.0, 0.0, 1.0);
    round_trip(&mut f.p, Edit::ChangeMarker { seq, clip: None, marker: mk });
    round_trip(&mut f.p, Edit::DeleteMarker { seq, clip: None, id: m });
    let cm = MarkerId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddMarker { seq, clip: Some(f.b), marker: Marker::new(cm, S, "smile") });
    let mut mk = f.clip(f.b).markers[0].clone();
    mk.note = "best take".into();
    round_trip(&mut f.p, Edit::ChangeMarker { seq, clip: Some(f.b), marker: mk });
    // Splitting moves the marker to the right half.
    f.p.apply(Edit::Split { seq, at: 3 * S + S / 2, clips: Some(vec![f.b]) }).unwrap();
    assert!(f.clip(f.b).markers.is_empty());
    let right = f.seq().clip_at(f.spine, 4 * S).unwrap().clone();
    assert_eq!(right.markers[0].time, S / 2);
    round_trip(&mut f.p, Edit::DeleteMarker { seq, clip: Some(right.id), id: cm });
}

#[test]
fn lanes() {
    let mut f = fixture();
    let seq = f.seq;
    let v2 = LaneId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::AddLane { seq, lane: Lane::new(v2, LaneKind::Visual, "FX"), index: None });
    assert_eq!(f.seq().lane_position(v2), Some(2));
    round_trip(&mut f.p, Edit::MoveLane { seq, lane: v2, to: 1 });
    assert_eq!(f.seq().lane_position(v2), Some(1));
    assert!(matches!(f.p.apply(Edit::MoveLane { seq, lane: v2, to: 3 }), Err(EditError::Invariant(Violation::LaneOrder(_)))));
    round_trip(&mut f.p, Edit::DeleteLane { seq, lane: f.a1 });
    assert!(f.seq().clip(f.song).is_none());
    assert!(f.p.apply(Edit::DeleteLane { seq, lane: f.spine }).is_err());
    assert!(f.p.apply(Edit::AddLane { seq, lane: Lane::new(LaneId(0), LaneKind::Spine, "x"), index: None }).is_err());
    round_trip(&mut f.p, Edit::RemoveLane { seq, lane: v2 });
}

#[test]
fn locked_lanes_refuse_edits() {
    let mut f = fixture();
    let seq = f.seq;
    let mut lane = f.seq().lane(f.v1).unwrap().clone();
    lane.locked = true;
    f.p.apply(Edit::SetLane { seq, lane }).unwrap();
    let before = f.p.clone();
    assert!(matches!(f.p.apply(Edit::Move { seq, clips: vec![f.title], delta: S, to_lane: None }), Err(EditError::Locked(_))));
    assert!(matches!(f.p.apply(Edit::SetGrade { seq, clip: f.title, grade: None }), Err(EditError::Locked(_))));
    // Split-all skips the locked lane.
    f.p.apply(Edit::Split { seq, at: 2 * S, clips: None }).unwrap();
    assert_eq!(f.seq().clips_on(f.v1).len(), 1);
    assert_eq!(before.sequence(seq).unwrap().clips_on(f.v1), f.seq().clips_on(f.v1));
}

#[test]
fn compound_clips() {
    let mut f = fixture();
    let seq = f.seq;
    let nested = SequenceId(f.p.alloc_id());
    let comp = ClipId(f.p.alloc_id());
    round_trip(&mut f.p, Edit::CreateCompound { seq, clips: vec![f.a, f.b, f.title], name: "Intro".into(), new_sequence: nested, new_clip: comp });
    assert_eq!(names(&f), vec!["Intro", "c"]);
    assert_eq!(f.clip(comp).range(), TimeRange::new(0, 5 * S));
    let n = f.p.sequence(nested).unwrap();
    assert_eq!(n.clips.len(), 3);
    assert_eq!(n.transitions.len(), 1, "the dissolve moved inside");
    assert_eq!(n.lanes.len(), 2);
    // Nesting a sequence in itself is refused.
    let mut inner = n.clone();
    let lid = inner.spine_id();
    inner.clips.push(Clip::new(ClipId(f.p.alloc_id()), lid, 5 * S, S, ClipSource::Sequence(SequenceRef { sequence: nested, extra: Default::default() })));
    assert!(matches!(f.p.apply(Edit::RemoveSequence { id: nested }), Err(EditError::Invariant(_))));
    let _ = inner;

    round_trip(&mut f.p, Edit::ExpandCompound { seq, clip: comp });
    assert_eq!(names(&f), vec!["a", "b", "c"]);
    assert_eq!(f.seq().clips_on(f.v1).len(), 1);
    assert_eq!(f.seq().transitions.len(), 1);
    // Overlay clips outside the spine selection cannot be included.
    let mut g = fixture();
    assert!(g.p.apply(Edit::CreateCompound { seq, clips: vec![g.c, g.title], name: String::new(), new_sequence: SequenceId(0), new_clip: ClipId(0) }).is_err());
    // A trimmed compound clip expands to just the visible part.
    let comp2 = ClipId(g.p.alloc_id());
    g.p.apply(Edit::CreateCompound { seq, clips: vec![g.a, g.b], name: "AB".into(), new_sequence: SequenceId(0), new_clip: comp2 }).unwrap();
    g.p.apply(Edit::Trim { seq, clip: comp2, edge: Edge::Start, delta: S, ripple: true }).unwrap();
    round_trip(&mut g.p, Edit::ExpandCompound { seq, clip: comp2 });
    let lay = g.spine_layout();
    assert_eq!(lay[0], ("a".into(), 0, 2 * S));
    assert_eq!(g.seq().spine_clips()[0].source_in, 2 * S);
}

#[test]
fn history_transactions() {
    let mut f = fixture();
    let seq = f.seq;
    let original = f.p.clone();
    let mut h = History::new();
    // A drag: many small moves, one undo step.
    h.begin("Move title");
    for _ in 0..10 {
        h.apply(&mut f.p, "", Edit::Move { seq, clips: vec![f.title], delta: S / 10, to_lane: None }).unwrap();
    }
    h.commit();
    assert_eq!(f.clip(f.title).start, 2 * S);
    assert_eq!(h.undo_len(), 1);
    assert_eq!(h.undo_label(), Some("Move title"));
    h.apply(&mut f.p, "Delete b", Edit::RippleDelete { seq, clips: vec![f.b] }).unwrap();
    assert!(h.is_dirty());
    assert_eq!(h.undo(&mut f.p).unwrap().as_deref(), Some("Delete b"));
    assert_eq!(h.undo(&mut f.p).unwrap().as_deref(), Some("Move title"));
    assert!(f.p.content_eq(&original));
    assert_eq!(h.undo(&mut f.p).unwrap(), None);
    h.redo(&mut f.p).unwrap();
    h.redo(&mut f.p).unwrap();
    assert!(f.seq().clip(f.b).is_none());
    assert!(!h.can_redo());
    // Cancelling a transaction reverts it.
    let snapshot = f.p.clone();
    let rev = h.revision();
    h.begin("Drag");
    h.apply(&mut f.p, "", Edit::Move { seq, clips: vec![f.song], delta: S, to_lane: None }).unwrap();
    assert!(h.revision() > rev);
    h.cancel(&mut f.p).unwrap();
    assert!(f.p.content_eq(&snapshot));
    assert_eq!(h.revision(), rev, "a cancelled change is not a change: the revision goes back with the project");
    // Cancelling a nested level reverts only that level; the outer one
    // keeps its edits and still commits as one step.
    let before = h.undo_len();
    h.begin("Request");
    h.apply(&mut f.p, "", Edit::RenameSequence { id: seq, name: "kept".into() }).unwrap();
    let kept = f.p.clone();
    h.begin("Try");
    h.apply(&mut f.p, "", Edit::Move { seq, clips: vec![f.song], delta: S, to_lane: None }).unwrap();
    let mid = h.revision();
    h.begin("Try again");
    h.apply(&mut f.p, "", Edit::Move { seq, clips: vec![f.song], delta: S, to_lane: None }).unwrap();
    h.cancel(&mut f.p).unwrap();
    assert_eq!(h.revision(), mid, "a nested level cancelled puts the revision back too");
    h.apply(&mut f.p, "", Edit::Move { seq, clips: vec![f.song], delta: S, to_lane: None }).unwrap();
    h.cancel(&mut f.p).unwrap();
    assert!(f.p.content_eq(&kept), "the inner level only");
    assert!(h.in_transaction());
    h.commit();
    assert!(!h.in_transaction());
    assert_eq!(h.undo_len(), before + 1);
    assert_eq!(h.undo_label(), Some("Request"));
    h.undo(&mut f.p).unwrap();
    assert!(f.p.content_eq(&snapshot));
    // A new edit after undo clears redo.
    h.undo(&mut f.p).unwrap();
    h.apply(&mut f.p, "Rename", Edit::RenameSequence { id: seq, name: "x".into() }).unwrap();
    assert!(!h.can_redo());
    // Steps carry who made them; a new change ends only its author's redo.
    let agent = Author::Agent { request: 1, call: 1 };
    let was = h.set_author(agent);
    h.apply(&mut f.p, "AI rename", Edit::RenameSequence { id: seq, name: "ai".into() }).unwrap();
    h.set_author(was);
    assert_eq!(h.undo_steps().last().map(|s| s.author), Some(agent));
    h.undo(&mut f.p).unwrap();
    h.apply(&mut f.p, "Mine", Edit::RenameSequence { id: seq, name: "mine".into() }).unwrap();
    assert!(h.redo_steps().iter().any(|s| s.author == agent), "the person's change keeps the agent's redo");
    // Saving in the middle of a transaction keeps it open, and in the save.
    h.begin("Drag");
    h.apply(&mut f.p, "", Edit::RenameSequence { id: seq, name: "dragging".into() }).unwrap();
    let saved = h.snapshot();
    assert!(h.in_transaction());
    assert_eq!(saved.undo.last().map(|s| s.label.as_str()), Some("Drag"));
    h.commit();
    let mut limited = History::with_limit(2);
    for i in 0..5 {
        limited.apply(&mut f.p, format!("r{i}"), Edit::RenameSequence { id: seq, name: format!("n{i}") }).unwrap();
    }
    assert_eq!(limited.undo_len(), 2);
}

#[test]
fn edits_serialise() {
    let f = fixture();
    let e = Edit::Trim { seq: f.seq, clip: f.a, edge: Edge::End, delta: -S, ripple: true };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"op\":\"trim\""));
    assert_eq!(serde_json::from_str::<Edit>(&json).unwrap(), e);
}

#[test]
fn frame_snapping_helper() {
    let f = fixture();
    let r = f.p.settings.frame_rate;
    assert_eq!(r.snap(S + 10_000), S);
    assert_eq!(r.snap(S + 20_000), S + 33_333);
    let targets = SnapTargets::collect(f.seq(), Some(4 * S), &[f.title]);
    assert_eq!(targets.snap(3 * S + 40_000, 50_000), Some(3 * S));
}

#[test]
fn batches_of_operations() {
    let mut f = fixture();
    let seq = f.seq;
    let batch = Edit::Batch { edits: vec![Edit::Split { seq, at: 2 * S, clips: Some(vec![f.a]) }, Edit::RippleDelete { seq, clips: vec![f.c] }, Edit::RenameSequence { id: seq, name: "Cut".into() }] };
    round_trip(&mut f.p, batch);
    assert_eq!(names(&f), vec!["a", "a", "b"]);
    // One failing member undoes the others.
    let before = f.p.clone();
    let bad = Edit::Batch { edits: vec![Edit::RippleDelete { seq, clips: vec![f.b] }, Edit::Slip { seq, clip: f.a, delta: -100 * S }] };
    assert!(f.p.apply(bad).is_err());
    assert!(f.p.content_eq(&before));
}
