---
name: motion-animation
use: Use when something must move well: keys, easing, loops, noise, physics, drivers.
topics: animation, keyframes, arcs, easing, curves, modifiers, modifier, noise, loops, loop, cycles, procedural, stepped, physics, dynamics, rigid-body, simulation-2d, drops, bounces, piles, drivers, driver, expressions, links, relationships, pick-whip, stagger, motion-presets, simulation
tools: animate, keys, keyframe, set, motion, stagger, modifier, physics, driver, expression, time_remap, use_move, use_design, save_move, save_design, graph, inspect, frame, check, merge, unmerge, restack
---
**Order**
1. Say what the move is for (arrive, emphasise, leave, loop, react) before touching keys.
2. Presets first for standard entries, exits and loops: `motion` sets `entry`, `exit` and `loop`. Keys (`animate`, `keys`) for a designed move; modifiers (`modifier`: noise, loop, stepped) and `expression` for behaviour that must keep going.
3. Choose the curve by the job: arrivals start fast and settle (power2_out), exits accelerate away (ease in), never ease in on an entrance; position travels on arcs, other properties do not. Relative values are factors (1.0 is as is).
4. Many things, one after another: `stagger` (letters 0.03-0.06 s apart, words 0.08-0.15 s, list items 0.3-0.5 s) in reading order. A move you will use again: `save_move` / `save_design`, then `use_move` / `use_design`.
5. Relationships that must hold whatever happens: `driver` (one property follows another). Falling, bouncing, piling: `physics`.
6. Check the move itself, not only still frames: `graph` reads the curve (overshoot, kink, hold) and `frame` with `every: 0.05` shows the move; reshape with `keys` (`curve` per key or exact handles).

**Checks**
- No kink (speed jumping at a key), no accidental hold, no overshoot you did not choose.
- One or two effects on a moving thing, not a pile.
- The move reads at the speed of the cut: entrances 0.3-0.6 s, exits 0.25-0.4 s.

**Mistakes**
- An `expo_in` fill that sat still then jumped; fills use fast-start, settle curves. A scaled bar squashes its round ends: animate `width` with `anchor: "left"`.
- Arcs on x and rotation: arcs are for position only.
- Unmerging and re-merging a scene to change one thing: edit inside merged clips by id.
- Looking only at stills 0.7 s apart and missing a two-frame snap.

## Reference: Motion and keyframes

- `animate` keys values over time; `ease` sets the feel (see the Motion
  guide). Arrivals: `_out` curves (power2_out, expo_out, back). Moves the
  eye follows: `smooth` / `sine_in_out`. Exits and hits: `_in`, `snap`.
- **Keyframes vs arcs**: keys set *when* a value changes; `path` and
  `arc` bend *where a position travels*. Arcs and paths are only for
  position (`property: "position"`), never for rotation, scale or
  opacity: give those a curve instead.
- **Relative** keys: scale and opacity values are factors (1.15 = 15%
  more), position and rotation are offsets.
- Stagger groups (`stagger`) rather than moving many things at once;
  parent followers to a leader (`parent`) rather than keying each.
- Read the curve (`graph`): a smooth move has one hump of speed per
  move and no kinks; a kink at an inner key looks like a hitch, so ease
  through it (`smooth`) or move the key. Overshoot belongs to arrivals
  (back, elastic), never to a slow drift.
- Look at the move itself (`contact_sheet` with `every: 0.05` over it);
  `check` reports `motion_snap`, `motion_stall`, `speed_step`.

## Reference: Procedural motion: modifiers and loops

- Ambient life (a floating logo, a hand-held camera, a flickering
  light) is a `modifier` noise, not dozens of keys: small amounts
  (rotation 1-3°, position 4-12 px, scale 1-3 %, a light's intensity
  5-15 %) and slow rates (0.5-2 per second). Fast noise (6+ per second)
  reads as shake: impacts and glitches only, confined with `from`/`to`.
- Camera shake on a camera layer or a parent null moves everything
  together; noise on each layer separately looks broken unless seeds
  and amounts differ on purpose.
- Loops: key one clean cycle, then `cycles`. `offset` for things that
  keep going (spins, scrolls, walks), `mirror` for things that swing
  back (pendulums, breathing), `repeat` for things that reset (a blink,
  a pulse). Check the seam: the last key's value and speed should meet
  the first's, or the loop hitches.
- `stepped` at 8-12 poses a second gives smooth animation a hand-made,
  stop-motion feel; put it on every moving property of the piece, not
  one, or it looks like dropped frames.
- `limits` keeps a wobble or an overshoot from going somewhere it must
  not (opacity past 100 %, a box past the frame edge).

## Reference: Drivers: relationships that hold

- Drive what must stay true when the source changes: a shadow's
  position and blur from a ball's height, a counter's number from a
  bar's width, wheels' rotation from a car's x (rotation = x / (π ·
  diameter) · 360), echoes of a hero layer each a little later
  (`delay` 0.05-0.15 s apart, opacity falling off).
- Keep formulas readable: one variable per thing it depends on,
  named for what it is (`ball_y`, not `a3`); scale with `multiply`
  rather than a formula when that is all it does.
- A driver overrides keys on that property (they only reach it as
  `value`): to animate on top of a driver, use `value + …` in the
  formula, or drive a parent and key the child.
- After driving, scrub the source (`frame` at a few moments) and check
  the follower lands where it should at the extremes.

## Reference: Physics: drops, bounces, piles

- A pile of cards, stickers or letters landing reads as real only with
  differences: start them at different heights and tilts (a few degrees
  to 30°), give some a `push` sideways and a little `spin`, and stagger
  their start positions so they don't land at once.
- `bounce` 0.2-0.4 for paper, cards and UI; 0.6-0.8 only for balls and
  rubbery things. `friction` 0.4-0.7 keeps them from skating.
- Short runs: most drops settle in 1.5-2.5 s; cut or move on once they
  rest. Letters that drop into a word: `static` a hidden box under the
  line as the floor, `walls` to keep the pile on screen.
- It is baked: after the run, check the last frame (`frame`) and tidy a
  key or two if one ends up covering something that matters.
