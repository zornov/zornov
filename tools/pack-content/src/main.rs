// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `pack-content`: builds `content-build/` from the two input sources.
//!
//! ```text
//! pack-content --made ./made --open ~/Movies/Studio/Content \
//!              --out content-build [--budget-mb 300] [--all-packs] [--threads N] [--limit N]
//! ```
//!
//! Output layout:
//!
//! ```text
//! content-build/
//!   core/                index.json, LICENSES.md, files   (< budget)
//!   packs/<id>/          index.json, LICENSES.md, files   (optional packs)
//!   review/sheets/       one contact sheet per set (pages of 120)
//!   review/gate-report.txt, review/size-report.txt
//! ```
//!
//! Without `--all-packs` the large packs (music, OpenGameArt sounds, the
//! full font collection) get their `index.json` and `LICENSES.md` but their
//! files are not copied; the size report says so.
//!
//! The core bundle is filled in two rounds so every kind is represented:
//! first the shared open-licence sets (icons, Twemoji, Kenney sounds, a few
//! fonts) and the first few sets of each of our kinds, then the remaining
//! sets in kind order until the budget is reached. `--budget-mb` is the
//! size of the whole `core/` folder including `index.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use studio_content::gate::{self, Reason, SheetEntry};
use studio_content::image::{self, resize_to_fit};
use studio_content::item::{Index, Item, Kind, Licence};
use studio_content::pack::{self, LicenceText, Pack, PackSpec};
use studio_content::{bundle, jpeg, key, made, naming, open, png, svg, video};
use studio_media::rgba::RgbaImage;

struct Args {
    made: PathBuf,
    open: PathBuf,
    out: PathBuf,
    repo_root: PathBuf,
    budget_mb: u64,
    all_packs: bool,
    threads: usize,
    limit: Option<usize>,
}

fn parse_args() -> Result<Args, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut a = Args {
        made: PathBuf::from("made"),
        open: PathBuf::from(format!("{home}/Movies/Studio/Content")),
        out: PathBuf::from("content-build"),
        repo_root: PathBuf::from("."),
        budget_mb: 300,
        all_packs: false,
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        limit: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--made" => a.made = PathBuf::from(value()?),
            "--open" => a.open = PathBuf::from(value()?),
            "--out" => a.out = PathBuf::from(value()?),
            "--repo-root" => a.repo_root = PathBuf::from(value()?),
            "--budget-mb" => a.budget_mb = value()?.parse().map_err(|e| format!("--budget-mb: {e}"))?,
            "--threads" => a.threads = value()?.parse().map_err(|e| format!("--threads: {e}"))?,
            "--limit" => a.limit = Some(value()?.parse().map_err(|e| format!("--limit: {e}"))?),
            "--all-packs" => a.all_packs = true,
            "-h" | "--help" => {
                println!("{}", include_str!("main.rs").lines().take_while(|l| l.starts_with("//!")).map(|l| l.trim_start_matches("//!").trim_start_matches(' ')).collect::<Vec<_>>().join("\n"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(a)
}

/// JPEG quality for opaque backgrounds, photos and overlays.
const JPEG_QUALITY: u8 = 86;

/// Held back from the core budget for `index.json` and `LICENSES.md`.
const INDEX_RESERVE_MB: u64 = 8;

/// Longest side for processed stills of each kind in the core bundle.
fn target_side(kind: Kind) -> u32 {
    match kind {
        Kind::Sticker | Kind::Emoji => 512,
        Kind::TextArt => 640,
        Kind::Frame => 800,
        Kind::GreenScreen => 1280,
        Kind::Overlay | Kind::Background | Kind::Photo => 1920,
        _ => 0,
    }
}

/// One processed item ready for packing.
struct Done {
    item: Item,
    /// Where the processed/copied file lives now (absolute), when it passed.
    staged: Option<PathBuf>,
    bytes: u64,
    reasons: Vec<Reason>,
    /// Gate warnings: the item is kept but flagged for review.
    warnings: Vec<Reason>,
    thumb: Option<RgbaImage>,
    note: Option<String>,
}

/// How many sets of each of our kinds go into the core bundle in the first
/// round, before the leftover budget is spent on more sets.
fn first_round_sets(kind: Kind) -> usize {
    match kind {
        Kind::Emoji | Kind::Overlay => usize::MAX,
        Kind::Sticker => 12,
        Kind::Frame => 3,
        Kind::TextArt => 2,
        Kind::GreenScreen => 6,
        Kind::Motion => 2,
        Kind::Background => 3,
        Kind::Photo => 2,
        _ => 0,
    }
}

/// Plural, lower-case label for pack names ("More stickers").
fn plural(kind: Kind) -> &'static str {
    match kind {
        Kind::Sticker => "stickers",
        Kind::Emoji => "emoji",
        Kind::Shape => "shapes",
        Kind::TextArt => "text art",
        Kind::Frame => "frames",
        Kind::Overlay => "overlays",
        Kind::Background => "backgrounds",
        Kind::Photo => "photos",
        Kind::GreenScreen => "green-screen items",
        Kind::Motion => "motion clips",
        Kind::Sound => "sounds",
        Kind::Music => "music",
        Kind::Font => "fonts",
        Kind::Look => "looks",
    }
}

/// Bytes of every file under `dir`.
fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_bytes(&p);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if c == '/' || c == '\\' || c == ':' { '-' } else { c }).collect()
}

/// Processes one made-for-Studio still into the work folder.
fn process_made_still(ix: &Index, item: &Item, work: &Path) -> Done {
    let src = ix.path(&item.file);
    let mut it = item.clone();
    let img = match studio_media::png::read(&src) {
        Ok(i) => i,
        Err(e) => return Done { item: it, staged: None, bytes: 0, reasons: vec![], warnings: vec![], thumb: None, note: Some(format!("unreadable: {e}")) },
    };
    let mut img = img;
    if item.kind == Kind::GreenScreen {
        match key::auto(&img) {
            Some((keyed, settings)) => {
                img = image::crop_to_content(&keyed, 4, 8);
                it.details.key = Some(settings);
            }
            None => {
                return Done { item: it, staged: None, bytes: 0, reasons: vec![], warnings: vec![], thumb: Some(resize_to_fit(&img, 150)), note: Some("no green screen detected".into()) };
            }
        }
    }
    let verdict = gate::verdict(item.kind, &img);
    let out = resize_to_fit(&img, target_side(item.kind));
    let thumb = Some(resize_to_fit(&out, 150));
    if !verdict.passed() {
        return Done { item: it, staged: None, bytes: 0, reasons: verdict.rejects, warnings: verdict.warnings, thumb, note: None };
    }
    // Opaque photographic kinds go to JPEG; anything with transparency stays PNG.
    let as_jpeg = matches!(item.kind, Kind::Background | Kind::Photo | Kind::Overlay) && image::is_opaque(&out);
    let rel = PathBuf::from(item.kind.folder()).join(safe_name(&item.set)).join(format!("{}.{}", safe_name(&item.name), if as_jpeg { "jpg" } else { "png" }));
    let dst = work.join(&rel);
    if let Some(d) = dst.parent() {
        let _ = fs::create_dir_all(d);
    }
    let written = if as_jpeg { jpeg::write(&out, JPEG_QUALITY, &dst) } else { png::write(&out, &dst) };
    if let Err(e) = written {
        return Done { item: it, staged: None, bytes: 0, reasons: vec![], warnings: vec![], thumb, note: Some(format!("write failed: {e}")) };
    }
    let bytes = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    it.file = rel;
    it.details.plain_variant = None; // not packed (see size report)
    it.details.width = Some(out.width);
    it.details.height = Some(out.height);
    it.details.modified = true;
    Done { item: it, staged: Some(dst), bytes, reasons: vec![], warnings: verdict.warnings, thumb, note: None }
}

/// Motion clips are copied as they are; the poster frame is keyed for the sheet.
fn process_made_motion(ix: &Index, item: &Item, work: &Path) -> Done {
    let src = ix.path(&item.file);
    let mut it = item.clone();
    let poster = video::poster_frame(&src, 0.25);
    let (thumb, note) = match poster {
        Ok(frame) => {
            let keyed = key::auto(&frame).map(|(k, s)| {
                it.details.key = Some(s);
                k
            });
            (Some(resize_to_fit(keyed.as_ref().unwrap_or(&frame), 150)), None)
        }
        Err(e) => (None, Some(format!("no poster frame: {e}"))),
    };
    if let Ok(d) = video::duration_secs(&src) {
        it.details.duration_secs = Some(d);
    }
    let rel = PathBuf::from(item.kind.folder()).join(safe_name(&item.set)).join(format!("{}.mp4", safe_name(&item.name)));
    let dst = work.join(&rel);
    if let Some(d) = dst.parent() {
        let _ = fs::create_dir_all(d);
    }
    if let Err(e) = fs::copy(&src, &dst) {
        return Done { item: it, staged: None, bytes: 0, reasons: vec![], warnings: vec![], thumb, note: Some(format!("copy failed: {e}")) };
    }
    let bytes = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    it.file = rel;
    Done { item: it, staged: Some(dst), bytes, reasons: vec![], warnings: vec![], thumb, note }
}

/// Open-licence items keep their upstream file; rasters are gated, vectors
/// and audio pass, thumbnails come from the preview or the file.
fn process_open(ix: &Index, item: &Item, sheet_thumbs: bool) -> Done {
    let src = ix.path(&item.file);
    let it = item.clone();
    let bytes = match fs::metadata(&src) {
        Ok(m) => m.len() + it.details.font_files.iter().skip(1).filter_map(|f| fs::metadata(ix.path(&f.file)).ok()).map(|m| m.len()).sum::<u64>(),
        Err(e) => return Done { item: it, staged: None, bytes: 0, reasons: vec![], warnings: vec![], thumb: None, note: Some(format!("missing: {e}")) },
    };
    let mut reasons = Vec::new();
    let mut thumb = None;
    if item.kind.is_image() {
        if item.is_vector() {
            if sheet_thumbs {
                thumb = match &item.preview {
                    Some(p) => studio_media::png::read(&ix.path(p)).ok().map(|i| resize_to_fit(&i, 150)),
                    None => fs::read_to_string(&src).ok().and_then(|t| svg::render_fit(&t, 150, &svg::RenderOptions::default()).ok()),
                };
            }
        } else {
            match studio_media::png::read(&src) {
                Ok(img) => {
                    reasons = gate::check(item.kind, &img);
                    if sheet_thumbs {
                        thumb = Some(resize_to_fit(&img, 150));
                    }
                }
                Err(e) => return Done { item: it, staged: None, bytes: 0, reasons: vec![], warnings: vec![], thumb: None, note: Some(format!("unreadable: {e}")) },
            }
        }
    }
    if !reasons.is_empty() {
        return Done { item: it, staged: None, bytes: 0, reasons, warnings: vec![], thumb, note: None };
    }
    Done { item: it, staged: Some(src), bytes, reasons: vec![], warnings: vec![], thumb, note: None }
}

/// Runs `f` over `items` on `threads` workers, keeping the input order.
fn par_map<T: Send, F: Fn(&Item) -> T + Sync>(items: &[&Item], threads: usize, f: F) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1).min(items.len().max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                let r = f(items[i]);
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

fn write_sheets(dir: &Path, title: &str, dones: &[&Done]) {
    const PER_PAGE: usize = 120;
    let entries: Vec<SheetEntry> = dones
        .iter()
        .filter_map(|d| d.thumb.as_ref().map(|t| SheetEntry { name: d.item.name.clone(), thumb: t.clone(), reasons: d.reasons.clone(), warnings: d.warnings.clone() }))
        .collect();
    if entries.is_empty() {
        return;
    }
    let pages = entries.len().div_ceil(PER_PAGE);
    for (p, chunk) in entries.chunks(PER_PAGE).enumerate() {
        let name = if pages > 1 { format!("{} {}", title, p + 1) } else { title.to_string() };
        let sheet = gate::contact_sheet(&name, chunk, 10);
        let file = dir.join(format!("{}.png", safe_name(&name)));
        if let Err(e) = png::write(&sheet, &file) {
            eprintln!("warning: could not write {}: {e}", file.display());
        }
    }
}

/// Curated families for the core bundle; everything else goes to the fonts pack.
const CORE_FONTS: [&str; 17] = [
    "Inter", "Roboto", "Open Sans", "Montserrat", "Poppins", "Nunito", "Raleway", "Oswald", "Bebas Neue", "Anton",
    "Playfair Display", "Merriweather", "Lobster", "Pacifico", "Dancing Script", "Caveat", "Permanent Marker",
];

/// Trims a family to a few files so the core bundle stays small: the
/// variable files when there are any, else regular/bold in both styles.
fn trim_font(item: &mut Item) {
    let files = &item.details.font_files;
    let variable: Vec<_> = files.iter().filter(|f| !f.axes.is_empty()).cloned().collect();
    let keep = if !variable.is_empty() {
        variable
    } else {
        files.iter().filter(|f| f.weight == 400 || f.weight == 700).cloned().collect()
    };
    if !keep.is_empty() {
        item.details.font_files = keep;
        item.file = item.details.font_files[0].file.clone();
    }
}

fn licence_texts(open: &Index) -> Vec<LicenceText> {
    let read = |rel: &str| fs::read_to_string(open.path(Path::new(rel))).ok();
    let mut out = Vec::new();
    if let Some(t) = read("stickers/fluent-emoji/LICENSE") {
        out.push(LicenceText { licence: Licence::Mit, source_id: "fluent-emoji".into(), title: "Fluent Emoji — MIT License".into(), text: t });
    }
    if let Some(t) = read("icons/tabler/pkg/package/LICENSE") {
        out.push(LicenceText { licence: Licence::Mit, source_id: "tabler".into(), title: "Tabler Icons — MIT License".into(), text: t });
    }
    if let Some(t) = read("emoji/twemoji/LICENSE-GRAPHICS") {
        out.push(LicenceText { licence: Licence::CcBy4, source_id: "twemoji".into(), title: "Twemoji graphics licence notice".into(), text: t });
    }
    if let Some(t) = read("emoji/openmoji/LICENSE.txt") {
        out.push(LicenceText { licence: Licence::CcBySa4, source_id: "openmoji".into(), title: "OpenMoji licence notice".into(), text: t });
    }
    // OFL and Apache texts come from the first font of each licence; the
    // per-family copyright lines are listed with the families themselves.
    for (licence, title) in [(Licence::Ofl11, "SIL Open Font License, Version 1.1"), (Licence::Apache2, "Apache License, Version 2.0")] {
        if let Some(f) = open.items.iter().find(|i| i.kind == Kind::Font && i.licence == licence) {
            if let Some(t) = f.details.licence_file.as_ref().and_then(|p| fs::read_to_string(open.path(p)).ok()) {
                let body = match t.find("This Font Software is licensed") {
                    Some(i) if licence == Licence::Ofl11 => t[i..].to_string(),
                    _ => t,
                };
                out.push(LicenceText { licence, source_id: String::new(), title: title.into(), text: body });
            }
        }
    }
    out
}

fn copy_into(src: &Path, pack_dir: &Path, rel: &Path) -> std::io::Result<u64> {
    let dst = pack_dir.join(rel);
    if let Some(d) = dst.parent() {
        fs::create_dir_all(d)?;
    }
    if dst.exists() {
        return Ok(0);
    }
    fs::copy(src, &dst)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let started = Instant::now();
    let out = &args.out;
    let work = out.join("work");
    let review = out.join("review");
    let sheets = review.join("sheets");
    for d in [&work, &sheets] {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    fs::write(out.join(".gitignore"), "*\n").map_err(|e| e.to_string())?;

    // 1. Load and audit names.
    let made_ix = made::load(&args.made)?;
    let open_ix = open::load(&args.open)?;
    println!("loaded {} made items, {} open-licence items", made_ix.len(), open_ix.len());
    let mut problems = naming::audit(&made_ix);
    problems.extend(naming::audit(&open_ix));
    if !problems.is_empty() {
        for (id, text, p) in &problems {
            eprintln!("bad name {text:?} ({id}): {p:?}");
        }
        return Err(format!("{} user-visible names break the naming rules", problems.len()));
    }
    println!("naming audit: every name, set and category is clean");

    // 2. Process, grouped by set (ours) or by source and kind (open), one sheet per group.
    let mut dones: Vec<Done> = Vec::new();
    let mut groups: Vec<(String, Vec<&Item>, bool)> = Vec::new(); // (title, items, is_made)
    let mut by_set: BTreeMap<(Kind, String), Vec<&Item>> = BTreeMap::new();
    for it in &made_ix.items {
        by_set.entry((it.kind, it.set.clone())).or_default().push(it);
    }
    for ((kind, set), items) in by_set {
        groups.push((format!("{} - {}", kind.label(), set), items, true));
    }
    let mut by_src: BTreeMap<(String, Kind), Vec<&Item>> = BTreeMap::new();
    for it in &open_ix.items {
        by_src.entry((it.source.id.clone(), it.kind)).or_default().push(it);
    }
    for ((src, kind), items) in by_src {
        groups.push((format!("{} - {}", kind.label(), src), items, false));
    }
    for (title, items, is_made) in &groups {
        let items: Vec<&Item> = match args.limit {
            Some(n) => items.iter().copied().take(n).collect(),
            None => items.clone(),
        };
        let t0 = Instant::now();
        let results = par_map(&items, args.threads, |it| {
            if *is_made {
                if it.kind == Kind::Motion { process_made_motion(&made_ix, it, &work) } else { process_made_still(&made_ix, it, &work) }
            } else {
                process_open(&open_ix, it, it.kind.is_image())
            }
        });
        let refs: Vec<&Done> = results.iter().collect();
        write_sheets(&sheets, title, &refs);
        let rejected = results.iter().filter(|d| !d.reasons.is_empty() || d.note.is_some()).count();
        println!("{title}: {} items, {} rejected, {:.1}s", results.len(), rejected, t0.elapsed().as_secs_f32());
        dones.extend(results.into_iter().map(|mut d| {
            d.thumb = None;
            d
        }));
    }

    // 3. Assign to packs. Core is filled first-fit from a priority list of
    // groups; the budget covers the whole folder, so some is held back for
    // index.json and LICENSES.md (measured after writing, see below).
    let total_budget = args.budget_mb * 1024 * 1024;
    let budget = total_budget.saturating_sub(INDEX_RESERVE_MB * 1024 * 1024);
    let mut core = Pack::new(PackSpec { id: "core".into(), name: "Zornov core library".into(), description: "Ships inside the app: stickers, emoji, shapes, text art, frames, green-screen stills and clips, overlays, backgrounds, sounds and a set of fonts.".into(), core: true, share_alike: false });
    let mut packs: BTreeMap<String, Pack> = BTreeMap::new();
    let spec = |id: &str, name: &str, desc: &str, sa: bool| PackSpec { id: id.into(), name: name.into(), description: desc.into(), core: false, share_alike: sa };
    let overflow = |kind: Kind| -> (String, String, String) {
        let f = kind.folder();
        (format!("more-{f}"), format!("More {}", plural(kind)), format!("The rest of our {} sets, beyond what fits in the core bundle.", kind.label().to_lowercase()))
    };
    // Priority groups: (sort key, group id, forced pack or None for core-candidate)
    struct Group {
        order: u32,
        items: Vec<(Item, PathBuf, u64)>,
        forced: Option<(String, String, String, bool)>,
        kind: Kind,
    }
    let mut group_map: BTreeMap<String, Group> = BTreeMap::new();
    let mut gate_report = String::new();
    let mut warning_report = String::new();
    let mut rejected_count = 0usize;
    let mut warned_count = 0usize;
    // Rank of each of our sets within its kind (by name), for the two rounds.
    let mut set_rank: BTreeMap<(Kind, String), usize> = BTreeMap::new();
    {
        let sets: BTreeSet<(Kind, String)> = made_ix.items.iter().map(|i| (i.kind, i.set.clone())).collect();
        let mut n: BTreeMap<Kind, usize> = BTreeMap::new();
        for (kind, set) in sets {
            let r = n.entry(kind).or_default();
            set_rank.insert((kind, set), *r);
            *r += 1;
        }
    }
    let round = |kind: Kind, set: &str| -> u32 { if set_rank.get(&(kind, set.to_string())).copied().unwrap_or(0) < first_round_sets(kind) { 20 } else { 40 } };
    for d in &dones {
        if !d.warnings.is_empty() {
            warned_count += 1;
            let why = d.warnings.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>().join("; ");
            warning_report.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", d.item.kind.label(), d.item.set, d.item.name, d.item.source.id, why));
        }
        let Some(staged) = &d.staged else {
            rejected_count += 1;
            let why = if d.reasons.is_empty() { d.note.clone().unwrap_or_default() } else { d.reasons.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>().join("; ") };
            gate_report.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", d.item.kind.label(), d.item.set, d.item.name, d.item.source.id, why));
            continue;
        };
        let it = &d.item;
        let (key, order, forced): (String, u32, Option<(String, String, String, bool)>) = match (it.source.id.as_str(), it.kind) {
            ("tabler", _) => ("open:tabler".into(), 10, None),
            ("twemoji", _) => ("open:twemoji".into(), 11, None),
            ("kenney", Kind::Sound) => ("open:kenney-sfx".into(), 12, None),
            ("kenney", Kind::Music) => ("open:kenney-music".into(), 13, None),
            ("google-fonts", _) if CORE_FONTS.contains(&it.name.as_str()) => ("open:core-fonts".into(), 14, None),
            ("made", Kind::Emoji) => (format!("made:emoji:{}", it.set), round(it.kind, &it.set), None),
            ("made", Kind::TextArt) => (format!("made:text:{}", it.set), round(it.kind, &it.set) + 1, None),
            ("made", Kind::Frame) => (format!("made:frame:{}", it.set), round(it.kind, &it.set) + 2, None),
            ("made", Kind::Sticker) => (format!("made:sticker:{}", it.set), round(it.kind, &it.set) + 3, None),
            ("made", Kind::GreenScreen) => (format!("made:gs:{}", it.set), round(it.kind, &it.set) + 4, None),
            ("made", Kind::Overlay) => (format!("made:overlay:{}", it.set), round(it.kind, &it.set) + 5, None),
            ("made", Kind::Motion) => (format!("made:motion:{}", it.set), round(it.kind, &it.set) + 6, None),
            ("made", Kind::Background) => (format!("made:bg:{}", it.set), round(it.kind, &it.set) + 7, None),
            ("made", Kind::Photo) => (format!("made:photo:{}", it.set), round(it.kind, &it.set) + 8, None),
            ("openmoji", _) => ("open:openmoji".into(), 50, Some(("openmoji".into(), "OpenMoji emoji (ShareAlike set)".into(), "The OpenMoji emoji set, CC BY-SA 4.0. Kept in its own pack because of the ShareAlike condition.".into(), true))),
            ("fluent-emoji", _) => ("open:fluent".into(), 51, Some(("fluent-stickers".into(), "Fluent emoji stickers".into(), "Microsoft's Fluent Emoji in the 3D style, MIT licensed.".into(), false))),
            ("kenney", _) => ("open:kenney-stickers".into(), 52, Some(("kenney-stickers".into(), "Kenney stickers".into(), "Emote stickers by Kenney, CC0.".into(), false))),
            ("incompetech", _) => ("open:incompetech".into(), 53, Some(("music-incompetech".into(), "Music by Kevin MacLeod".into(), "Royalty-free music by Kevin MacLeod (incompetech.com), CC BY 4.0: the credit line must be shown wherever a track is used.".into(), false))),
            ("opengameart", Kind::Music) => ("open:oga-music".into(), 54, Some(("music-opengameart".into(), "Music from OpenGameArt".into(), "Tracks from OpenGameArt.org, CC0 and CC BY 4.0.".into(), false))),
            ("opengameart", Kind::Sound) => ("open:oga-sfx".into(), 55, Some(("sound-effects-opengameart".into(), "More sound effects".into(), "Sound effects from OpenGameArt.org, CC0.".into(), false))),
            ("google-fonts", _) => ("open:fonts".into(), 56, Some(("fonts-google".into(), "More fonts".into(), "The whole Google Fonts collection (OFL and Apache-2.0 families).".into(), false))),
            (src, kind) => (format!("other:{src}:{}", kind.folder()), 90, Some((format!("extra-{src}"), format!("Extra {src}"), String::new(), false))),
        };
        let mut item = it.clone();
        if item.kind == Kind::Font && forced.is_none() {
            trim_font(&mut item);
        }
        if item.source.id == "twemoji" && item.is_vector() {
            // The app renders the SVG itself; the upstream 72 px PNGs would
            // only cost budget.
            item.preview = None;
        }
        let bytes = if item.kind == Kind::Font && forced.is_none() {
            item.details.font_files.iter().filter_map(|f| fs::metadata(open_ix.path(&f.file)).ok()).map(|m| m.len()).sum()
        } else {
            d.bytes
        };
        group_map.entry(key).or_insert_with(|| Group { order, items: Vec::new(), forced, kind: it.kind }).items.push((item, staged.clone(), bytes));
    }
    let mut ordered: Vec<(String, Group)> = group_map.into_iter().collect();
    ordered.sort_by_key(|(k, g)| (g.order, k.clone()));
    let mut placements: Vec<(String, Item, PathBuf, u64)> = Vec::new(); // (pack id, item, staged, bytes)
    let mut large_packs: BTreeSet<String> = ["music-incompetech", "music-opengameart", "sound-effects-opengameart", "fonts-google"].iter().map(|s| s.to_string()).collect();
    for (_, g) in ordered {
        let total: u64 = g.items.iter().map(|(_, _, b)| b).sum();
        let pack_id = match &g.forced {
            Some((id, name, desc, sa)) => {
                packs.entry(id.clone()).or_insert_with(|| Pack::new(spec(id, name, desc, *sa)));
                id.clone()
            }
            None if core.bytes + total <= budget => "core".to_string(),
            None => {
                let (id, name, desc) = overflow(g.kind);
                packs.entry(id.clone()).or_insert_with(|| Pack::new(spec(&id, &name, &desc, false)));
                id
            }
        };
        for (item, staged, bytes) in g.items {
            if pack_id == "core" {
                core.bytes += bytes;
            } else {
                packs.get_mut(&pack_id).unwrap().bytes += bytes;
            }
            placements.push((pack_id.clone(), item, staged, bytes));
        }
    }
    if args.all_packs {
        large_packs.clear();
    }

    // 4. Write packs: move/copy files, index.json, LICENSES.md.
    let texts = licence_texts(&open_ix);
    let core_dir = out.join("core");
    for d in [&core_dir, &out.join("packs")] {
        if d.exists() {
            fs::remove_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
    }
    let mut copied_bytes: BTreeMap<String, u64> = BTreeMap::new();
    for (pack_id, mut item, staged, _bytes) in placements {
        let pack_dir = if pack_id == "core" { core_dir.clone() } else { out.join("packs").join(&pack_id) };
        let materialise = !large_packs.contains(&pack_id);
        if materialise {
            let mut copied = 0u64;
            if item.source.id == "made" {
                let dst = pack_dir.join(&item.file);
                if let Some(p) = dst.parent() {
                    fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
                fs::rename(&staged, &dst).or_else(|_| fs::copy(&staged, &dst).map(|_| ())).map_err(|e| format!("{}: {e}", dst.display()))?;
                copied += fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
            } else {
                // Open items keep their upstream relative paths; copy every file the item names.
                let mut rels: Vec<PathBuf> = vec![item.file.clone()];
                rels.extend(item.preview.iter().cloned());
                rels.extend(item.details.filled_variant.iter().cloned());
                rels.extend(item.details.font_files.iter().map(|f| f.file.clone()));
                rels.extend(item.details.licence_file.iter().cloned());
                for rel in rels {
                    copied += copy_into(&open_ix.path(&rel), &pack_dir, &rel).map_err(|e| format!("{}: {e}", rel.display()))?;
                }
            }
            *copied_bytes.entry(pack_id.clone()).or_default() += copied;
        }
        if item.source.id != "made" && item.details.licence_file.is_some() && !materialise {
            // Keep the reference; the file arrives with the pack.
        }
        if item.source.id == "made" {
            item.details.plain_variant = None;
        }
        if pack_id == "core" {
            core.items.push(item);
        } else {
            packs.get_mut(&pack_id).unwrap().items.push(item);
        }
    }
    let _ = fs::remove_dir_all(&work);
    let mut all_packs: Vec<Pack> = vec![core];
    all_packs.extend(packs.into_values());
    let mut size_report = String::new();
    size_report.push_str(&format!("# Size report — {}\n\n", chrono_like_now()));
    size_report.push_str(&format!("Core budget: {} MB. Rejected by the gate or unreadable: {rejected_count}; kept with a warning: {warned_count} (see gate-report.txt).\n\n", args.budget_mb));
    size_report.push_str("| pack | items | bytes | on disk | status |\n|---|---|---|---|---|\n");
    let mut core_note = String::new();
    for p in &all_packs {
        let dir = if p.spec.core { core_dir.clone() } else { out.join("packs").join(&p.spec.id) };
        bundle::write_index(&dir, p).map_err(|e| format!("{}: {e}", dir.display()))?;
        fs::write(dir.join("LICENSES.md"), pack::licenses_md(p, &texts)).map_err(|e| e.to_string())?;
        let status = if large_packs.contains(&p.spec.id) { "index and licences only (run with --all-packs to copy files)" } else { "built" };
        let on_disk = copied_bytes.get(&p.spec.id).copied().unwrap_or(0);
        size_report.push_str(&format!("| {} | {} | {} | {} | {} |\n", p.spec.id, p.items.len(), mb(p.bytes), mb(on_disk), status));
        if p.spec.core {
            let on_disk = dir_bytes(&dir);
            core_note = format!("\nCore folder on disk, including index.json and LICENSES.md: {} (budget {} MB).\n", mb(on_disk), args.budget_mb);
            if on_disk > total_budget {
                return Err(format!("core folder is {} which is over the {} MB budget", mb(on_disk), args.budget_mb));
            }
        }
    }
    size_report.push_str(&core_note);
    size_report.push_str("\nPer-kind breakdown of the core bundle:\n\n");
    let core = &all_packs[0];
    let mut per_kind: BTreeMap<Kind, (usize, u64)> = BTreeMap::new();
    for it in &core.items {
        let e = per_kind.entry(it.kind).or_default();
        e.0 += 1;
        e.1 += 0;
    }
    for (k, (n, _)) in &per_kind {
        size_report.push_str(&format!("- {}: {n} items\n", k.label()));
    }
    size_report.push_str("\nNotes:\n- Our stills are re-encoded at the core sizes (stickers and emoji 512 px, text art 640 px, frames 800 px, green-screen cut-outs 1280 px, overlays/backgrounds/photos 1920 px on the longest side; opaque backgrounds, photos and overlays as JPEG quality 86, everything with transparency as PNG).\n- Green-screen stills are keyed, cropped to their subject and shipped with transparency; the key settings are stored on the item. Ones that touch the frame (bursts, sparks, leaves) are kept with a warning for review.\n- Twemoji ships as SVG only; the app renders its own thumbnails.\n- Outline-free sticker variants are not packed yet (would double the sticker bytes).\n- Motion clips are copied unchanged (H.264 MP4, green screen; keyed on import).\n");
    fs::write(review.join("size-report.txt"), &size_report).map_err(|e| e.to_string())?;
    fs::write(
        review.join("gate-report.txt"),
        format!("# Rejected\nkind\tset\tname\tsource\treason\n{gate_report}\n# Kept with a warning (check the amber cells on the contact sheets)\nkind\tset\tname\tsource\treason\n{warning_report}"),
    )
    .map_err(|e| e.to_string())?;
    let attributions = pack::attributions_md(&all_packs, &texts);
    fs::write(args.repo_root.join("ATTRIBUTIONS.md"), attributions).map_err(|e| e.to_string())?;
    println!("\n{size_report}");
    println!("done in {:.0}s", started.elapsed().as_secs_f32());
    std::io::stdout().flush().ok();
    Ok(())
}

/// Date without pulling in a time crate: seconds since the epoch, which
/// is enough to tell builds apart.
fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("build {secs}")
}
