// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `studio-quality`: the quality benchmark and critics, headless.
//!
//! ```text
//! studio-quality bench [--out DIR] [--only TEXT] [--raw] [--outside DIR] [--pictures DIR] [--label NAME] [--history FILE]
//! studio-quality model FILE [--class prop|character|…] [--finish OUT.glb]
//! studio-quality picture FILE.png [--text "what it should say" --read "what it says"]
//! studio-quality vector FILE.svg [--reference FILE.png]
//! ```

use std::path::PathBuf;
use studio_quality::{bench, score::Class};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bench") => run_bench(&args),
        Some("model") => model(&args),
        Some("picture") => picture(&args),
        Some("vector") => vector(&args),
        _ => Err("usage: studio-quality bench|model|picture|vector … (see the source's header)".into()),
    };
    if let Err(e) = result {
        eprintln!("studio-quality: {e}");
        std::process::exit(1);
    }
}

fn run_bench(args: &[String]) -> Result<(), String> {
    let out = PathBuf::from(arg(args, "--out").unwrap_or_else(|| "quality-bench".into()));
    let s = bench::Settings {
        out: out.clone(),
        outside_models: arg(args, "--outside").map(PathBuf::from),
        pictures: arg(args, "--pictures").map(PathBuf::from),
        only: arg(args, "--only"),
        raw_only: args.iter().any(|a| a == "--raw"),
    };
    let results = bench::run(&s)?;
    let md = bench::report_markdown(&results);
    std::fs::write(out.join("report.md"), &md).map_err(|e| e.to_string())?;
    std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&results).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if let Some(h) = arg(args, "--history") {
        bench::record(std::path::Path::new(&h), &arg(args, "--label").unwrap_or_else(|| "run".into()), &results)?;
    }
    println!("{}", serde_json::to_string_pretty(&bench::summary(&results)).unwrap_or_default());
    Ok(())
}

fn model(args: &[String]) -> Result<(), String> {
    let file = args.get(1).ok_or("model FILE")?;
    let class = Class::parse(&arg(args, "--class").unwrap_or_else(|| "prop".into())).ok_or("class: prop, character, creature, vehicle, architecture, environment, product")?;
    let m = studio_mesh::import::load(std::path::Path::new(file), 400.0)?.mesh;
    let r = studio_quality::critique_model(&m, class, None);
    println!("{}", r.summary());
    if let Some(out) = arg(args, "--finish") {
        let f = studio_quality::production::finish(&m, None, &studio_quality::production::Options::new(class));
        std::fs::write(&out, studio_mesh::export::glb(&f.mesh)).map_err(|e| format!("{out}: {e}"))?;
        println!("finished: {}", f.report.summary());
    }
    Ok(())
}

fn picture(args: &[String]) -> Result<(), String> {
    let file = args.get(1).ok_or("picture FILE.png")?;
    let (w, h, rgba) = studio_quality::raster::read_png(std::path::Path::new(file))?;
    let q = studio_quality::image::analyse(w, h, &rgba);
    println!("{}", serde_json::to_string_pretty(&q).unwrap_or_default());
    for f in studio_quality::image::faults(&q) {
        println!("fault: {f}");
    }
    if let (Some(t), Some(r)) = (arg(args, "--text"), arg(args, "--read")) {
        println!("text accuracy {:.3}", studio_quality::image::text_accuracy(&t, &r));
    }
    Ok(())
}

fn vector(args: &[String]) -> Result<(), String> {
    let file = args.get(1).ok_or("vector FILE.svg")?;
    let doc = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    let (shapes, view) = studio_quality::vector::parse_svg(&doc);
    let mut q = studio_quality::vector::analyse(&shapes, view);
    if let Some(r) = arg(args, "--reference") {
        let (w, h, rgba) = studio_quality::raster::read_png(std::path::Path::new(&r))?;
        let (iou, err) = studio_quality::vector::compare(&shapes, view, w, h, &rgba);
        q.fill_iou = Some(iou);
        q.colour_error = Some(err);
    }
    println!("{}", serde_json::to_string_pretty(&q).unwrap_or_default());
    Ok(())
}
