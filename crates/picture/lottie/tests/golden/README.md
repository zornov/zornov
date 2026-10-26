# Lottie goldens

Frames of the fixtures in `../fixtures` drawn by an independent player:
EffectCraft's Lottie importer and CPU compositor
(<https://github.com/storytold/effectcraft>, revision 9919048, MIT OR Apache-2.0),
through a small local program that renders a frame to RGBA. `golden_frames_match`
checks this player against them; `write_goldens` (ignored) rewrites them from a
folder of such renders.

shapes-gradient has no golden: there the two players differ on where a
rectangle's path starts under trim paths, and lottie-web 5.12.2 (checked by hand)
starts it where this player does.
