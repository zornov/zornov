---
name: living-places
use: Use when a still backdrop should feel alive: water, foliage, weather, particles.
topics: living, places, particles, regions, weather
tools: living, particles, fx_layer, see, parallax, puppet, sfx, frame, contact_sheet, check
---
**Order**
1. Read what the picture is made of: `see {regions: true, t}` (sky, water, ground, path, trees, rocks, with boxes and where the light is).
2. `living` with recipes that fit those regions: `water` (ripples and glints), `foliage` (sway), `clouds` (drift), `rays` (breathing light). Each moves only inside its own region, so the ground holds still.
3. Weather and time tell story: `rain`, `storm`, `snow`, `mist`, `leaves`, `fireflies`, `dusk`, `night`, `day_to_night`, with `ramp` to bring it in on a beat. `particles` puts one effect where you want it (a `region`, `behind` a character, or in front).
4. A backdrop whose parts the model cannot name: `puppet` with `rig: "backdrop"` and `act: "breeze"`; a separate sky or foreground layer at its own speed gives depth (`parallax`).
5. Give the place a sound bed (`sfx` stream, wind, rain, birds, crickets, fire, waves, forest, night) under the voice, and change it when the place changes.
6. Frame a moment, then look at two moments a second apart: the life must show.

**Checks**
- Motion is subtle unless the moment is about the weather.
- Weather in front of everyone falls in front; weather behind a character is behind.
- Characters standing in the place are matched to its light (skill story-film).

**Mistakes**
- Moving the whole picture instead of its regions: the ground should not sway.
- A storm that arrives at the wrong time: ramp it in as the trouble starts.
- A place with no sound of its own (`review` reports `no_ambience`).

## Reference: Living places

A generated backdrop is a still. Read it (`see` `regions: true`), then
make each part do what it would: water flows and glints, leaves sway,
clouds drift, light breathes, weather comes and goes.

- **One op for all of it**: `living` with `recipes`. `water`: ripples
  flowing along the water (down the picture for a stream running at the
  camera, sideways and slower for a lake) and glints dancing on it;
  `foliage`: trees and plants swaying; `clouds`: wisps drifting across
  the sky; `rays`: light rays from the brightest sky, breathing slowly;
  `dusk`, `night`, `day_to_night`, `night_to_day`: the light changes for
  everything (characters too), stars come out in the sky; `rain`,
  `storm` (heavier, wind, the light dimmed, lightning flashes), `snow`,
  `leaves`, `dust`, `embers`, `magic`, `fireflies` (low, over the
  ground); `mist` / `fog` low over the water or ground. `ramp: [2, 5]`
  brings weather or night in from 2 to 5 seconds into the layers. Each
  recipe is its own layer over the backdrop (weather and light on top of
  everything), named for what it does, with ordinary effects and masks:
  tune or remove any of them.
- **By hand**: an adjustment layer (`fx_layer`) with `water`,
  `noise_displace`, `wave`, `god_rays` or a grade, then `mask`
  `region` on it, puts that effect inside one part of the place. `mask`
  `region` works on any clip (a duplicated backdrop, a particle layer);
  `invert: true` keeps everything but the region.
- **Particles**: `particles` with a `preset`. Every preset is a whole
  look (rain falls in streaks and splashes on the ground, snow drifts
  and settles, leaves tumble and land, sparks fly up, fall and bounce,
  smoke rises and spreads, mist and clouds drift as soft puffs, fireflies
  wander and pulse, stars twinkle). `params` change it: `amount`
  (density, animates smoothly between 0 and 2: rain that starts light
  and gets heavy), `size`, `speed`, `gravity` (times the preset's),
  `wind` (% of the frame height per second; negative blows left),
  `turbulence`, `direction` (degrees added), `colour` (a tint),
  `opacity`, `look` (the sprite: soft dots, streaks, flakes, leaves,
  sparks, puffs, bubbles, confetti, stars, rings), `emitter` (top edge,
  area, point, bottom edge) with `area_a` / `area_b` (0..1 corners).
  Particles are spread in depth: far ones smaller, slower and higher up;
  `ground` (0..1 down the frame, where the nearest land; more than 1.5:
  never) and `horizon` (filled in from the place when you give a
  region) set where falling things land, so rain splashes and snow
  settles on the ground, further back nearer the horizon. Glows (sparks,
  fireflies, magic, embers, stars, glints, dust) screen onto what is
  below. Every setting animates (`animate` `particles.amount`…), and
  the same moment always gives the same particles, so scrubbing and
  export agree.
- **In front or behind**: weather on top falls in front of everyone;
  `behind` (a character's clip) puts a layer just under that character;
  make the layer 3D (`three_d` with `z`) to sort it with other 3D layers.
  Mist behind the characters and a few leaves in front gives depth.
- **Check it**: `frame` a moment, and look at two moments a second
  apart: water and leaves should move a little, never wobble the whole
  picture. Keep it subtle: life, not spectacle, unless the story asks.
