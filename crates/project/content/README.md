# studio-content

Library content for Studio: the index of stickers, emoji, shapes, text art,
frames, overlays, backgrounds, green-screen stills and motion clips, sounds,
music and fonts; loaders for the two input sources; search; thumbnails;
automatic chroma key; the quality gate; and pack building with licences.

## Modules

| module | what it does |
|---|---|
| `item` | `Item`, `Kind`, `Licence`, `Source`, `Index` (one root folder), `Library` (every open index) |
| `made` | loads `manifest.json` + `Motion/manifest.json` of our own assets |
| `open` | loads `catalog.json` of the open-licence collection (Twemoji, OpenMoji, Fluent Emoji, Kenney, Tabler Icons, Google Fonts, Kevin MacLeod, OpenGameArt) |
| `naming` | `humanise`, `sentence_case`, `check_name`, `audit`: no ids, hashes, codepoints or prompts in user-visible strings |
| `search` | ranked prefix search over names, tags, sets and categories (AND semantics) |
| `image` | Lanczos-3 resample on premultiplied alpha, crop, blit, checkerboard, 5×7 label font |
| `thumb` | `Thumbnailer`: decode (PNG, JPEG, SVG, MP4 poster, audio waveform, font sample), key, downscale, PNG cache keyed by path + size + mtime |
| `key` | `detect_key`, `apply`, `refine_edges`, `auto`: chroma key in CbCr space with spill suppression and un-mixed soft edges; neutrals stay opaque; pre-keyed pictures get their edges cleaned |
| `gate` | `verdict` / `check` (size, hard edge, mostly empty, touches frame; for green-screen stills touching the frame is a warning, kept and marked amber) and `contact_sheet` |
| `svg` | small SVG rasteriser (paths, shapes, transforms, fill rules, strokes with round joins, flat gradients) |
| `video` | poster frame from an H.264 MP4 via `studio-mp4` + `studio-h264` |
| `png` | PNG encoder with dynamic Huffman DEFLATE (smaller than the fixed-code encoder in `studio-media`) |
| `jpeg` | baseline JPEG encoder for opaque backgrounds, photos and overlays |
| `pack` | `Pack`, `PackSpec`, `LICENSES.md` / `ATTRIBUTIONS.md` generation with the exact wording each licence needs |
| `bundle` | `index.json` read/write; `load_all(content_dir)` opens core + packs into a `Library` |

## Using it in the app

```rust
let lib = studio_content::bundle::load_all(&content_dir)?;      // core + packs/*
let thumbs = studio_content::thumb::Thumbnailer::new(cache_dir)?;
for hit in lib.search("confetti") {
    let img = thumbs.thumbnail(&hit.path(), hit.item.kind, 160)?; // cached, keyed, Lanczos
}
for (source, licence) in lib.credits() { /* Licences screen */ }
```

Green-screen items (`Kind::GreenScreen`, `Kind::Motion`) are keyed by
`key::auto` when thumbnailed and when packed; the settings used are stored in
`item.details.key` so the inspector can edit them.

## Building the content folder

```
CARGO_TARGET_DIR=target-content cargo build --release --manifest-path tools/pack-content/Cargo.toml
./target-content/release/pack-content --out content-build        # core + small packs
./target-content/release/pack-content --out content-build --all-packs   # also copy music, fonts, OGA sounds
```

Inputs default to `./made` (our own assets) and
`~/Movies/Studio/Content` (the open-licence collection; `--made`, `--open` to override). The tool fails
when any user-visible name breaks the naming rules or the finished `core/`
folder (files plus `index.json` and `LICENSES.md`) is over the budget, writes a
contact sheet per set to `content-build/review/sheets/`, `gate-report.txt`
(rejections, then items kept with a warning), `size-report.txt`, and
refreshes `ATTRIBUTIONS.md` at the repository root.

The core bundle is filled in two rounds so every kind is represented: the
shared open-licence sets (Tabler Icons, Twemoji, Kenney sounds and music, a
few Google Fonts families) and the first few sets of each of our kinds, then
the remaining sets in kind order until the budget runs out. What does not fit
goes to a `more-<kind>` pack.
