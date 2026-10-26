---
name: lighting-mood
use: Use when a 3D scene needs mood: key light, lamps, flames, fog, rain, wet ground, exposure.
topics: atmosphere, atmospheric, mood, practicals, practical, lamps, lamp, candle, candlelight, neon, flame, flames, glow, bloom, rain, rainy, snow, snowy, fog, foggy, mist, haze, embers, sparks, wet, wetness, puddles, reflections, cinematic, night, dusk, dawn, noir, tonemap, look-3d
tools: lighting, light, atmosphere, model, mesh, asset, camera, frame, check, review, notes, set, animate
---
**Order**
1. Say the mood, the time and the weather in one line in `notes` ("tense night street, rain, one warm lamp, wet road"). Everything below follows from it. Mood to set-up: hopeful or cosy golden_hour or firelight, lonely blue_hour, tense or noir low_key, romantic butterfly, scary under, party neon, epic rim.
2. The key first. `lighting {preset, on: subject}` gives a key with shadows, a weaker fill and a rim; the strongest light must lead the next by at least a quarter (or `check` says `3d_flat_key`), and the ambient stays at 10 to 25 percent of the key. A look with no direction is flat however nice the fog is.
3. Every lit thing in the picture is two things. The lamp, candle, bulb, neon tube or window is a model with `emissive` and `emissive_strength` (neon 4 to 8, bulb 6 to 10, window 2 to 4, flame 8): `mesh {clip, do: "material", emissive: "#ffb060", emissive_strength: 8}`; and at it a point or spot `light` in the same colour that shows itself: `glow` 0.8 to 1.2, `flare` 0.2 for a lens streak, `beam` 0.3 to 0.6 when there is air, `source_size` in metres (bulb 0.05, lantern 0.15, window 1). A flame is `light {flame: true, flame_height: 0.05 candle, 0.3 torch, 0.8 campfire, 2.5 bonfire, flicker: 0.35, glow: 0.6}`. Light colours: tungsten #ffb060, sodium #ff9a40, moon #9fb6ff, neon its own.
4. Air. `atmosphere {preset}` sets fog, weather and look together (presets: neon_rain, candlelit, foggy_dawn, snowy_dusk, desert_noon, overcast_mist, storm, ember_smoke, noir_haze, clean_film; `atmosphere {list: true}` says what each is); then fields change it: `fog_density` 0.01 haze, 0.03 mist, 0.08 fog; `fog_falloff` 0.1 to 0.2 keeps it near the ground; `fog_colour` from the light behind it; `fog_anisotropy` 0.5 makes halos round lamps. `ground: "auto"` takes the floor from the lowest model; without it the floor is world y 0. Beams and shafts exist only in air: no fog or dust, no beam.
5. Weather. `weather: ["rain"]` or objects `{kind, amount, wind: [x, y, z], size, speed, zone}` (rain, snow, dust, embers, sparks, leaves, ash, smoke; three layers at most; `zone` keeps smoke over a chimney and embers over a fire). Rain and snow are seen where light catches them: put a lamp or a bright sky behind them, `amount` 0.5 to 0.8. Dust motes belong in a beam. Embers and sparks need a fire in the shot.
6. Wet ground. `model {wetness: 0.7}` on the street or `mesh {clip, wetness}` (0.3 damp, 0.7 after rain, 1 puddles on everything flat): wet surfaces darken, turn glossy and mirror the lamps and signs, so a wet street with no glowing lamp has nothing to reflect. Rain on a wet floor splashes.
7. The look. Filmic tone map is the default and the exposure meters itself: `exposure` only moves it (-0.5 for moodier). Richness 0.4 to 0.6 keeps blacks deep and coloured, contrast 1.1 to 1.2, vignette 0.15 to 0.3, `bloom_strength` 0.15 to 0.3, `halation` 0.1 to 0.3 on night lights, `temperature` up for warm and down for cold, `shadow_tint` a teal or blue for night. Turn nothing up to make a scene brighter than its lights are; add a light.
8. Look at it: `frame` at the key moments, from the camera the film uses, then `check` and `review {only: "3d"}`, and fix each note with the fix it gives.

**Checks**
- The subject stands off its background (a rim, or a background 30 percent lighter or darker): `3d_no_separation`.
- Blacks are deep and still show detail, not grey, not pure black: `3d_muddy_blacks`, `3d_crushed_blacks`.
- Nothing is white where there should be detail, and glare does not wash the shadows: `3d_blown_highlights`, `3d_over_bloom`.
- A key light leads: `3d_flat_key`.
- Every lamp, candle and neon sign shows itself and every flame is a flame: `3d_source_no_effect`, `3d_flat_source`.
- The weather and air the request names are on screen: `3d_air_missing`.
- Set aside only what the brief asks for (a white high-key studio, a flat low-poly look) with `3d-ok: blown_highlights - white studio` in `notes`.

**Mistakes**
- A bright scene made by raising bloom or exposure: it blows out. Raise the key or the lamps and leave bloom at 0.15 to 0.3.
- Lights with no glow on a foggy night: the viewer sees lit walls and no lamps. Give each practical `glow`, and `beam` when the air is thick.
- Fog with no `ground`: it hangs at the middle of the frame, not on the floor.
- Fog so thick the subject vanishes: past 0.08 the subject needs a rim or a light of its own.
- A flame drawn as a yellow card or a light with no `flame`: it does not flicker and has no shape.
- Rain in an unlit scene: nothing shows it. Weather is lit by the scene's lights.
- Emissive at 1 on a lamp: it reads as a pale surface and cannot bloom or light the air.
- Expecting a walkable world (`build_world`) to take this skill's lights and weather. A world layer takes the air and look clip's fog (density, thinning, colour) and its exposure, bloom, saturation and vignette, and nothing else; its sun, sky and lamps stay its own (`edit_world` with `sky`, `time_of_day`, `lights`, `fog`, `look`), and rain, snow, wet reflections, glow, flames and the tone map are not drawn in it (a path-traced render of a world does draw them).

## Reference: Values by mood

| mood | key | air | weather | look |
|---|---|---|---|---|
| neon night | two coloured spots, low ambient | fog 0.02, falloff 0.1, anisotropy 0.5 | rain 0.7, floor wetness 0.9 | filmic, richness 0.6, bloom 0.3, halation 0.2, teal shadows |
| candle room | one flame light 2x intensity, shadows on | fog 0.012 warm | dust 0.9 | richness 0.5, temperature 0.2, vignette 0.35, grain 0.15 |
| foggy dawn | low sun with shadows, `beam` 0.35 | fog 0.011, falloff 0.2, patchiness 0.5 | dust 0.8 | saturation 1.2, contrast 1.15, warm highlights, cool fill |
| snowy dusk | blue sky fill, lit windows (spots, glow 0.9, beam 0.7) | fog 0.025 cold | snow 0.9 | temperature -0.2, richness 0.3 |
| desert noon | high sun, hard shadows | fog 0.003 warm | dust 0.5, wind 4 | contrast 1.25, saturation 1.25, richness 0.45 |
| storm | flat cold key | fog 0.03 grey | rain 1, wind 6, floor wetness 1 | saturation 0.85, vignette 0.3 |

## Reference: How the checks read a scene

`check` (with frames) and `review` measure a few frames of the 3D shots. Blown: over 5 percent of the frame pure white, or over 12 percent with a channel at maximum. Crushed: over 35 percent pure black. Muddy: the darkest 2 percent above 12 percent brightness and the frame spanning under 60 percent of the tones. Over-bloom: bloom at 0.6 or more, or 0.25 and over 12 percent of the frame brighter than 85 percent. Separation: the subject is lifted out by leaving it out of a second frame; its brightness, colour and outline are compared with a ring round it. Each note carries the exact op that fixes it for this scene.
