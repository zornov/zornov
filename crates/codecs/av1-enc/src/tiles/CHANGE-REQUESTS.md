# av1-tiles: change requests for tiled encoding

`tiles::` (layout, fan-out, tile group framing, merge helpers) and a tiled intra gate
(bit-exact on studio-av1 and VideoToolbox) are on feature/av1-tiles. Measured at 1080p, speed 6,
threads 1: about 65-70% of the time is the mode/partition/motion search in `decide_sb`, so speed 6
only becomes usable when superblocks are coded in parallel. That is what tiles are for. The pieces
below are outside the av1-tiles scope.

## Design

Each tile is coded by its own tile coder (own `FrameBuf`, own `TileWriter`, own decider) in FRAME
coordinates, so no position changes. A tile coder only visits the superblocks of its `TileRect` and
treats the tile edge as an edge for neighbour availability. After all tiles are coded,
`tiles::merge_recon` and `tiles::merge_mode_info` copy each tile's region into the frame-level
reconstruction and mode info that the filters read (the post-filters cross tile edges). The tile
bytes go through `tiles::tile_group`.

Spec facts (checked against the studio-av1 decoder's `decode_tile` / `decode_tiles`):
- every tile starts from the frame's CDFs and clears above context, left context, the
  loop-restoration reference values (`RefSgrXqd`, `RefLrWiener`) and delta LF;
- neighbour availability (`AvailU`, `AvailL`) and the MV candidate scan are bounded by the tile
  (`MiRowStart..MiRowEnd`, `MiColStart..MiColEnd`);
- partition edge rules (`hasRows`/`hasCols`) and MV clamping still use the FRAME's `MiRows`/`MiCols`;
- intra above-right / below-left availability stops at the tile edge.

## R1 syntax/ (av1 lead): `TileWriter::with_tile`

`pub fn with_tile(self, mi_row0: usize, mi_row1: usize, mi_col0: usize, mi_col1: usize) -> TileWriter`
(mi units, end exclusive; the default is the whole frame, so single-tile streams do not change).
- `ContextState` keeps `tile: [row0, row1, col0, col1]`; every neighbour test written `r > 0`,
  `c > 0`, `row == 0`, `col == 0` becomes `r > row0`, `c > col0`, `== row0`, `== col0`
  (syntax/ctx.rs around lines 61, 79, 88, 200-239, 313-332, and the inter-info contexts in
  inter_info.rs that look at the above/left block).
- `if sb.col == 0 { self.ctx.clear_left() }` (syntax/mod.rs, `write_sb_ex` and `write_inter_sb`)
  becomes `sb.col == tile col0 / sb4`.
- A fresh writer per tile already gives clear_above_context and fresh CDFs; `LrWriter` must start
  with the default reference values per writer. The driver only calls `write_*_sb` for the SBs of
  its tile, so units are coded by the tile that contains them (spec `read_lr`).
- `cdef_idx` stays addressed in frame coordinates; no change.
- Test: a writer for the top-left tile rect fed the same blocks as a standalone picture of that
  size must produce the same bytes.

## R2 motion/ (av1 lead): tile-bounded MV scan

- `find_stack(mi, frame, shape, ref_frame)` takes the tile: `mi_row_start/end`, `mi_col_start/end`
  instead of `0..frame.mi_rows/mi_cols`, and `avail_u = r > row0`, `avail_l = c > col0`
  (motion/mvpred.rs lines 39-50). The frame-level `MvFrame` (mi_rows/mi_cols for clamps) stays.
- `InterFrameCoder::with_tile(row0, row1, col0, col1)`: builds its `ContextState` bounded by the
  tile (R1) and passes the bounds to `find_stack`; the SB loop stays with the caller.
- Inter prediction reads the shared reference (`Arc<FrameBuf>`, immutable) at frame coordinates:
  untouched. `RefStore` must be `Sync` (plain data).

## R3 mode/ and recon/intra.rs

Nothing new: `FrameBuf::set_tile(mi_row0, mi_row1, mi_col0, mi_col1)` already restricts intra
availability to a tile. Please confirm with a test that `have_above_right` / `have_below_left` and
the intra mode context in `mode/` stop at the tile edge for an interior tile. recon/ is av1-10bit's;
no request unless the confirmation fails.

## R4 encoder.rs (av1 lead): the frame loop

```text
let (layout, cl, rl) = TileLayout::for_frame(&self.seq, self.cfg.tiles.0, self.cfg.tiles.1);
// FrameParams.tile_cols_log2/rows_log2 = cl, rl; drop the Unsupported("tiles") check
let tiles = tiles::encode_tiles(&layout, self.threads(), |i, rect| {
    let (c0, c1, r0, r1) = rect.mi(64, mi_cols, mi_rows);
    let mut buf = FrameBuf::new(w, h, depth);   // own full-frame buffer, written only inside the tile
    buf.quant = ..; buf.enable_intra_edge_filter = false;
    buf.set_tile(r0, r1, c0, c1);
    let mut tw = TileWriter::new(&self.seq, q, mi_cols, mi_rows).with_tile(r0, r1, c0, c1);
    // decider / InterFrameCoder::new(..).with_tile(..)
    for row in rect.sb_row0..rect.sb_row1 { for col in rect.sb_col0..rect.sb_col1 {
        decide_sb; write_*_sb; keep (pos, sb) } }
    TileOut { buf, mi: tw.mode_info().clone(), sbs, bytes: tw.finish() }
});
// per tile i: tiles::merge_recon(&mut frame_buf, &t.buf, layout.rect(i), 64);
//             tiles::merge_mode_info(&mut frame_mi, &t.mi, layout.rect(i), 64);
```
- `frame_buf` / `frame_mi` start as `FrameBuf::new` / `MiInfo::new(mi_cols, mi_rows, false)`; the
  filters then run on them as they do now.
- The rewrite after the filter search (cdef bits > 0 or LR) is per tile and also parallel:
  `encode_tiles` again, each tile with `TileWriter::new(..).with_tile(..).with_cdef(..).with_lr(..)`
  rewriting its stored `sbs`.
- Frame OBU body: `write_frame_header(..)`, `byte_align()`, then `tiles::tile_group(&tile_bytes)`
  (replaces `w.put_bytes(&tile)`); with one tile it is the same bytes as now.
- Filters: call `filters::search_deblock_mt(.., self.threads())` and
  `filters::search_cdef_mt(.., self.threads())`. The old names stay and mean one thread.
  `search_lr` already takes `threads`.
- `Config.tiles` default stays (0,0); (2,1) = 4x2 is the sensible 1080p setting.

## Caveat

Tiles cost bytes (fresh CDFs per tile, weaker prediction at tile edges, size fields). The cost at
equal PSNR will be measured once R1-R4 have landed. Same bytes at threads 1 and N needs no shared
mutable state between tile coders and merging in tile order.
