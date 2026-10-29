// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sound department's tools for the AI: `mix` (buses, sends, lane and
//! bus processing, automation, surround and object panning, solo/mute,
//! meters), `loudness` (measure against a delivery spec, normalise in one
//! step), `stems` (DX / MX / FX / M&E / mix WAVs, ADM objects), `dialogue`
//! (leveling, voice matching), `adr` (cue sheet, takes, sync),
//! `subtitles` (from speech, timing rules, translation, burn-in, SRT /
//! VTT) and `dub` (per-character voices per language, lines fitted to the
//! original timing). The window's Mixer panel uses the same `sound_desk`
//! functions, so the AI and the person see one mix.

use super::{Session, f, secs};
use crate::sound_desk::{self as desk, AdrCue, Desk, LaneMix, Send};
use serde_json::{Value, json};
use studio_model as m;
use studio_sound::loudness::LoudnessTarget;
use studio_sound::mixer::BusKind;
use studio_sound::surround::Layout;

/// The tools' names.
pub const TOOLS: [&str; 7] = ["mix", "loudness", "stems", "dialogue", "adr", "subtitles", "dub"];

/// Tool descriptions and schemas for MCP.
pub fn tools() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    let num = |d: &str| json!({"type": "number", "description": d});
    let any = |d: &str| json!({"description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let t = |name: &str, desc: &str, props: Value| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": []}});
    vec![
        t(
            "mix",
            "The film mix: lanes into buses (DX, MX, FX, BG, aux returns) into the master. Fields: action (show, setup, lane, bus, master, layout, automate, meters), lane, bus, role, volume, pan, mute, solo, send, effect (eq, compressor, limiter, gate, de_esser, reverb, delay, pitch_shift) with params, surround, layout (stereo, 5.1, 7.1), param, keys. More: guide {topic: \"tool:mix\"}.",
            json!({"action": s("show, setup, lane, bus, master, layout, automate, meters"), "lane": any("lane name or id"), "bus": any("bus name, kind or id"), "add": any("{kind, name} for a new bus"), "role": s("dialogue, music, effects, ambience"), "volume": num("dB"), "pan": num("-1..1"), "mute": b("silence it"), "solo": b("solo it"), "send": any("{bus, level, pre}"), "unsend": any("bus to stop sending to"), "effect": s("effect kind"), "params": any("effect settings"), "remove_effect": s("effect kind to take off"), "remove": b("remove the bus"), "surround": any("{x, y, z, width, centre, lfe}"), "object": b("an object in ADM export"), "ceiling": num("master true-peak ceiling dBTP"), "limiter": b("master limiter on/off"), "ducking": b("duck music under dialogue"), "layout": s("stereo, 5.1, 7.1"), "param": s("what to automate"), "keys": any("[[seconds, value], ...]"), "clear": b("remove the automation"), "at": num("seconds"), "duration": num("seconds")}),
        ),
        t(
            "loudness",
            "Loudness against a delivery spec: ebu_r128 (-23 LUFS, -1 dBTP), atsc_a85 (-24 LKFS, -2 dBTP), netflix (-27 LKFS dialogue-gated, -2 dBTP), amazon (-24, -2), streaming (YouTube/Spotify -14, -1), podcast (-16, -1). Without normalise: integrated, short-term max, momentary max, range and true peak of the delivered mix and whether it passes. normalise: true sets the master gain and limiter ceiling so the mix meets the spec (one step, measured again after).",
            json!({"target": s("the spec (default: the desk's, else streaming)"), "normalise": b("bring the mix to the spec"), "layout": s("measure this layout (default the desk's)")}),
        ),
        t(
            "stems",
            "Write the delivery stems as 24-bit 48 kHz Broadcast WAVs in the mix layout: DX, MX, FX (with backgrounds, or BG apart with separate_bg), M&E (music and effects, no dialogue: for foreign versions), the full mix, and for each dubbed language its DX and full mix. adm: also an ADM BWF (ITU-R BS.2076) with the bed and every object lane moving through the room, for immersive mastering.",
            json!({"path": s("folder for the files (default: Stems in the project folder)"), "layout": s("stereo, 5.1, 7.1 (default the desk's)"), "separate_bg": b("backgrounds as their own BG stem"), "adm": b("also write an ADM file")}),
        ),
        t(
            "dialogue",
            "Dialogue editing. level {clips, target LUFS (default -24; -27 for film, -16 for online)}: each phrase brought to the target as the clip's volume keys (editable). match {clip (the take), reference (the clip to sound like)}: EQ, level and room so pickups and ADR sit with the production sound.",
            json!({"action": s("level or match"), "clips": any("clip ids"), "clip": any("the take"), "reference": any("clip to match"), "target": num("LUFS"), "max_boost": num("dB"), "max_cut": num("dB")}),
        ),
        t(
            "adr",
            "ADR (re-recorded dialogue). Fields: action (list, add, from_subtitles, set, remove, sheet, beeps, take), cue, start, end, character, text, status, file, warp, match, replace.",
            json!({"action": s("list, add, from_subtitles, set, remove, sheet, beeps, take"), "cue": any("cue number"), "start": num("seconds"), "end": num("seconds"), "character": s("who says it"), "text": s("the line"), "reason": s("noise, performance, line change, technical, added"), "status": s("to do, recorded, approved"), "notes": s("notes"), "from": num("seconds"), "to": num("seconds"), "characters": any("names"), "lang": s("subtitle language to take lines from"), "path": s("file to write"), "file": s("the recorded take"), "warp": b("fit the take word by word"), "match": b("match the production voice"), "replace": b("dip the production line")}),
        ),
        t(
            "subtitles",
            "Subtitles and captions per language. Fields: action (list, make, from_captions, import, export, show, edit, speakers, conform, check, translate, burn_in, remove), lang, spec (netflix, bbc, social), path, format (srt, vtt), cue, text, start, end, texts. More: guide {topic: \"tool:subtitles\"}.",
            json!({"action": s("list, make, from_captions, import, export, show, edit, speakers, conform, check, translate, burn_in, remove"), "lang": s("language code: en, es, hi, fr, ja…"), "from": s("source language for translate"), "spec": s("netflix, bbc, social"), "translate": b("Whisper translation into English"), "path": s("file"), "format": s("srt or vtt"), "cue": any("subtitle number (1-based)"), "text": s("new text"), "start": num("seconds"), "end": num("seconds"), "speaker": s("character name"), "names": any("{subtitle number: name}"), "texts": any("translations, one per subtitle"), "style": s("text style for burn-in"), "kind": s("subtitles, sdh or forced")}),
        ),
        t(
            "dub",
            "Dubbing into other languages. Fields: action (characters, voice, make, list), lang, character, voice, pace, fit (span, phrases, natural).",
            json!({"action": s("characters, voice, make, list"), "lang": s("language code"), "character": s("name"), "voice": s("voice id of the speech service"), "pace": num("words a minute"), "fit": s("span, phrases or natural"), "cues": any("subtitle numbers"), "characters": any("names")}),
        ),
    ]
}

fn r1(v: f64) -> Value {
    if v.is_finite() { json!((v * 10.0).round() / 10.0) } else { Value::Null }
}

fn us(s: f64) -> i64 {
    (s * 1e6).round() as i64
}

fn keys_from(v: &Value) -> Result<desk::Keys, String> {
    let arr = v.as_array().ok_or("keys is a list of [seconds, value]")?;
    let mut out = Vec::new();
    for k in arr {
        let p = k.as_array().filter(|p| p.len() == 2).ok_or("each key is [seconds, value]")?;
        let (t, x) = (p[0].as_f64().ok_or("key time is a number")?, p[1].as_f64().ok_or("key value is a number")?);
        out.push((us(t), x as f32));
    }
    out.sort_by_key(|k| k.0);
    Ok(out)
}

fn target_of(s: &str) -> Result<LoudnessTarget, String> {
    LoudnessTarget::from_id(s).ok_or_else(|| format!("no spec \"{s}\"; specs: {}", LoudnessTarget::ALL.iter().map(|t| t.id()).collect::<Vec<_>>().join(", ")))
}

fn layout_of(s: &str) -> Result<Layout, String> {
    Layout::from_id(s).ok_or_else(|| format!("layout \"{s}\": stereo, 5.1 or 7.1"))
}

fn names(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

fn ids(v: Option<&Value>) -> Vec<u64> {
    match v {
        Some(Value::Number(n)) => n.as_u64().into_iter().collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).collect(),
        _ => Vec::new(),
    }
}

impl Session {
    /// Runs one of the sound department's tools.
    pub(super) fn sound_tool(&mut self, tool: &str, a: &Value) -> Result<Value, String> {
        match tool {
            "mix" => self.mix_tool(a),
            "loudness" => self.loudness_tool(a),
            "stems" => self.stems_tool(a),
            "dialogue" => self.dialogue_tool(a),
            "adr" => self.adr_tool(a),
            "subtitles" => self.subtitles_tool(a),
            "dub" => self.dub_tool(a),
            other => Err(format!("unknown sound tool {other}")),
        }
    }

    /// A clip named any way the AI names clips (id, `"Dialogue@4.2"`, a name).
    fn clip_ref(&self, v: &Value) -> Result<u64, String> {
        let mut o = json!({"clip": v});
        super::select::resolve_refs(&self.doc.project, &[], &mut o)?;
        o["clip"].as_u64().ok_or_else(|| format!("{v} names no clip"))
    }

    fn lane_id(&self, v: &Value) -> Result<u64, String> {
        super::craft::lane_of(&self.doc.project, v, None).map(|l| l as u64)
    }

    /// The whole desk as the AI reads it.
    pub(super) fn mix_show(&self) -> Value {
        let d = Desk::of(&self.doc.model);
        let seq = self.doc.model.sequence(self.doc.model.active_sequence);
        let bus_name = |id: Option<u64>| id.and_then(|i| d.bus(i)).map(|b| json!(b.name)).unwrap_or(Value::Null);
        let plan_lanes: Vec<studio_sound::mixer::LaneSpec> = seq.map(|s| s.lanes.iter().map(|l| desk::mix::lane_spec(l, false)).collect()).unwrap_or_default();
        let lanes: Vec<Value> = seq
            .map(|s| {
                s.lanes
                    .iter()
                    .zip(&plan_lanes)
                    .map(|(l, spec)| {
                        let mix = LaneMix::of(l);
                        let role = desk::lane_role(l);
                        // Where it actually goes: its bus, else its role's bus, else the master.
                        let to = mix.bus.and_then(|b| d.bus(b)).or_else(|| d.buses.iter().find(|b| b.kind() == role.bus_kind())).map(|b| b.name.clone()).unwrap_or_else(|| "master".into());
                        let p = spec.surround.at(0);
                        let mut v = json!({"lane": l.id.0, "name": l.name, "role": desk::role_id(role), "to": to, "volume": l.volume_db, "pan": l.pan});
                        if l.muted {
                            v["muted"] = json!(true);
                        }
                        if l.solo {
                            v["solo"] = json!(true);
                        }
                        if !mix.sends.is_empty() {
                            v["sends"] = json!(mix.sends.iter().map(|s| json!({"bus": bus_name(Some(s.bus)), "level": s.level_db, "pre": s.pre, "automated": !s.keys.is_empty()})).collect::<Vec<_>>());
                        }
                        if !mix.effects.is_empty() {
                            v["effects"] = json!(super::sound_ops::describe_effects(&mix.effects));
                        }
                        if !mix.volume_keys.is_empty() {
                            v["volume_keys"] = json!(mix.volume_keys.iter().map(|(t, x)| json!([secs(*t), x])).collect::<Vec<_>>());
                        }
                        if !mix.pan_keys.is_empty() {
                            v["pan_keys"] = json!(mix.pan_keys.iter().map(|(t, x)| json!([secs(*t), x])).collect::<Vec<_>>());
                        }
                        v["surround"] = json!({"x": p.x, "y": p.y, "z": p.z, "width": spec.surround.shape.width, "centre": spec.surround.shape.centre, "lfe": if spec.surround.shape.lfe_db.is_finite() { json!(spec.surround.shape.lfe_db) } else { Value::Null }, "moves": !(mix.surround.x_keys.is_empty() && mix.surround.y_keys.is_empty() && mix.surround.z_keys.is_empty())});
                        if let Some(l) = &mix.dub {
                            v["dub"] = json!(l);
                        }
                        if mix.adr {
                            v["adr"] = json!(true);
                        }
                        if mix.object {
                            v["object"] = json!(true);
                        }
                        v
                    })
                    .collect()
            })
            .unwrap_or_default();
        let buses: Vec<Value> = d.buses.iter().map(|b| json!({"bus": b.id, "name": b.name, "kind": b.kind, "volume": b.volume_db, "muted": b.muted, "solo": b.solo, "effects": super::sound_ops::describe_effects(&b.effects), "automated": !b.keys.is_empty()})).collect();
        let delivery = d.delivery.as_ref().map(|x| json!({"target": x.target, "gain": x.gain_db, "measured_lufs": x.measured_lufs, "true_peak": x.true_peak_dbtp, "range": x.range_lu}));
        json!({
            "layout": d.layout,
            "lanes": lanes,
            "buses": buses,
            "master": {"volume": d.master.volume_db, "ceiling": d.master.ceiling_db, "limiter": d.master.limiter, "ducking": d.master.ducking, "effects": super::sound_ops::describe_effects(&d.master.effects)},
            "delivery": delivery,
            "languages": d.languages.iter().map(|l| json!({"lang": l.lang, "name": l.name, "lane": l.lane})).collect::<Vec<_>>(),
            "original_language": d.original_language,
            "hint": if d.buses.is_empty() { "no buses yet: mix {action: setup} adds DX, MX, FX, BG and a Reverb return" } else { "" },
        })
    }

    fn mix_tool(&mut self, a: &Value) -> Result<Value, String> {
        let action = a.get("action").and_then(Value::as_str).unwrap_or("show");
        match action {
            "show" => Ok(self.mix_show()),
            "setup" => {
                let said = desk::edit_desk(&mut self.doc, "Film buses", |d| {
                    let added = d.add_film_buses();
                    Ok(if added.is_empty() { "the film buses were already there".into() } else { format!("added {}", added.join(", ")) })
                })?;
                Ok(json!({"ok": true, "said": said, "mix": self.mix_show()}))
            }
            "layout" => {
                let l = layout_of(a.get("layout").and_then(Value::as_str).ok_or("layout needs \"layout\": stereo, 5.1 or 7.1")?)?;
                desk::edit_desk(&mut self.doc, "Mix layout", |d| {
                    d.layout = l.id().into();
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true, "layout": l.id(), "channels": l.names()}))
            }
            "lane" => self.mix_lane(a),
            "bus" => self.mix_bus(a),
            "master" => {
                let mut said = Vec::new();
                let new_id = self.doc.alloc_id();
                desk::edit_desk(&mut self.doc, "Master", |d| {
                    if let Some(v) = f(a, "volume") {
                        d.master.volume_db = v.clamp(-60.0, 24.0) as f32;
                        said.push(format!("master {v:.1} dB"));
                    }
                    if let Some(v) = f(a, "ceiling") {
                        d.master.ceiling_db = v.clamp(-20.0, 0.0) as f32;
                        said.push(format!("ceiling {v:.1} dBTP"));
                    }
                    if let Some(v) = a.get("limiter").and_then(Value::as_bool) {
                        d.master.limiter = v;
                        said.push(format!("limiter {}", if v { "on" } else { "off" }));
                    }
                    if let Some(v) = a.get("ducking").and_then(Value::as_bool) {
                        d.master.ducking = v;
                        said.push(format!("ducking {}", if v { "on" } else { "off" }));
                    }
                    if let Some(k) = a.get("effect").and_then(Value::as_str) {
                        said.push(super::sound_ops::set_effect(&mut d.master.effects, k, a.get("params"), new_id)?);
                    }
                    if let Some(k) = a.get("remove_effect").and_then(Value::as_str) {
                        d.master.effects.retain(|e| e.kind != k);
                        said.push(format!("no {k}"));
                    }
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true, "said": said.join("; ")}))
            }
            "automate" => self.mix_automate(a),
            "meters" => {
                let at = f(a, "at").unwrap_or(0.0);
                let dur = f(a, "duration").unwrap_or(5.0).clamp(0.1, 600.0);
                Ok(self.meters_over(us(at), us(at + dur)))
            }
            other => Err(format!("mix action \"{other}\": show, setup, lane, bus, master, layout, automate, meters")),
        }
    }

    fn mix_lane(&mut self, a: &Value) -> Result<Value, String> {
        let lane = self.lane_id(a.get("lane").ok_or("mix lane needs \"lane\" (a name or id)")?)?;
        let d = Desk::of(&self.doc.model);
        let bus = match a.get("bus") {
            Some(Value::Null) => Some(None),
            Some(v) => Some(Some(d.find_bus(v).ok_or_else(|| format!("no bus {v}; buses: {}", d.buses.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")))?)),
            None => None,
        };
        let send = match a.get("send") {
            Some(v) => {
                let b = d.find_bus(v.get("bus").unwrap_or(v)).ok_or("send needs a bus that exists (mix setup adds a Reverb return)")?;
                Some(Send { bus: b, level_db: v.get("level").and_then(Value::as_f64).unwrap_or(-12.0) as f32, keys: Vec::new(), pre: v.get("pre").and_then(Value::as_bool).unwrap_or(false) })
            }
            None => None,
        };
        let unsend = a.get("unsend").map(|v| d.find_bus(v).ok_or("unsend: no such bus")).transpose()?;
        let new_id = self.doc.alloc_id();
        let mut said = Vec::new();
        desk::edit_lane(&mut self.doc, lane, "Lane mix", |mix, l| {
            if let Some(r) = a.get("role").and_then(Value::as_str) {
                let role = desk::role_from_id(r).ok_or_else(|| format!("role \"{r}\": dialogue, music, effects or ambience"))?;
                mix.role = Some(desk::role_id(role).into());
                said.push(format!("role {}", desk::role_id(role)));
            }
            if let Some(b) = bus {
                mix.bus = b;
                said.push(format!("into {}", b.and_then(|i| d.bus(i)).map(|x| x.name.clone()).unwrap_or_else(|| "its role's bus".into())));
            }
            if let Some(v) = f(a, "volume") {
                l.volume_db = v.clamp(-90.0, 24.0);
                mix.volume_keys.clear();
                said.push(format!("fader {v:.1} dB"));
            }
            if let Some(v) = f(a, "pan") {
                l.pan = v.clamp(-1.0, 1.0);
                mix.pan_keys.clear();
                said.push(format!("pan {v:.2}"));
            }
            if let Some(v) = a.get("mute").and_then(Value::as_bool) {
                l.muted = v;
            }
            if let Some(v) = a.get("solo").and_then(Value::as_bool) {
                l.solo = v;
            }
            if let Some(s) = send.clone() {
                said.push(format!("send to {} at {:.1} dB{}", d.bus(s.bus).map(|b| b.name.as_str()).unwrap_or("?"), s.level_db, if s.pre { " pre-fader" } else { "" }));
                mix.sends.retain(|x| x.bus != s.bus);
                mix.sends.push(s);
            }
            if let Some(b) = unsend {
                mix.sends.retain(|x| x.bus != b);
                said.push("send removed".into());
            }
            if let Some(k) = a.get("effect").and_then(Value::as_str) {
                said.push(super::sound_ops::set_effect(&mut mix.effects, k, a.get("params"), new_id)?);
            }
            if let Some(k) = a.get("remove_effect").and_then(Value::as_str) {
                mix.effects.retain(|e| e.kind != k);
                said.push(format!("no {k}"));
            }
            if let Some(sv) = a.get("surround") {
                let s = &mut mix.surround;
                let g = |k: &str| sv.get(k).and_then(Value::as_f64).map(|v| v as f32);
                if let Some(v) = g("x") {
                    s.x = v.clamp(-1.0, 1.0);
                    s.x_keys.clear();
                    s.placed = true;
                }
                if let Some(v) = g("y") {
                    s.y = v.clamp(-1.0, 1.0);
                    s.y_keys.clear();
                    s.placed = true;
                }
                if let Some(v) = g("z") {
                    s.z = v.clamp(0.0, 1.0);
                    s.z_keys.clear();
                }
                if let Some(v) = g("width") {
                    s.width = Some(v.clamp(0.0, 180.0));
                }
                if let Some(v) = sv.get("centre").or_else(|| sv.get("center")).and_then(Value::as_bool) {
                    s.centre = Some(v);
                }
                match sv.get("lfe") {
                    Some(Value::Null) => s.lfe_db = None,
                    Some(v) => s.lfe_db = v.as_f64().map(|x| x.clamp(-60.0, 10.0) as f32),
                    None => {}
                }
                if sv.get("reset").and_then(Value::as_bool) == Some(true) {
                    *s = desk::Surround::default();
                }
                said.push(format!("surround x {:.2} y {:.2}", s.x, s.y));
            }
            if let Some(o) = a.get("object").and_then(Value::as_bool) {
                mix.object = o;
                said.push(if o { "an object in ADM export".into() } else { "part of the bed".into() });
            }
            Ok(String::new())
        })?;
        Ok(json!({"ok": true, "lane": lane, "said": said.join("; ")}))
    }

    fn mix_bus(&mut self, a: &Value) -> Result<Value, String> {
        let new_id = self.doc.alloc_id();
        let mut said = Vec::new();
        let mut out_id = 0;
        desk::edit_desk(&mut self.doc, "Bus", |d| {
            let id = if let Some(add) = a.get("add") {
                let kind = add.get("kind").and_then(Value::as_str).and_then(BusKind::from_id).ok_or("add needs {kind: dialogue|music|effects|ambience|aux, name}")?;
                let id = d.next_bus_id();
                let name = add.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| kind.stem().into());
                d.buses.push(desk::Bus { id, kind: kind.id().into(), name: name.clone(), ..Default::default() });
                said.push(format!("bus {name} added"));
                id
            } else {
                let v = a.get("bus").ok_or("mix bus needs \"bus\" (a name, kind or id) or \"add\"")?;
                d.find_bus(v).ok_or_else(|| format!("no bus {v}; buses: {}", d.buses.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")))?
            };
            out_id = id;
            if a.get("remove").and_then(Value::as_bool) == Some(true) {
                d.buses.retain(|b| b.id != id);
                said.push("removed".into());
                return Ok(String::new());
            }
            let b = d.buses.iter_mut().find(|b| b.id == id).ok_or("that bus is gone")?;
            if let Some(v) = f(a, "volume") {
                b.volume_db = v.clamp(-90.0, 24.0) as f32;
                b.keys.clear();
                said.push(format!("{} {v:.1} dB", b.name));
            }
            if let Some(v) = a.get("mute").and_then(Value::as_bool) {
                b.muted = v;
            }
            if let Some(v) = a.get("solo").and_then(Value::as_bool) {
                b.solo = v;
            }
            if let Some(k) = a.get("effect").and_then(Value::as_str) {
                said.push(super::sound_ops::set_effect(&mut b.effects, k, a.get("params"), new_id)?);
            }
            if let Some(k) = a.get("remove_effect").and_then(Value::as_str) {
                b.effects.retain(|e| e.kind != k);
                said.push(format!("no {k}"));
            }
            Ok(String::new())
        })?;
        Ok(json!({"ok": true, "bus": out_id, "said": said.join("; ")}))
    }

    fn mix_automate(&mut self, a: &Value) -> Result<Value, String> {
        let param = a.get("param").and_then(Value::as_str).unwrap_or("volume");
        let clear = a.get("clear").and_then(Value::as_bool) == Some(true);
        let keys = if clear { Vec::new() } else { keys_from(a.get("keys").ok_or("automate needs \"keys\": [[seconds, value], ...] (or clear: true)")?)? };
        let n = keys.len();
        if let Some(lv) = a.get("lane") {
            let lane = self.lane_id(lv)?;
            let d = Desk::of(&self.doc.model);
            let send_bus = if param == "send" { Some(d.find_bus(a.get("bus").ok_or("send automation needs \"bus\"")?).ok_or("no such bus")?) } else { None };
            desk::edit_lane(&mut self.doc, lane, "Automation", |mix, _| {
                match param {
                    "volume" | "fader" | "gain" => mix.volume_keys = keys,
                    "pan" | "balance" => mix.pan_keys = keys,
                    "x" => mix.surround.x_keys = keys,
                    "y" => mix.surround.y_keys = keys,
                    "z" | "height" => mix.surround.z_keys = keys,
                    "send" => {
                        let b = send_bus.unwrap_or_default();
                        let s = mix.sends.iter_mut().find(|s| s.bus == b).ok_or("the lane has no send to that bus: add it with mix lane {send}")?;
                        s.keys = keys;
                    }
                    other => return Err(format!("param \"{other}\": volume, pan, x, y, z or send")),
                }
                Ok(String::new())
            })?;
            return Ok(json!({"ok": true, "lane": lane, "param": param, "keys": n}));
        }
        let bv = a.get("bus").ok_or("automate needs \"lane\" or \"bus\"")?.clone();
        desk::edit_desk(&mut self.doc, "Automation", |d| {
            let id = d.find_bus(&bv).ok_or("no such bus")?;
            let b = d.buses.iter_mut().find(|b| b.id == id).ok_or("no such bus")?;
            if !matches!(param, "volume" | "fader" | "gain") {
                return Err("buses automate their volume".into());
            }
            b.keys = keys;
            Ok(String::new())
        })?;
        Ok(json!({"ok": true, "param": param, "keys": n}))
    }

    /// Peak and loudness over a stretch, per bus and per channel.
    fn meters_over(&self, t0: i64, t1: i64) -> Value {
        let plan = crate::sound_bridge::plan(&self.doc.model);
        let layout = plan.layout;
        let bus_names: Vec<(u64, String)> = plan.buses.iter().map(|b| (b.id, b.name.clone())).collect();
        let lane_names: Vec<(u64, String)> = self.doc.model.sequence(self.doc.model.active_sequence).map(|s| s.lanes.iter().map(|l| (l.id.0, l.name.clone())).collect()).unwrap_or_default();
        let rate = crate::sound_bridge::RATE;
        let mut mx = studio_sound::Mixer::new(rate, 1024, studio_sound::Quality::High, plan);
        mx.seek(t0);
        let frames = studio_sound::micros_to_frames(t1 - t0, rate).max(0) as usize;
        let mut buf = studio_sound::MultiBuf::new(layout.channels(), 1024);
        let (mut bus_peak, mut lane_peak, mut ch_peak) = (vec![0.0f32; bus_names.len()], vec![0.0f32; lane_names.len()], vec![0.0f32; layout.channels()]);
        let mut done = 0;
        let mut momentary_max = f64::NEG_INFINITY;
        while done < frames {
            let n = (frames - done).min(1024);
            buf.set_frames(n);
            mx.render_multi(&mut buf);
            let m = mx.meters();
            for (i, p) in m.bus_peaks.iter().enumerate() {
                bus_peak[i] = bus_peak[i].max(*p);
            }
            for (i, p) in m.lane_peaks.iter().enumerate() {
                if let Some(x) = lane_peak.get_mut(i) {
                    *x = x.max(*p);
                }
            }
            for (i, p) in m.channel_peaks.iter().enumerate() {
                ch_peak[i] = ch_peak[i].max(*p);
            }
            momentary_max = momentary_max.max(m.momentary_lufs);
            done += n;
        }
        let db = |p: f32| r1(20.0 * (p.max(1e-9) as f64).log10());
        let r = mx.loudness().report();
        json!({
            "from": secs(t0), "to": secs(t1), "layout": layout.id(),
            "lanes": lane_names.iter().zip(&lane_peak).map(|((id, n), p)| json!({"lane": id, "name": n, "peak_db": db(*p)})).collect::<Vec<_>>(),
            "buses": bus_names.iter().zip(&bus_peak).map(|((id, n), p)| json!({"bus": id, "name": n, "peak_db": db(*p)})).collect::<Vec<_>>(),
            "channels": layout.names().iter().zip(&ch_peak).map(|(n, p)| json!({"channel": n, "peak_db": db(*p)})).collect::<Vec<_>>(),
            "integrated_lufs": r1(r.integrated_lufs), "short_term_max_lufs": r1(r.short_term_max_lufs), "momentary_max_lufs": r1(momentary_max), "true_peak_dbtp": r1(r.true_peak_dbtp), "range_lu": r1(r.range_lu),
        })
    }

    fn loudness_tool(&mut self, a: &Value) -> Result<Value, String> {
        let d = Desk::of(&self.doc.model);
        let target = match a.get("target").and_then(Value::as_str) {
            Some(s) => target_of(s)?,
            None => d.delivery.as_ref().and_then(|x| LoudnessTarget::from_id(&x.target)).unwrap_or(LoudnessTarget::Streaming),
        };
        let layout = match a.get("layout").and_then(Value::as_str) {
            Some(s) => layout_of(s)?,
            None => d.layout(),
        };
        let normalised = a.get("normalise").or_else(|| a.get("normalize")).and_then(Value::as_bool) == Some(true);
        let r = if normalised {
            if layout != d.layout() {
                desk::edit_desk(&mut self.doc, "Mix layout", |d| {
                    d.layout = layout.id().into();
                    Ok(String::new())
                })?;
            }
            desk::render::normalise(&mut self.doc, target)?
        } else {
            desk::render::measure(&self.doc.model, target, layout)
        };
        let (ok, why) = desk::render::verdict(target, &r);
        let d = Desk::of(&self.doc.model);
        Ok(json!({
            "spec": target.label(), "layout": layout.id(), "passes": ok, "problems": why,
            "integrated_lufs": r1(r.report.integrated_lufs), "judged_lufs": r1(r.lufs), "dialogue_gated": r.dialogue_gated,
            "speech_share": (r.speech_share * 100.0).round() / 100.0,
            "short_term_max_lufs": r1(r.report.short_term_max_lufs), "momentary_max_lufs": r1(r.report.momentary_max_lufs),
            "range_lu": r1(r.report.range_lu), "true_peak_dbtp": r1(r.report.true_peak_dbtp),
            "master_gain": d.delivery.as_ref().map(|x| x.gain_db), "normalised": normalised,
        }))
    }

    fn project_title(&self) -> String {
        let n = self.doc.project.name.trim();
        if n.is_empty() { "Zornov".into() } else { n.to_owned() }
    }

    fn stems_tool(&mut self, a: &Value) -> Result<Value, String> {
        let d = Desk::of(&self.doc.model);
        let layout = match a.get("layout").and_then(Value::as_str) {
            Some(s) => layout_of(s)?,
            None => d.layout(),
        };
        let dir = match a.get("path").and_then(Value::as_str) {
            Some(p) => std::path::PathBuf::from(p),
            None => self.home.clone().map(|h| m::file::ProjectFolder::new(h).export_dir(m::file::ExportKind::Stems)).ok_or("stems needs \"path\" (a folder) when the project has no folder")?,
        };
        let title = self.project_title();
        let written = desk::render::export_stems(&self.doc.model, &dir, layout, a.get("separate_bg").and_then(Value::as_bool) == Some(true), &title)?;
        let mut out = json!({"ok": true, "layout": layout.id(), "files": written.iter().map(|w| json!({"stem": w.name, "path": w.path.display().to_string(), "peak_db": w.peak_dbfs})).collect::<Vec<_>>()});
        if a.get("adm").and_then(Value::as_bool) == Some(true) {
            let path = dir.join(format!("{title} ADM.wav"));
            let (bed, objects) = desk::render::export_adm(&self.doc.model, &path, layout, &title)?;
            out["adm"] = json!({"path": path.display().to_string(), "bed_channels": bed, "objects": objects, "note": if objects == 0 { "no lane is marked as an object (mix lane {object: true})" } else { "" }});
        }
        Ok(out)
    }

    fn dialogue_tool(&mut self, a: &Value) -> Result<Value, String> {
        match a.get("action").and_then(Value::as_str).unwrap_or("level") {
            "level" => {
                let clips = ids(a.get("clips").or_else(|| a.get("clip")));
                if clips.is_empty() {
                    return Err("dialogue level needs \"clips\"".into());
                }
                let mut p = studio_sound::dialogue::LevelerParams::default();
                if let Some(t) = f(a, "target") {
                    p.target_lufs = t.clamp(-40.0, -10.0) as f32;
                }
                if let Some(v) = f(a, "max_boost") {
                    p.max_boost_db = v.clamp(0.0, 30.0) as f32;
                }
                if let Some(v) = f(a, "max_cut") {
                    p.max_cut_db = v.clamp(0.0, 30.0) as f32;
                }
                self.doc.begin_batch("Level dialogue");
                let mut out = Vec::new();
                for c in clips {
                    match desk::dialogue::level(&mut self.doc, c, p) {
                        Ok(ph) => out.push(json!({"clip": c, "phrases": ph.iter().map(|x| json!({"from": secs(x.start), "to": secs(x.end), "was_lufs": r1(x.lufs as f64), "gain": r1(x.gain_db as f64)})).collect::<Vec<_>>()})),
                        Err(e) => {
                            self.doc.rollback_batch();
                            return Err(e);
                        }
                    }
                }
                self.doc.commit_batch();
                Ok(json!({"ok": true, "target_lufs": p.target_lufs, "clips": out, "note": "phrase times are into each clip; the gains are the clips' volume keys"}))
            }
            "match" => {
                let take = self.clip_ref(a.get("clip").ok_or("dialogue match needs \"clip\" (the take)")?)?;
                let reference = self.clip_ref(a.get("reference").ok_or("dialogue match needs \"reference\" (the clip to sound like)")?)?;
                let vm = desk::dialogue::match_clip(&mut self.doc, take, reference)?;
                Ok(json!({"ok": true, "eq": {"bass": r1(vm.eq_db[0] as f64), "low_mid": r1(vm.eq_db[1] as f64), "mid": r1(vm.eq_db[2] as f64), "presence": r1(vm.eq_db[3] as f64), "air": r1(vm.eq_db[4] as f64)}, "gain": r1(vm.gain_db as f64), "room": if vm.reverb_mix > 0.0 { json!({"room": "small_room", "mix": r1(vm.reverb_mix as f64)}) } else { Value::Null }}))
            }
            other => Err(format!("dialogue action \"{other}\": level or match")),
        }
    }

    fn adr_tool(&mut self, a: &Value) -> Result<Value, String> {
        let fps = self.doc.project.fps;
        let list = |d: &Desk| json!(d.adr.iter().map(|c| json!({"cue": c.id, "character": c.character, "in": desk::adr::timecode(c.start, fps), "out": desk::adr::timecode(c.end, fps), "start": secs(c.start), "end": secs(c.end), "text": c.text, "reason": c.reason, "status": c.status, "take": c.take, "notes": c.notes})).collect::<Vec<_>>());
        match a.get("action").and_then(Value::as_str).unwrap_or("list") {
            "list" => Ok(json!({"cues": list(&Desk::of(&self.doc.model))})),
            "add" => {
                let start = f(a, "start").ok_or("adr add needs \"start\" and \"end\" (seconds)")?;
                let end = f(a, "end").ok_or("adr add needs \"end\"")?;
                if end <= start {
                    return Err("end must be after start".into());
                }
                let cue = AdrCue { character: a.get("character").and_then(Value::as_str).unwrap_or("").into(), start: us(start), end: us(end), text: a.get("text").and_then(Value::as_str).unwrap_or("").into(), reason: a.get("reason").and_then(Value::as_str).unwrap_or("").into(), notes: a.get("notes").and_then(Value::as_str).unwrap_or("").into(), ..Default::default() };
                let mut id = 0;
                desk::edit_desk(&mut self.doc, "ADR cue", |d| {
                    id = desk::adr::add_cue(d, cue);
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true, "cue": id}))
            }
            "from_subtitles" => {
                let d = Desk::of(&self.doc.model);
                let lang = a.get("lang").and_then(Value::as_str).unwrap_or(&d.original_language).to_owned();
                let cues = desk::adr::cues_from_subtitles(&d, &lang, f(a, "from").unwrap_or(0.0) as f32, f(a, "to").unwrap_or(f64::MAX) as f32, &names(a.get("characters")), a.get("reason").and_then(Value::as_str).unwrap_or(""));
                if cues.is_empty() {
                    return Err(format!("no {lang} subtitles in that range (subtitles make first)"));
                }
                let n = cues.len();
                desk::edit_desk(&mut self.doc, "ADR cues", |d| {
                    for c in cues {
                        desk::adr::add_cue(d, c);
                    }
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true, "added": n, "cues": list(&Desk::of(&self.doc.model))}))
            }
            "set" => {
                let id = a.get("cue").and_then(Value::as_u64).ok_or("adr set needs \"cue\"")? as u32;
                desk::edit_desk(&mut self.doc, "ADR cue", |d| {
                    let c = d.adr.iter_mut().find(|c| c.id == id).ok_or_else(|| format!("no cue {id}"))?;
                    if let Some(s) = a.get("status").and_then(Value::as_str) {
                        c.status = s.into();
                    }
                    if let Some(s) = a.get("notes").and_then(Value::as_str) {
                        c.notes = s.into();
                    }
                    if let Some(s) = a.get("text").and_then(Value::as_str) {
                        c.text = s.into();
                    }
                    if let Some(s) = a.get("character").and_then(Value::as_str) {
                        c.character = s.into();
                    }
                    if let Some(v) = f(a, "start") {
                        c.start = us(v);
                    }
                    if let Some(v) = f(a, "end") {
                        c.end = us(v);
                    }
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true, "cues": list(&Desk::of(&self.doc.model))}))
            }
            "remove" => {
                let id = a.get("cue").and_then(Value::as_u64).ok_or("adr remove needs \"cue\"")? as u32;
                desk::edit_desk(&mut self.doc, "Remove ADR cue", |d| {
                    d.adr.retain(|c| c.id != id);
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true}))
            }
            "sheet" => {
                let d = Desk::of(&self.doc.model);
                let csv = desk::adr::cue_sheet(&d, fps, &self.project_title());
                if let Some(p) = a.get("path").and_then(Value::as_str) {
                    std::fs::write(p, &csv).map_err(|e| format!("{p}: {e}"))?;
                    return Ok(json!({"ok": true, "path": p, "cues": d.adr.len()}));
                }
                Ok(json!({"csv": csv}))
            }
            "beeps" => {
                let path = a.get("path").and_then(Value::as_str).ok_or("adr beeps needs \"path\" for the WAV")?;
                let b = desk::adr::beeps(crate::sound_bridge::RATE, 4.0, fps);
                let wav = studio_media::wav::Wav { sample_rate: crate::sound_bridge::RATE, channels: 1, samples: b };
                std::fs::write(path, studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("{path}: {e}"))?;
                Ok(json!({"ok": true, "path": path, "line_starts_at": 4.0, "note": "play from 4 s before the cue's in point; the line starts where the fourth beep would be"}))
            }
            "take" => {
                let id = a.get("cue").and_then(Value::as_u64).ok_or("adr take needs \"cue\"")? as u32;
                let file = a.get("file").and_then(Value::as_str).ok_or("adr take needs \"file\" (the recorded take)")?;
                let home = self.home.clone();
                let r = desk::adr::take(&mut self.doc, home.as_deref(), id, std::path::Path::new(file), a.get("warp").and_then(Value::as_bool) == Some(true), a.get("match").and_then(Value::as_bool).unwrap_or(true), a.get("replace").and_then(Value::as_bool).unwrap_or(true))?;
                Ok(json!({"ok": true, "clip": r.clip, "at": (r.start * 1000.0).round() / 1000.0, "shift": (r.shift * 1000.0).round() / 1000.0, "sync_confidence": (r.confidence as f64 * 100.0).round() / 100.0, "warped": r.warped, "matched": r.matched.map(|m| json!({"gain": r1(m.gain_db as f64), "eq": m.eq_db, "room_mix": m.reverb_mix})), "production_dipped": r.replaced}))
            }
            other => Err(format!("adr action \"{other}\": list, add, from_subtitles, set, remove, sheet, beeps, take")),
        }
    }

    fn subtitles_tool(&mut self, a: &Value) -> Result<Value, String> {
        let d = Desk::of(&self.doc.model);
        let fps = desk::subs::fps(&self.doc);
        let lang = a.get("lang").and_then(Value::as_str).map(str::to_owned);
        let spec_for = |lang: &str| -> Result<studio_text::subtitles::SubtitleSpec, String> {
            let name = a.get("spec").and_then(Value::as_str).unwrap_or("netflix");
            studio_text::subtitles::SubtitleSpec::preset(name, lang, fps).ok_or_else(|| format!("spec \"{name}\": netflix, bbc or social"))
        };
        let summary = |t: &desk::SubTrack| json!({"lang": t.lang, "name": t.name, "kind": t.kind, "subtitles": t.cues.len(), "first": t.cues.first().map(|c| c.text.clone())});
        let store = |s: &mut Session, t: desk::SubTrack| -> Result<(), String> {
            desk::edit_desk(&mut s.doc, "Subtitles", |d| {
                d.subtitles.retain(|x| !(x.lang == t.lang && x.kind == t.kind));
                d.subtitles.push(t);
                Ok(String::new())
            })
            .map(|_| ())
        };
        let need = |d: &Desk, l: &Option<String>| -> Result<desk::SubTrack, String> {
            let l = l.clone().unwrap_or_else(|| d.original_language.clone());
            d.subtitles_of(&l).cloned().ok_or_else(|| format!("no {l} subtitles; tracks: {}", d.subtitles.iter().map(|t| t.lang.as_str()).collect::<Vec<_>>().join(", ")))
        };
        match a.get("action").and_then(Value::as_str).unwrap_or("list") {
            "list" => Ok(json!({"tracks": d.subtitles.iter().map(summary).collect::<Vec<_>>()})),
            "make" => {
                let translate = a.get("translate").and_then(Value::as_bool) == Some(true);
                let (words, heard) = desk::subs::transcribe(&self.doc.model, if translate { None } else { lang.as_deref() }, translate)?;
                let l = if translate { "en".to_owned() } else { lang.clone().unwrap_or(heard) };
                let shots = self.shot_changes();
                let mut t = desk::subs::from_words(words, &l, &spec_for(&l)?, &shots);
                if let Some(k) = a.get("kind").and_then(Value::as_str) {
                    t.kind = k.into();
                }
                let out = summary(&t);
                if d.subtitles.is_empty() && !translate {
                    desk::edit_desk(&mut self.doc, "Original language", |d| {
                        d.original_language = l.clone();
                        Ok(String::new())
                    })?;
                }
                store(self, t)?;
                Ok(json!({"ok": true, "track": out}))
            }
            "from_captions" => {
                let l = lang.clone().unwrap_or_else(|| d.original_language.clone());
                let t = desk::subs::from_caption_clips(&self.doc, &l).ok_or("no caption clips on the timeline (a lane named Captions)")?;
                let out = summary(&t);
                store(self, t)?;
                Ok(json!({"ok": true, "track": out}))
            }
            "import" => {
                let path = a.get("path").and_then(Value::as_str).ok_or("subtitles import needs \"path\"")?;
                let l = lang.clone().ok_or("subtitles import needs \"lang\"")?;
                let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                let t = desk::subs::read(&text, &l)?;
                let out = summary(&t);
                store(self, t)?;
                Ok(json!({"ok": true, "track": out}))
            }
            "export" => {
                let t = need(&d, &lang)?;
                let vtt = a.get("format").and_then(Value::as_str).is_some_and(|f| f.eq_ignore_ascii_case("vtt"));
                let body = desk::subs::write(&t, vtt);
                match a.get("path").and_then(Value::as_str) {
                    Some(p) => {
                        std::fs::write(p, &body).map_err(|e| format!("{p}: {e}"))?;
                        Ok(json!({"ok": true, "path": p, "subtitles": t.cues.len()}))
                    }
                    None => Ok(json!({"text": body})),
                }
            }
            "show" => {
                let t = need(&d, &lang)?;
                Ok(json!({"lang": t.lang, "name": t.name, "kind": t.kind, "subtitles": t.cues.iter().enumerate().map(|(i, c)| json!({"cue": i + 1, "start": (c.start * 1000.0).round() / 1000.0, "end": (c.end * 1000.0).round() / 1000.0, "text": c.text, "speaker": c.speaker})).collect::<Vec<_>>()}))
            }
            "edit" => {
                let mut t = need(&d, &lang)?;
                let n = a.get("cue").and_then(Value::as_u64).ok_or("subtitles edit needs \"cue\" (1-based)")? as usize;
                let c = t.cues.get_mut(n.wrapping_sub(1)).ok_or_else(|| format!("no subtitle {n}"))?;
                if let Some(s) = a.get("text").and_then(Value::as_str) {
                    c.text = s.into();
                }
                if let Some(v) = f(a, "start") {
                    c.start = v as f32;
                }
                if let Some(v) = f(a, "end") {
                    c.end = v as f32;
                }
                if let Some(s) = a.get("speaker").and_then(Value::as_str) {
                    c.speaker = Some(s.into()).filter(|s: &String| !s.is_empty());
                }
                store(self, t)?;
                Ok(json!({"ok": true}))
            }
            "speakers" => {
                let mut t = need(&d, &lang)?;
                let map = a.get("names").and_then(Value::as_object).ok_or("subtitles speakers needs \"names\": {\"1\": \"MAYA\", \"2\": \"RAJ\"}")?;
                for (k, v) in map {
                    let n: usize = k.parse().map_err(|_| format!("\"{k}\" is not a subtitle number"))?;
                    if let Some(c) = t.cues.get_mut(n.wrapping_sub(1)) {
                        c.speaker = v.as_str().map(str::to_owned);
                    }
                }
                store(self, t)?;
                Ok(json!({"ok": true}))
            }
            "conform" => {
                let mut t = need(&d, &lang)?;
                let spec = spec_for(&t.lang)?;
                let shots = self.shot_changes();
                let n = desk::subs::conform(&mut t, &spec, &shots);
                let problems = desk::subs::check(&t, &spec);
                store(self, t)?;
                Ok(json!({"ok": true, "moved": n, "still_wrong": problems}))
            }
            "check" => {
                let t = need(&d, &lang)?;
                let problems = desk::subs::check(&t, &spec_for(&t.lang)?);
                Ok(json!({"lang": t.lang, "passes": problems.is_empty(), "problems": problems}))
            }
            "translate" => {
                let to = lang.clone().ok_or("subtitles translate needs \"lang\" (the language to translate into)")?;
                let from = a.get("from").and_then(Value::as_str).map(str::to_owned);
                let src = need(&d, &from.or(Some(d.original_language.clone())))?;
                let texts = match a.get("texts") {
                    Some(Value::Array(v)) => Some(v.iter().map(|x| x.as_str().unwrap_or("").to_owned()).collect()),
                    _ => None,
                };
                let t = desk::subs::translate(&src, &to, texts)?;
                let problems = desk::subs::check(&t, &spec_for(&to)?);
                let out = summary(&t);
                store(self, t)?;
                Ok(json!({"ok": true, "track": out, "reading_problems": problems}))
            }
            "burn_in" => {
                let t = need(&d, &lang)?;
                let style = a.get("style").and_then(Value::as_str);
                let style_id = match style {
                    Some(name) => Some(studio_text::styles::catalogue()[self.resolve(name, &[crate::services::library::Panel::Text])?.1].id.clone()),
                    None => None,
                };
                let (lane, n) = desk::subs::burn_in(&mut self.doc, &t, style_id.as_deref())?;
                Ok(json!({"ok": true, "lane": lane, "clips": n}))
            }
            "remove" => {
                let l = lang.clone().ok_or("subtitles remove needs \"lang\"")?;
                desk::edit_desk(&mut self.doc, "Remove subtitles", |d| {
                    d.subtitles.retain(|t| t.lang != l);
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true}))
            }
            other => Err(format!("subtitles action \"{other}\": list, make, from_captions, import, export, show, edit, speakers, conform, check, translate, burn_in, remove")),
        }
    }

    /// Shot changes on the timeline (seconds): where spine clips meet.
    fn shot_changes(&self) -> Vec<f32> {
        let p = &self.doc.project;
        let spine = p.lanes.iter().find(|l| l.kind == crate::view::LaneKind::Spine).map(|l| l.id);
        let mut v: Vec<f32> = p.clips.iter().filter(|c| Some(c.lane) == spine && c.start > 0).map(|c| c.start as f32 / 1e6).collect();
        v.sort_by(f32::total_cmp);
        v
    }

    fn dub_tool(&mut self, a: &Value) -> Result<Value, String> {
        let d = Desk::of(&self.doc.model);
        match a.get("action").and_then(Value::as_str).unwrap_or("list") {
            "list" => Ok(json!({"original": d.original_language, "languages": d.languages.iter().map(|l| json!({"lang": l.lang, "name": l.name, "lane": l.lane, "subtitles": d.subtitles_of(&l.lang).map(|t| t.cues.len())})).collect::<Vec<_>>()})),
            "characters" => {
                let mut who: std::collections::BTreeMap<String, usize> = Default::default();
                for t in &d.subtitles {
                    for c in &t.cues {
                        if let Some(s) = &c.speaker {
                            *who.entry(s.clone()).or_default() += 1;
                        }
                    }
                }
                for k in d.characters.keys() {
                    who.entry(k.clone()).or_default();
                }
                Ok(json!({"characters": who.iter().map(|(k, n)| json!({"name": k, "lines": n, "voices": d.characters.get(k).map(|c| json!(c.voices)).unwrap_or(json!({})), "pace": d.characters.get(k).and_then(|c| c.pace)})).collect::<Vec<_>>(), "service": crate::agent::voice_service::VoiceSettings::load().provider.name(), "hint": "set speakers on subtitles (subtitles speakers) so each line has its character"}))
            }
            "voice" => {
                let who = a.get("character").and_then(Value::as_str).ok_or("dub voice needs \"character\"")?.to_owned();
                let lang = a.get("lang").and_then(Value::as_str).ok_or("dub voice needs \"lang\"")?.to_owned();
                let voice = a.get("voice").and_then(Value::as_str).map(str::to_owned);
                desk::edit_desk(&mut self.doc, "Character voice", |d| {
                    let c = d.characters.entry(who.clone()).or_default();
                    match voice {
                        Some(v) if !v.is_empty() => {
                            c.voices.insert(lang.clone(), v);
                        }
                        _ => {
                            c.voices.remove(&lang);
                        }
                    }
                    if let Some(p) = f(a, "pace") {
                        c.pace = Some(p.clamp(80.0, 300.0) as f32);
                    }
                    Ok(String::new())
                })?;
                Ok(json!({"ok": true}))
            }
            "make" => {
                let lang = a.get("lang").and_then(Value::as_str).ok_or("dub make needs \"lang\"")?.to_owned();
                let fit = match a.get("fit").and_then(Value::as_str) {
                    Some(s) => desk::dub::Fit::from_id(s).ok_or("fit: span, phrases or natural")?,
                    None => desk::dub::Fit::Phrases,
                };
                let cues: Vec<usize> = ids(a.get("cues")).into_iter().map(|x| x as usize).collect();
                let home = self.home.clone();
                let lines = desk::dub::dub(&mut self.doc, home.as_deref(), &lang, fit, &cues, &names(a.get("characters")))?;
                let long: Vec<Value> = lines.iter().filter_map(|l| l.long_by.map(|x| json!({"cue": l.cue, "too_long_by": (x * 100.0).round() / 100.0}))).collect();
                Ok(json!({
                    "ok": true, "lang": lang, "lines": lines.len(),
                    "placed": lines.iter().map(|l| json!({"cue": l.cue, "character": l.character, "from": (l.start * 1000.0).round() / 1000.0, "to": (l.end * 1000.0).round() / 1000.0, "stretch": (l.stretch * 100.0).round() / 100.0, "voice": l.voice})).collect::<Vec<_>>(),
                    "reword": long,
                    "note": "exports now carry this language as its own audio track (M&E + dub)",
                }))
            }
            other => Err(format!("dub action \"{other}\": list, characters, voice, make")),
        }
    }
}
