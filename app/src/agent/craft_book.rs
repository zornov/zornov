// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The craft book: the editor's and motion designer's knowledge, served as
//! `guide` topics (`craft` for all of it, or one part by name).

pub const BOOK: &str = include_str!("craft_book.md");

/// The topic names, each with the section heading it opens.
const TOPICS: &[(&[&str], &str)] = &[
    (&["layering", "layers", "hierarchy", "stacking", "order", "depth"], "## Layering"),
    (&["placement", "composition", "layout", "safe", "alignment", "spacing"], "## Placement and composition"),
    (&["anchor", "anchors", "scale", "scaling", "rotation", "rotations", "crop", "crops"], "## Anchor points, scale, rotation, crop"),
    (&["opacity", "visibility", "blend", "blending", "blends", "mattes"], "## Opacity, visibility and blending"),
    (&["shadows", "shadow", "depth", "glow", "glows", "blur", "gaussian"], "## Shadows, depth and glows"),
    (&["shapes", "boxes", "radius", "roundness", "outline", "stroke"], "## Shapes and boxes"),
    (&["cuts", "transitions", "transition"], "## Cuts and transitions"),
    (&["fades", "fade"], "## Fades"),
    (&["flashes", "flash", "surprise", "surprises"], "## Flashes and surprise"),
    (&["markers", "marker"], "## Markers"),
    (&["searching", "search", "catalogue-search"], "## Searching"),
];

/// The first topic word of each chapter.
pub fn topic_words() -> Vec<&'static str> {
    TOPICS.iter().map(|(n, _)| n[0]).collect()
}

/// All the topic words, for error messages.
pub fn names() -> String {
    TOPICS.iter().map(|(n, _)| n[0]).collect::<Vec<_>>().join(", ")
}

/// A part of the book by topic name, or the whole book for `craft`.
pub fn topic(name: &str) -> Option<String> {
    let n = name.trim().to_lowercase().replace([' ', '_'], "-");
    if matches!(n.as_str(), "craft" | "book" | "editing" | "design") {
        return Some(BOOK.to_owned());
    }
    let heading = TOPICS.iter().find(|(names, _)| names.contains(&n.as_str()))?.1;
    let start = BOOK.find(heading)?;
    let rest = &BOOK[start + heading.len()..];
    let end = rest.find("\n## ").map_or(BOOK.len(), |i| start + heading.len() + i);
    Some(BOOK[start..end].trim().to_owned())
}

/// The book as a contents list: each chapter with its size and the word
/// that opens it. `craft` gives this rather than the whole book (about
/// 100 000 characters, more than any job reads).
pub fn contents() -> String {
    let mut s = format!("# The craft book: {} chapters, {} characters in all\nRead one with `guide {{\"topic\": word}}` (a long chapter comes as its opening and an outline of its parts; `part` reads one, `full: true` all of it); `guide {{\"topic\": \"craft\", \"full\": true}}` is the whole book, which you almost never need.\n\n", TOPICS.len(), BOOK.len());
    for (names, heading) in TOPICS {
        let size = topic(names[0]).map_or(0, |t| t.len());
        s.push_str(&format!("- {} ({size}): {}\n", heading.trim_start_matches("## "), names[0]));
    }
    s.push_str(&format!("\nHow to do a kind of work (stories, characters, 3D, sound, colour, titles, vector, mattes, effects, export, review) is in the skills: {}. `skill {{name}}` loads one.\n", super::skills::names()));
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_topic_finds_its_section() {
        for (names, heading) in super::TOPICS {
            assert!(super::BOOK.contains(heading), "{heading}");
            assert!(super::topic(names[0]).is_some_and(|t| t.starts_with(heading)), "{}", names[0]);
        }
        assert!(super::topic("craft").unwrap().len() > 5_000);
        assert!(super::topic("nonsense").is_none());
    }
}
