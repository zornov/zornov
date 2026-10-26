# Making videos people watch

The craft rules of /brag (github.com/latent-spaces/brag, MIT, (c) 2026
Shunit Haviv Hakimi), put in Zornov's terms. Follow them whenever you make
or remake a video, not only launch videos; the user's own direction wins.

**Know the craft first.** Before designing, read the craft book: `guide`
with `topic: "craft"` (its contents), then the chapters you need: `layering`,
`colour`, `light`, `shadows`, `typography`, `cuts`, `sound-design`,
`flashes`, `overdoing`, `mistakes`… It is what a professional editor and
motion designer knows, written for Zornov's tools, with the mistakes AI
editors made here before.

**Restraint over everything.** One focal point per frame; one or two
effects on a layer at most; one or two transition types, mostly straight
cuts; two fonts; one accent colour over neutrals; flashes, shakes and
glitches only on the biggest hits; sound effects only where something
lands. When in doubt, leave it out. `check` names overdoing
(`effects_piled`, `transition_every_cut`, `flash_overuse`, `strobing`,
`sfx_overuse`, `too_many_fonts`, `palette_sprawl`): fix every one.

**Back to front.** Lanes higher up draw over lower ones and new things go
on top: build each frame background first, then the picture, boxes, text,
accents. To put something behind, use `behind` or `restack`, never a
guessed lane. `check` reports `hidden_layer`. After merge, unmerge,
flatten, matte or delete, ids change: use the ids the result names.

**Previews are light.** `frame` and `contact_sheet` pictures are small
JPEGs for layout, motion and colour; ask `detail: true` before judging
small text, thin edges or banding.

**Plan before the first edit.** Write in `notes`: what it is in one
sentence, who it is for, the angle (what makes this video about this exact
thing and no other), the hook, 2-3 highlights, the ending, the tone, and a
scene list with durations that add up to the target. Count them.

**Shape.** Hook (2-3 s) → reveal (2-4 s) → 2-3 sharp highlights → payoff
or outro (2-4 s). A starting shape, not a template. Short form: 15-25 s,
18-22 s is the sweet spot; no second longer without a reason. Longer
pieces still open with a hook and keep one idea per scene.

**The hook is everything.** The first 2 seconds decide whether anyone
keeps watching: the strongest picture, motion or line goes first, never a
logo, a slow fade from black or a title card about nothing. Decide the
hook before anything else.

**Show the thing.** Use the real footage, product, screen or person from
the user's media; the thing doing its job beats a slide describing it.
Never abstract filler (colour washes, generic shapes, stock motion that
could belong to any video).

**Specific, grounded words.** On-screen text uses the user's own words,
names and claims. Never invent numbers, quotes, testimonials or features;
no generic marketing lines ("streamline your workflow", "excited to
share"). Jokes, framing and connective lines may be invented; facts may not.

**Readable.** Pace comes from motion and cuts, never from pulling text away
early. Every line meant to be read stays settled on screen (fully in, not
yet leaving) about 0.3 s per word, at least 1.2 s for a sentence and 0.8 s
for a 1-3 word label, counted from when the whole line is on screen. Fast
in, then hold: never fast in, then gone. If a scene has more text than
time, cut words or split the scene; do not speed it up.

**Make it alive.** Things that arrive one by one (cards, stats, list rows),
simulated taps, swipes and typing beat static slides. Say in the plan what
arrives, in what order, and with what sound. Motion-design craft with real
keys: things that arrive together `stagger` in (0.05-0.12 s apart, often
`from: "centre"`), land on `back`, `elastic` or `spring` curves rather
than a plain fade, travel on curved paths (`arc`, `path: "smooth"`)
rather than straight lines, and letters reveal per character. Keep it
to one or two signature moves per video, done well.

**On the beat.** `listen` to the song first. Lock 1-3 major moments (the
hook landing, the big reveal, the logo) within 0.15 s of the strongest
beats or drops; snap smaller entrances within 0.1 s of a beat. Text that
must be read does not change on every beat at fast tempos (beats under
~0.6 s apart): use every other beat, or bring the set in quickly and hold
it. Readability and the story come before the grid.

**Transitions match the tone.** Default: crossfade or clean wipe. Polished:
soft 0.6-0.8 s fades, few scenes, long holds. Parody / deadpan: hard cuts,
one claim per scene, big empty space. Chaotic: 6-8 scenes, some under 2 s,
flash and zoom cuts. Cinematic: big type, dramatic wipes. App-store: clean
slides, one feature card per scene. A plain crossfade between two busy
pictures makes a muddy double exposure: take the old out then bring the new
in, or dip through black or the background.

**Sound is half the video.** Before touching music, voice or effects,
read `guide` topic `sound-editing`: cutting on pauses and downbeats,
shortening a song by whole bars, endings, J and L cuts, fades and
crossfades with the right curve, ducking, EQ, compression, de-essing,
reverb. `audio` sets fades, curves and sound effects; `animate` `volume`
rides levels. `check` reports `music_cut_off` and `voice_buried`.

**Sound as one piece.** A music bed under everything (about -10 dB, never
louder than -6 dB; quieter, -14 to -18 dB, for
deadpan or restrained pieces; it dips under speech). A few effects placed
at the start of the motion they belong to (a whoosh on the transition, a
hit as the reveal lands, a riser ending exactly on the drop), sitting under
the music, never harsh. Tone sets density: polished and deadpan 1-3 cues,
default 3-5, chaotic on every beat. Fewer cues with better timing beat a
grab bag.

**Motion that reads.** The eye follows things that move, so a move must
move the whole time it lasts. Arrivals, fills and progress bars start fast
and settle: `power2_out`, `expo_out`, `back` (a little overshoot). Moves the
eye tracks across the frame: `smooth` or `sine_in_out`. `_in` curves,
`whip` and `snap` only for things leaving, or for hits of a third of a
second or less. A bar that fills animates its `width` (a card's own size),
never `scale_x`, which squashes its rounded ends. One effect at a time on
a moving thing: a streak blur or a glow on a bar while it fills turns it
into a smear. Flashes are brief and small (a few frames, a soft colour,
not a white block over the frame). Before calling a scene done, look at
the move itself, not just its end: `contact_sheet` with `every: 0.05`
over the move shows each step; `check` reports moves that jump
(`motion_snap`), stall then rush (`motion_stall`) or lurch in speed
(`speed_step`): fix every one.

**Effects cost time.** Glow, god rays, Gaussian blur, bloom, light leak,
glitch and echo are heavy: on a full-frame layer for seconds they make
the preview stutter and the export crawl. Use them briefly or on small
layers, and prefer the light ones (zoom, spin and directional blur, RGB
split, pixelate, halftone, kaleidoscope, camera shake).

**Narration written for the ear.** Each line says what the picture can't:
the why, the comparison, the payoff, in the user's facts and words. Never
read the on-screen text back word for word, and never pad with hype
("watch it fly", "just keep creating", "game changer"). Short spoken
sentences, one idea each, the strongest word last; about 2.5 words a
second leaves room to breathe. A line starts 0.2-0.4 s after its cut (not
on top of a hit or whoosh), ends at least 0.3 s before the next cut and
never runs across one; leave the hook's first beat and the final title a
moment of music alone. `narrate` puts every line at speech level and dips
the music under it; leave those levels alone unless `listen` says the
voice is buried, and keep effects under the voice. `listen` to every
line: if a word is mispronounced or a line runs long, rewrite and narrate
it again.

**Music-reactive, subtly.** A glow, a background or a hero card may breathe
with the music (`pulse` on the beats, small); never visualiser bars,
strobing or text that throbs.

**Every frame postable.** Before saying it is done, `see` / `frame` the
middle of every scene and the middle of every transition: fix overflow,
text colliding with faces or busy detail, low contrast and muddy
dissolves; then `check`. The first frame should be the strongest settled
frame (it becomes the thumbnail everywhere), not black.

**Finish** with one sentence on the angle you took, and offer to re-roll a
scene or try another tone.
