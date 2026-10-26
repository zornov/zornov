---
name: objects-in-footage
use: Use when you need what is in footage: title room, a moment by words, an object to cut out or follow.
topics: objects, object, detect, detection, find, find-moment, search, free-space, space, ocr, text-in-picture, depth, distance, size, relations, in-front, behind, what-is-in
tools: objects, see, cut_out, roto, track, text, place, frame, set, animate, check
---
**Order**
1. Read the shot before using it: `objects {clip}` (or `see`, which adds one line of objects per shot once a shot is read). The first read takes seconds a shot and fetches the models once; every later question about the same footage is free. Add `names: ["red kettle"]` for things that are not in the usual vocabulary of common objects.
2. Ask the question you have, not for everything: action `where` (object, t: box in canvas px, distance and size in metres), `when` (object: comes in, leaves), `around` (object, t: on, holding, in front, behind, near), `space` (t, to: clear rectangles), `find` (query: the moment that matches a description), `text` (words in the picture).
3. Use the numbers as they come. Boxes are canvas pixels from the centre, y down: a `where` box goes straight into `text`, `place`, `cut_out {box}` or `track {point}`.
4. Titles and graphics: ask `space` (t, to) for the time the title will be on screen, put the title in the first rectangle, and look at one frame. Keep a figure's face and the subject clear; a lower third goes in a bottom rectangle, a title where the shot has most room.
5. Behind or beside a thing: `around` says what is in front of and behind an object. To put text behind a person, cut the person out (`objects` with action `mask`, object, t and apply true, or the `cut_out` op it prints), put the title on the lane under the cut-out and the footage under both.
6. Following an object: `mask` prints a `track` op with a point inside the object; run it, then attach the graphic to the track. For a graphic that must follow a thing only while it is on screen, take the times from `when`.
7. Finding material: `find` (query) over the main-lane shots (or `clips`) gives timeline seconds with a strength (strong, likely, weak). Look at the best one with `frame` before cutting there.

**Checks**
- Distances and sizes are marked rough when nothing of known size (a person, a car, a door) is in view. Trust "this is nearer than that" and the order in `around`; do not trust metres then.
- A box is the detector's, a few percent loose; `mask` gives the tight box and a point inside.
- The camera's motion is separate from an object's: an object "still in the place" while the camera pans moves across the frame, so a fixed title on it needs a `track`, not keys.
- A score under about 30 percent is a guess; look at a frame before building on it.
- `find` strength: a score of 0.10 and above is a strong match, 0.055 to 0.10 likely, below that weak. Scores are small numbers; compare them with each other, not with 1.

**Mistakes**
- Asking for the whole list of a busy shot again and again: the list is the 12 most prominent; ask by name, or with `t`, for the rest.
- Reading every shot of a long timeline up front: read the shots you will work on (`clips: [ids]`), the others cost seconds each.
- Using ids across shots: `#3` belongs to one shot; the answer says which clip.
- Putting a title on the subject because the middle looked empty: `space` already keeps clear of every object, the text found in the picture and the busiest parts.
- Cutting out with a point on a big thing when `mask` already found its box and a point inside.

## Reference: tool `objects`

What is in the footage and pictures, as a map read once per piece of footage and kept by its content. Each thing has an id (`#3`, per shot), a label with a score, a box in every sampled frame, how far away and how big in metres, main colours, text on it, a face and body points for people, which way animals and people face, when it comes in and leaves, how it moves apart from the camera, and what the camera does.

`action`: list (default), where, when, around, find, space, text, mask. `object`: an id or a name. `t`, `to`: timeline seconds. `clip`, `clips`: shots (default: the clip at `t`, else every main-lane shot). `names`: extra things to look for. `detail`, `max`: more or fewer lines. `quick`: fewer key frames and no text. `refresh`: read again. `apply` (mask): run the cut-out.

## Reference: How the map is made

The object finder (OWLv2, Apache-2.0) looks at a few key frames (about one every 1.5 s, at most six a shot) for the usual objects plus any `names`. Between key frames the motion tracker carries every box, so a shot costs the finder's time per key frame, not per frame. At each key frame detections are matched to the objects already followed (they keep their ids), new objects are tracked backwards to find when they came in, and objects the finder stops seeing are ended. Depth (Depth Anything V2 small) gives relative distance; things of known size (a person 1.7 m, a face 0.22 m, a car 1.5 m) fix the scale, so distances and sizes are in metres when such a thing is in view and rough otherwise. Faces and body points come from BlazeFace and BlazePose, text from PaddleOCR, picture-text matching from SigLIP (Apache-2.0), cutting from SAM 2.1.

## Reference: Relations

- **on**: its bottom rests on the top of the other thing, over it, and it is smaller.
- **holding**: a person's hand is on a small thing at about the same distance.
- **in front of / behind**: the boxes overlap and the distances differ by more than a few tenths of a metre.
- **near**: close together on screen and at about the same distance. Left and right are given by `around`.
