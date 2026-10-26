// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real-model tests. They need the models in `~/Movies/Studio/Models` (see
//! `docs/models.md`; download them from Settings > AI models or with
//! `ModelStore::download`) and skip with a message when a model is missing:
//!
//! `cargo test --release -p studio-ai --test real_models -- --ignored --nocapture`

use studio_ai::features::transcribe::{TranscribeOptions, Whisper};
use studio_ai::{Frame, JobCtx, Mask, ModelId, ModelStore};
use std::path::PathBuf;

fn store_with(id: ModelId) -> Option<ModelStore> {
    let s = ModelStore::default();
    if s.is_installed(id) {
        Some(s)
    } else {
        eprintln!("{} not installed in {}; skipping", id.spec().name, s.root().display());
        None
    }
}

/// A photo from `$STUDIO_AI_TEST_PHOTOS/<name>.jpg` (not shipped: set the
/// variable to a folder of your own test pictures to enable the semantic
/// checks).
fn photo(name: &str) -> Option<Frame> {
    let dir = std::env::var_os("STUDIO_AI_TEST_PHOTOS")?;
    let img = studio_media::jpeg::read(&PathBuf::from(dir).join(format!("{name}.jpg"))).ok()?;
    Frame::new(img.width as usize, img.height as usize, img.pixels).ok()
}

/// Writes a mask as a grey PNG next to the test photos (for eyeballing).
fn dump(name: &str, m: &Mask) {
    let Some(dir) = std::env::var_os("STUDIO_AI_TEST_OUT") else { return };
    let mut img = studio_media::rgba::RgbaImage::new(m.width as u32, m.height as u32);
    for (p, v) in img.pixels.chunks_exact_mut(4).zip(m.to_u8()) {
        p.copy_from_slice(&[v, v, v, 255]);
    }
    let _ = std::fs::write(PathBuf::from(dir).join(format!("{name}.png")), studio_media::png::encode(&img));
}

/// A synthetic test image: a red disk on a noisy grey-green background.
fn disk_image(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Frame {
    let mut f = Frame::filled(w, h, [0, 0, 0, 255]);
    for y in 0..h {
        for x in 0..w {
            let n = ((x * 7919 + y * 104_729) % 61) as u8;
            let inside = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() < r;
            let px = if inside { [220, 30 + n / 4, 40, 255] } else { [90 + n, 120 + n / 2, 90, 255] };
            f.rgba[(y * w + x) * 4..][..4].copy_from_slice(&px);
        }
    }
    f
}

fn iou(m: &Mask, f: impl Fn(usize, usize) -> bool) -> f32 {
    let (mut i, mut u) = (0, 0);
    for y in 0..m.height {
        for x in 0..m.width {
            let a = m.data[y * m.width + x] > 0.5;
            let b = f(x, y);
            i += (a && b) as u32;
            u += (a || b) as u32;
        }
    }
    i as f32 / u.max(1) as f32
}

/// Speech from the macOS `say` command as 16 kHz mono.
fn speech(text: &str) -> Option<Vec<f32>> {
    let dir = std::env::temp_dir().join(format!("studio-ai-say-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    // tests run in parallel: one file per text
    let path = dir.join(format!("{}.wav", studio_ai::cache::data_hash(text.as_bytes())));
    let ok = std::process::Command::new("say").args(["-v", "Samantha", "-o"]).arg(&path).args(["--data-format=LEI16@16000", text]).status().ok()?.success();
    if !ok {
        eprintln!("`say` failed; skipping");
        return None;
    }
    let w = studio_media::wav::read(&path).ok()?;
    assert_eq!(w.sample_rate, 16_000);
    Some(studio_nn::audio::downmix(&w.samples, w.channels as usize))
}

#[test]
#[ignore]
fn whisper_transcribes_with_word_times() {
    let Some(store) = store_with(ModelId::WhisperTiny) else { return };
    let Some(audio) = speech("The quick brown fox jumps over the lazy dog. Then it runs away into the forest.") else { return };
    let w = Whisper::load(&store, ModelId::WhisperTiny).unwrap();
    let (lang, p) = w.detect_language(&audio).unwrap();
    println!("language {lang} p={p:.2}");
    assert_eq!(lang, "en");
    let t0 = std::time::Instant::now();
    let t = w.transcribe(&audio, 16_000, &TranscribeOptions::default(), &JobCtx::detached()).unwrap();
    println!("transcribed {:.1}s of audio in {:.2}s", audio.len() as f64 / 16000.0, t0.elapsed().as_secs_f64());
    for s in &t.segments {
        println!("[{:.2} - {:.2}] {}", s.start, s.end, s.text);
        for wd in &s.words {
            println!("    {:>6.2} {:>6.2} {:.2} {}", wd.start, wd.end, wd.probability, wd.text);
        }
    }
    let text = t.text().to_lowercase();
    assert!(text.contains("quick brown fox") && text.contains("lazy dog"), "{text}");
    let words: Vec<_> = t.words().collect();
    assert!(words.len() >= 14, "{} words", words.len());
    let dur = audio.len() as f64 / 16000.0;
    for pair in words.windows(2) {
        assert!(pair[0].start <= pair[1].start);
    }
    // "fox" should be in the first half, "forest" near the end
    let fox = words.iter().find(|w| w.text.to_lowercase().contains("fox")).unwrap();
    let forest = words.iter().find(|w| w.text.to_lowercase().contains("forest")).unwrap();
    assert!(fox.start < dur * 0.5 && forest.start > dur * 0.6, "fox {} forest {} dur {dur}", fox.start, forest.start);
}

#[test]
#[ignore]
fn modnet_mattes() {
    use studio_ai::features::matting::BackgroundRemover;
    let Some(store) = store_with(ModelId::Modnet) else { return };
    let mut br = BackgroundRemover::load(&store).unwrap();
    let f = disk_image(320, 240, 160.0, 120.0, 60.0);
    let t0 = std::time::Instant::now();
    let m = br.next_frame(&f).unwrap();
    println!("matte {}x{} in {:.2}s", m.width, m.height, t0.elapsed().as_secs_f64());
    assert_eq!((m.width, m.height), (320, 240));
    assert!(m.data.iter().all(|v| (0.0..=1.0).contains(v)));
    let m2 = br.next_frame(&f).unwrap();
    assert!(m2.data.iter().all(|v| v.is_finite()));
    if let Some(p) = photo("chawla") {
        let t0 = std::time::Instant::now();
        let m = br.matte(&p).unwrap();
        println!("photo matte in {:.2}s, coverage {:.2}", t0.elapsed().as_secs_f64(), m.coverage());
        dump("chawla_matte", &m);
        // a centred portrait: the middle is foreground, the top corners are not
        let at = |x: f32, y: f32| m.data[(y * m.height as f32) as usize * m.width + (x * m.width as f32) as usize];
        assert!(at(0.5, 0.45) > 0.9 && at(0.03, 0.03) < 0.1 && at(0.97, 0.03) < 0.1);
    }
}

#[test]
#[ignore]
fn sam_cuts_out_and_tracks() {
    use studio_ai::features::segment::{ObjectCutout, Prompt};
    let Some(store) = store_with(ModelId::Sam21Tiny) else { return };
    let sam = ObjectCutout::load(&store).unwrap();
    let (cx, cy, r) = (200.0, 130.0, 50.0);
    let f = disk_image(400, 300, cx, cy, r);
    let truth = |x: usize, y: usize| ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() < r;
    let t0 = std::time::Instant::now();
    let emb = sam.embed(&f).unwrap();
    println!("embed {:.2}s", t0.elapsed().as_secs_f64());
    let t0 = std::time::Instant::now();
    let click = sam.segment(&emb, &[Prompt::Point { x: cx + 10.0, y: cy - 5.0, positive: true }]).unwrap();
    println!("decode {:.3}s", t0.elapsed().as_secs_f64());
    let boxed = sam.segment(&emb, &[Prompt::Box { x0: cx - r - 8.0, y0: cy - r - 8.0, x1: cx + r + 8.0, y1: cy + r + 8.0 }]).unwrap();
    let both = sam.segment(&emb, &[Prompt::Box { x0: cx - r - 8.0, y0: cy - r - 8.0, x1: cx + r + 8.0, y1: cy + r + 8.0 }, Prompt::Point { x: cx, y: cy, positive: true }]).unwrap();
    for (name, s) in [("click", &click), ("box", &boxed), ("box+click", &both)] {
        let j = iou(&s.mask, truth);
        println!("{name}: iou {j:.3} score {:.2} present {}", s.score, s.present);
        dump(&format!("sam_{name}"), &s.mask);
        assert!(j > 0.85 && s.present, "{name}: iou {j}");
    }
    // tracking a moving disk
    let frames: Vec<Frame> = (0..6).map(|i| disk_image(400, 300, cx + i as f32 * 12.0, cy, r)).collect();
    let mut src = studio_ai::VecSource { frames, fps: 30.0 };
    let mut ious = vec![0.0; 6];
    sam.track_clip(&mut src, 2, &[Prompt::Point { x: cx + 24.0, y: cy, positive: true }], &JobCtx::detached(), |i, s| {
        let c = cx + i as f32 * 12.0;
        ious[i] = iou(&s.mask, |x, y| ((x as f32 - c).powi(2) + (y as f32 - cy).powi(2)).sqrt() < r);
        Ok(())
    })
    .unwrap();
    println!("tracking ious {ious:?}");
    assert!(ious.iter().all(|&j| j > 0.8));
    if let Some(p) = photo("melvin") {
        // click on the person's face area (centre-left of this photo)
        let s = sam.cut_out(&p, &[Prompt::Point { x: p.width as f32 * 0.5, y: p.height as f32 * 0.3, positive: true }]).unwrap();
        println!("photo: coverage {:.2} score {:.2}", s.mask.coverage(), s.score);
        dump("sam_melvin", &s.mask);
    }
}

/// SNR (dB) of `y` against `clean`, after the best global gain.
fn snr_db(clean: &[f32], y: &[f32]) -> f32 {
    let n = clean.len().min(y.len());
    let dot: f64 = (0..n).map(|i| clean[i] as f64 * y[i] as f64).sum();
    let yy: f64 = (0..n).map(|i| (y[i] as f64).powi(2)).sum();
    let g = if yy > 0.0 { dot / yy } else { 0.0 };
    let (mut s, mut e) = (0.0f64, 0.0f64);
    for i in 0..n {
        s += (clean[i] as f64).powi(2);
        e += (clean[i] as f64 - g * y[i] as f64).powi(2);
    }
    (10.0 * (s / e.max(1e-12)).log10()) as f32
}

#[test]
#[ignore]
fn deepfilternet_cleans_noisy_speech() {
    use studio_ai::features::denoise::{CleanupEngine, CleanupSettings, VoiceCleanup};
    let Some(store) = store_with(ModelId::DeepFilterNet3) else { return };
    let Some(clean16) = speech("Please remember to bring your umbrella tomorrow, because the forecast says it will rain all afternoon.") else { return };
    let clean = studio_nn::audio::resample(&clean16, 16_000, 48_000);
    // pseudo-random noise plus a hum, at about 0 dB SNR
    let mut seed = 1u64;
    let noise: Vec<f32> = (0..clean.len())
        .map(|i| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.25 + 0.05 * (i as f32 * 2.0 * std::f32::consts::PI * 120.0 / 48_000.0).sin()
        })
        .collect();
    let p = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32;
    let k = (p(&clean) / p(&noise)).sqrt();
    let noisy: Vec<f32> = clean.iter().zip(&noise).map(|(c, n)| c + n * k).collect();
    let vc = VoiceCleanup::load(&store).unwrap();
    assert_eq!(vc.engine(), CleanupEngine::DeepFilterNet);
    let t0 = std::time::Instant::now();
    let out = vc.process(&noisy, 48_000, 1, &CleanupSettings::default(), &JobCtx::detached()).unwrap();
    println!("{:.1}s of audio cleaned in {:.2}s", clean.len() as f64 / 48e3, t0.elapsed().as_secs_f64());
    let (before, after) = (snr_db(&clean, &noisy), snr_db(&clean, &out));
    let gate = VoiceCleanup::fallback().process(&noisy, 48_000, 1, &CleanupSettings::default(), &JobCtx::detached()).unwrap();
    println!("SNR noisy {before:.1} dB → DeepFilterNet {after:.1} dB, spectral gate {:.1} dB", snr_db(&clean, &gate));
    assert_eq!(out.len(), noisy.len());
    assert!(after > before + 8.0, "{before} → {after}");
    // chunked inference (1 s chunks) must sound the same as one pass
    let mut chunked = VoiceCleanup::load(&store).unwrap();
    chunked.set_chunk_frames(100);
    let out2 = chunked.process(&noisy, 48_000, 1, &CleanupSettings::default(), &JobCtx::detached()).unwrap();
    let c = snr_db(&clean, &out2);
    println!("chunked: {c:.1} dB");
    assert!(c > after - 1.0, "{c} vs {after}");
    // half strength keeps some noise
    let half = vc.process(&noisy, 48_000, 1, &CleanupSettings { strength: 0.5 }, &JobCtx::detached()).unwrap();
    let h = snr_db(&clean, &half);
    assert!(h > before && h < after, "{h}");
}

#[test]
#[ignore]
fn rife_interpolates_motion() {
    use studio_ai::features::interpolate::{blend, FrameInterpolator, InterpEngine};
    let Some(store) = store_with(ModelId::Rife49) else { return };
    let fi = FrameInterpolator::load(&store).unwrap();
    assert_eq!(fi.engine(), InterpEngine::Rife);
    let (y, r) = (120.0, 40.0);
    let a = disk_image(320, 240, 120.0, y, r);
    let b = disk_image(320, 240, 160.0, y, r);
    let t0 = std::time::Instant::now();
    let mid = fi.interpolate(&a, &b, 0.5).unwrap();
    println!("interpolated 320x240 in {:.3}s", t0.elapsed().as_secs_f64());
    let red = |f: &Frame| studio_ai::Mask::new(f.width, f.height, f.rgba.chunks_exact(4).map(|p| if p[0] > 160 && p[1] < 110 { 1.0 } else { 0.0 }).collect());
    let truth = |x: usize, yy: usize| ((x as f32 - 140.0).powi(2) + (yy as f32 - y).powi(2)).sqrt() < r;
    let j = iou(&red(&mid), truth);
    let jb = iou(&red(&blend(&a, &b, 0.5)), truth);
    println!("iou rife {j:.3}, blend {jb:.3}");
    assert!(j > 0.85 && j > jb + 0.1, "rife {j} blend {jb}");
    // odd sizes are padded and cropped back
    let odd = fi.interpolate(&disk_image(101, 77, 50.0, 38.0, 20.0), &disk_image(101, 77, 54.0, 38.0, 20.0), 0.25).unwrap();
    assert_eq!((odd.width, odd.height), (101, 77));
}

/// A checkerboard floor in perspective: near at the bottom, far at the top.
fn floor_image(w: usize, h: usize) -> Frame {
    let mut f = Frame::filled(w, h, [150, 190, 230, 255]); // sky
    let horizon = h as f32 * 0.35;
    for y in 0..h {
        let yf = y as f32 - horizon;
        if yf <= 0.0 {
            continue;
        }
        let z = 200.0 / yf; // distance along the floor
        for x in 0..w {
            let xw = (x as f32 - w as f32 / 2.0) * z / 200.0;
            let c = ((xw.floor() as i64 + z.floor() as i64) & 1) == 0;
            let v = if c { 220 } else { 60 };
            f.rgba[(y * w + x) * 4..][..4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    f
}

#[test]
#[ignore]
fn depth_orders_near_and_far() {
    use studio_ai::features::depth::DepthEstimator;
    let Some(store) = store_with(ModelId::DepthAnythingV2Small) else { return };
    let mut de = DepthEstimator::load(&store).unwrap();
    let f = floor_image(480, 320);
    let t0 = std::time::Instant::now();
    let d = de.estimate(&f).unwrap();
    println!("depth {}x{} in {:.2}s", d.width, d.height, t0.elapsed().as_secs_f64());
    dump("depth_floor", &d);
    let row = |y: usize| d.data[y * d.width..(y + 1) * d.width].iter().sum::<f32>() / d.width as f32;
    println!("near row {:.2}, mid row {:.2}, sky {:.2}", row(310), row(150), row(20));
    assert!(row(310) > row(150) && row(150) > row(20));
    let a = de.next_frame(&f).unwrap();
    let b = de.next_frame(&f).unwrap();
    assert!(a.data.iter().zip(&b.data).all(|(x, y)| (x - y).abs() < 0.05), "a still clip stays still");
    if let Some(p) = photo("melvin") {
        let d = de.estimate(&p).unwrap();
        dump("depth_melvin", &d);
    }
}

#[test]
#[ignore]
fn faces_and_pose() {
    use studio_ai::features::faces::{body, FaceDetector, PoseTracker};
    let Some(store) = store_with(ModelId::FaceDetector) else { return };
    let fd = FaceDetector::load(&store).unwrap();
    let blank = disk_image(320, 240, 160.0, 120.0, 50.0);
    assert!(fd.detect(&blank).unwrap().is_empty(), "no faces in a red disk");
    for name in ["melvin", "chawla"] {
        let Some(p) = photo(name) else { continue };
        let t0 = std::time::Instant::now();
        let faces = fd.detect(&p).unwrap();
        println!("{name}: {} face(s) in {:.3}s: {:?}", faces.len(), t0.elapsed().as_secs_f64(), faces.iter().map(|f| (f.bbox, f.score)).collect::<Vec<_>>());
        assert!(!faces.is_empty());
        let f = &faces[0];
        // eyes above the mouth, inside the box
        assert!(f.keypoints[0].1 < f.keypoints[3].1 && f.keypoints[1].1 < f.keypoints[3].1);
        let (cx, cy) = f.bbox.centre();
        assert!(cx > 0.0 && cx < p.width as f32 && cy > 0.0 && cy < p.height as f32);
    }
    let Some(store) = store_with(ModelId::PoseLandmarker) else { return };
    let mut pt = PoseTracker::load(&store).unwrap();
    assert!(pt.detect(&blank).unwrap().is_none(), "no person in a red disk");
    for name in ["aldrin", "chawla"] {
        let Some(p) = photo(name) else { continue };
        let t0 = std::time::Instant::now();
        let pose = pt.detect(&p).unwrap();
        println!("{name}: pose in {:.3}s", t0.elapsed().as_secs_f64());
        let pose = pose.expect("a person");
        let l = &pose.landmarks;
        for (i, n) in [(body::NOSE, "nose"), (body::LEFT_SHOULDER, "l shoulder"), (body::RIGHT_SHOULDER, "r shoulder"), (body::LEFT_HIP, "l hip"), (body::LEFT_ANKLE, "l ankle")] {
            println!("   {n:<10} ({:.0}, {:.0}) vis {:.2}", l[i].x, l[i].y, l[i].visibility);
        }
        assert!(l[body::NOSE].y < l[body::LEFT_SHOULDER].y && l[body::LEFT_SHOULDER].y < l[body::LEFT_HIP].y);
        if name == "aldrin" {
            // full body: ankles well below the hips
            assert!(l[body::LEFT_ANKLE].y > l[body::LEFT_HIP].y + 150.0);
        }
        // tracking reuses the landmarks' region and finds the same pose
        pt.reset();
        let a = pt.next_frame(&p).unwrap().unwrap();
        let b = pt.next_frame(&p).unwrap().unwrap();
        let d = ((a.landmarks[0].x - b.landmarks[0].x).powi(2) + (a.landmarks[0].y - b.landmarks[0].y).powi(2)).sqrt();
        assert!(d < p.width as f32 * 0.03, "nose moved {d}px between identical frames");
    }
}

#[test]
#[ignore]
fn whisper_long_audio_seeks_across_windows() {
    let Some(store) = store_with(ModelId::WhisperTiny) else { return };
    let para = "Zornov is a video editor that runs entirely on your own computer. It cuts, colours and captions your clips without sending anything to the internet. ";
    let Some(audio) = speech(&para.repeat(6)) else { return };
    let dur = audio.len() as f64 / 16000.0;
    assert!(dur > 35.0, "need more than one 30 s window, got {dur}");
    let w = Whisper::load(&store, ModelId::WhisperTiny).unwrap();
    let t0 = std::time::Instant::now();
    let ctx = JobCtx::detached();
    let t = w.transcribe(&audio, 16_000, &TranscribeOptions { language: Some("en".into()), ..Default::default() }, &ctx).unwrap();
    println!("{dur:.1}s transcribed in {:.1}s", t0.elapsed().as_secs_f64());
    for s in &t.segments {
        println!("[{:6.2} - {:6.2}] {}", s.start, s.end, s.text);
    }
    let text = t.text().to_lowercase();
    assert!(text.matches("internet").count() >= 5, "{text}");
    for w in t.segments.windows(2) {
        assert!(w[1].start >= w[0].start - 0.5, "segments go forward");
    }
    let last = t.segments.last().unwrap();
    assert!(last.end > dur - 6.0 && last.end <= dur + 1.0, "covers the whole file: {} of {dur}", last.end);
    // SRT export and the cache codec on a real result
    assert!(t.to_srt().contains(" --> "));
    let cache = studio_ai::ResultCache::new(std::env::temp_dir().join(format!("studio-ai-rt-{}", std::process::id())));
    let key = studio_ai::CacheKey::new("speech", ModelId::WhisperTiny.key(), TranscribeOptions::default().cache_params());
    cache.put(&key, &t).unwrap();
    assert_eq!(cache.get::<studio_ai::features::transcribe::Transcript>(&key).unwrap(), t);
    cache.clear().unwrap();
}

/// Scene regions on a landscape: `$STUDIO_AI_TEST_SCENE` (a PNG of a
/// landscape with sky at the top); writes a colour map of the regions to
/// `$STUDIO_AI_TEST_OUT/regions.png`.
#[test]
#[ignore]
fn scene_regions_on_a_landscape() {
    use studio_ai::features::regions::{outlines, Region, SceneParser};
    let Some(store) = store_with(ModelId::UperNetConvNextTiny) else { return };
    let Some(path) = std::env::var_os("STUDIO_AI_TEST_SCENE") else {
        eprintln!("STUDIO_AI_TEST_SCENE not set; skipping");
        return;
    };
    let img = studio_media::png::read(&PathBuf::from(path)).unwrap();
    let frame = Frame::new(img.width as usize, img.height as usize, img.pixels).unwrap();
    let parser = SceneParser::load(&store).unwrap();
    let t0 = std::time::Instant::now();
    let mut map = parser.parse(&frame, 256).unwrap();
    println!("parsed in {:.2}s", t0.elapsed().as_secs_f64());
    if let Some(ds) = store_with(ModelId::DepthAnythingV2Small) {
        let depth = studio_ai::features::depth::DepthEstimator::load(&ds).unwrap().estimate(&frame).unwrap();
        map.refine_with_depth(&depth);
    }
    for r in Region::ALL {
        let share = map.share(r);
        if share > 0.005 {
            let o = outlines(&map.hard(r), map.width, map.height, 0.5, 0.002, 1.0, 160);
            println!("{:10} {:5.1}%  {} outline(s), {} points", r.name(), share * 100.0, o.len(), o.iter().map(|x| x.points.len()).sum::<usize>());
        }
    }
    // Sky at the top of a landscape, not at the bottom.
    let top: usize = (0..map.width).filter(|&x| map.label(map.width * 2 + x) == Some(Region::Sky)).count();
    assert!(top > map.width / 3, "sky along the top: {top} of {}", map.width);
    if let Some(dir) = std::env::var_os("STUDIO_AI_TEST_OUT") {
        let colours: [[u8; 3]; 10] = [[110, 170, 255], [20, 60, 200], [150, 110, 60], [230, 200, 120], [30, 150, 40], [130, 130, 130], [120, 90, 160], [200, 60, 60], [255, 0, 255], [255, 160, 0]];
        let mut out = studio_media::rgba::RgbaImage::new(map.width as u32, map.height as u32);
        for (i, p) in out.pixels.chunks_exact_mut(4).enumerate() {
            let c = map.label(i).map(|r| colours[Region::ALL.iter().position(|&x| x == r).unwrap()]).unwrap_or([0, 0, 0]);
            p.copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
        let _ = std::fs::write(PathBuf::from(dir).join("regions.png"), studio_media::png::encode(&out));
    }
}
