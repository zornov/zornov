// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Prints the whole catalogue as Markdown tables:
//! `cargo run -p studio-fx --example catalogue`.

use studio_fx::params::{ParamDef, ParamKind, ParamValue};
use studio_fx::{effects, motion, speed, transitions};

fn param_list(params: &[ParamDef]) -> String {
    params
        .iter()
        .map(|d| {
            let range = match d.kind {
                ParamKind::Float { min, max } => format!(" {min}..{max}"),
                ParamKind::Int { min, max } => format!(" {min}..{max}"),
                ParamKind::Choice(o) => format!(" [{}]", o.join(" / ")),
                _ => String::new(),
            };
            let default = match d.default {
                ParamValue::Float(f) => format!("{f}"),
                ParamValue::Int(i) => format!("{i}"),
                ParamValue::Bool(b) => format!("{b}"),
                ParamValue::Choice(c) => match d.kind {
                    ParamKind::Choice(o) => o[c as usize].to_string(),
                    _ => c.to_string(),
                },
                ParamValue::Colour(c) => format!("rgba({}, {}, {}, {})", c[0], c[1], c[2], c[3]),
                ParamValue::Point(p) => format!("({}, {})", p[0], p[1]),
            };
            let anim = if d.animatable { "" } else { ", fixed" };
            if matches!(d.kind, ParamKind::Choice(o) if o.len() > 8) {
                format!("`{}` = {default}{anim}", d.id)
            } else {
                format!("`{}`{range} = {default}{anim}", d.id)
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn main() {
    let mut cat = None;
    println!("### Effects\n");
    for e in effects::all() {
        if cat != Some(e.category) {
            cat = Some(e.category);
            println!("\n**{}**\n\n| id | name | GPU | parameters |\n|---|---|---|---|", e.category.label());
        }
        println!("| `{}` | {} | {} | {} |", e.id, e.name, if e.wgsl.is_some() { "yes" } else { "" }, param_list(e.params));
    }
    println!("\n### Transitions\n\n| id | name | GPU | parameters |\n|---|---|---|---|");
    for t in transitions::all() {
        println!("| `{}` | {} | {} | {} |", t.id, t.name, if t.wgsl.is_some() { "yes" } else { "" }, param_list(t.params));
    }
    println!("\n### Motion presets\n\n| id | name | kind | parameters |\n|---|---|---|---|");
    for m in motion::all() {
        println!("| `{}` | {} | {:?} | {} |", m.id, m.name, m.kind, param_list(m.params));
    }
    println!("\n### Speed presets\n\n| id | name | keys (position: speed) |\n|---|---|---|");
    for p in speed::PRESETS {
        let keys: Vec<String> = p.keys.iter().map(|k| format!("{}: {}x", k.t, k.speed)).collect();
        println!("| `{}` | {} | {} |", p.id, p.name, keys.join(", "));
    }
}
