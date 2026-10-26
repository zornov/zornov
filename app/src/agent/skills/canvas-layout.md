---
name: canvas-layout
use: Use when placing things on the canvas: overlaps, what is in front, safe areas, free space.
topics: canvas_layout, canvas-layout, layout-knowledge, where, positions, overlap, overlaps, free-space, placement-check, collisions
tools: canvas_layout, inspect, see, check, review, restack, set
---
**Order**
1. Do not guess where things are. `canvas_layout` reads the project itself: for any moment or range, where every visible thing is, from what the renderer really draws. Positions are canvas pixels from the centre, y down, like `x` and `y` everywhere.
2. Before placing anything: `view: "free"` (empty boxes, balance, safe areas), or `view: "fit"` with `size: [w, h]` (or `like` a clip) and `prefer` (`top`, `bottom`, `lower_third`, `left`, `right`) or `near` (a clip or `[x, y]`): the spots where it fits, clear of everything drawn.
3. After moving things: `canvas_layout t: 2.5` for the whole frame (compact, front to back), `view: "overlaps"` for what covers what, `view: "safe"` for title-safe and action-safe violations.
4. One thing: `view: "item", clip: 12` gives the box of what shows (after transform, crop, mask and effects; transparent padding is not counted), corners, rotation, depth, what covers it and by how much, the nearest things, motion (with `from`, `to`), a puppet's head, hands, feet, mouth and which way it faces, text line by line, 3D things projected through the project camera.
5. A stretch of film: `from` and `to` list who is there, how they move and where they meet. `draw: false` gives boxes from the placement only, fast.
6. Reading an item: `box` is what shows; `vis` the share not hidden; `over` and `under` name the clips it hides or is hidden by; `flags` say `cut_left`, `off_screen`, `hidden`, `outside_title_safe`; `labels` are what object detection found in the picture.

**Checks**
- A title goes where `fit` says, inside the title-safe box, never over a head or a face. `check` and `review` report `title_over_face`, `title_over_subject`, `layout_overlap`, `off_screen`, `cut_by_frame`, `title_unsafe`, `hidden_behind` and `faces_away_listener` from the same read.
- Two characters get their own spots (`fit` with `like`). A character who speaks faces who it speaks to (`faces` in the answer; flip with `"flip": true`).
- In front is the higher `z`; change it with `restack`, not by moving the thing away.
- `inspect` and `see` with `t` carry a short layout line; ask `canvas_layout` for the numbers.

**Mistakes**
- Using the transform box: padding and crop make it bigger than what shows; read `box`.
- Asking for every frame: ask one moment, or a range with few `samples`; each thing is drawn alone, so a big scene costs a little time.
