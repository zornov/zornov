// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Guide text served in parts.
//!
//! A guide section can run to 30 000 characters, and everything the AI reads
//! stays in its context for the rest of the job. So a big section is not
//! sent whole by default: the AI gets its opening (the rules that apply to
//! everything in it) and an outline of its parts, and asks for the parts the
//! job needs with `part`. Nothing is hidden: `full: true` gives the whole
//! section, and every part is reachable by its title.
//!
//! A part is a paragraph that opens with a bold title (`**Puppet pins**`),
//! up to the next such paragraph. The split reads the text as written, so a
//! new part in `guide.md` is served without touching code.

/// A section longer than this (in characters) is served as opening plus
/// outline.
pub const BIG: usize = 9_000;

/// One part of a section.
#[derive(Debug, PartialEq)]
pub struct Part<'a> {
    pub title: &'a str,
    pub body: &'a str,
}

/// The title of a part when `block` opens one: a paragraph that begins
/// with a bold phrase (`**Puppet pins**`, `**Layers without taking them
/// apart.** text`, `**Text that moves** (`text`):`). A list item starts with
/// a dash, so it never does.
fn lead(block: &str) -> Option<&str> {
    let rest = block.strip_prefix("**")?;
    let title = &rest[..rest.find("**")?];
    (title.len() >= 4 && title.len() <= 70 && !title.contains('\n')).then_some(title.trim_end_matches(['.', ':']))
}

/// A section split into its opening text and its parts.
pub fn split(section: &str) -> (&str, Vec<Part<'_>>) {
    let mut starts: Vec<(usize, &str)> = Vec::new();
    let (mut pos, mut fenced) = (0usize, false);
    for block in section.split("\n\n") {
        // Blank lines inside a code fence do not start a paragraph.
        let was_fenced = fenced;
        if block.matches("```").count() % 2 == 1 {
            fenced = !fenced;
        }
        if !was_fenced {
            if let Some(t) = lead(block) {
                starts.push((pos, t));
            }
        }
        pos += block.len() + 2;
    }
    let intro_end = starts.first().map_or(section.len(), |s| s.0);
    let parts = starts
        .iter()
        .enumerate()
        .map(|(i, &(at, title))| {
            let end = starts.get(i + 1).map_or(section.len(), |s| s.0);
            Part { title, body: section[at..end.min(section.len())].trim_end() }
        })
        .collect();
    (section[..intro_end].trim_end(), parts)
}

/// What a part holds, in a few words: the names in the first column of a
/// table (the ops of a group), or else its first sentence.
fn gist(body: &str) -> String {
    let mut names: Vec<&str> = Vec::new();
    for line in body.lines().filter(|l| l.starts_with("| `")) {
        if let Some(cell) = line[1..].split('|').next() {
            for n in cell.split('/') {
                let n = n.trim().trim_matches('`');
                if !n.is_empty() && n.len() < 24 {
                    names.push(n);
                }
            }
        }
    }
    if names.len() >= 2 {
        let more = names.len().saturating_sub(60);
        names.truncate(60);
        return format!("{}{}", names.join(" "), if more > 0 { format!(" +{more}") } else { String::new() });
    }
    let text: String = body.lines().skip_while(|l| l.starts_with("**") && l.ends_with("**")).collect::<Vec<_>>().join(" ");
    let text = text.trim_start_matches(|c: char| c == '*' || c == ' ');
    let text = text.split_once("** ").map_or(text, |(_, r)| r);
    let first = text.split_inclusive(['.', ':']).find(|s| s.trim().len() > 12).unwrap_or(text).trim();
    let mut out: String = first.chars().filter(|c| *c != '*' && *c != '`').take(110).collect();
    if first.chars().count() > 110 {
        out.push_str("...");
    }
    out
}

/// The parts whose title matches `want`: exactly, else by prefix, else by
/// containing it (case does not matter). Parts that share a title all come.
fn find<'a, 'b>(parts: &'b [Part<'a>], want: &str) -> Vec<&'b Part<'a>> {
    let w = want.trim().trim_matches('*').to_lowercase();
    if w.is_empty() {
        return Vec::new();
    }
    let tiers: [&dyn Fn(&str) -> bool; 3] = [&|t| t == w, &|t| t.starts_with(&w), &|t| t.contains(&w)];
    for tier in tiers {
        let hit: Vec<&Part> = parts.iter().filter(|p| tier(&p.title.to_lowercase())).collect();
        if !hit.is_empty() {
            return hit;
        }
    }
    Vec::new()
}

/// The table rows of the section that describe `name` (an op or a field in
/// the first column), with the table's header: `guide ops` asked for
/// `split` gives the `split` row, not the page it is on.
fn rows(section: &str, name: &str) -> Option<String> {
    let want = name.trim().trim_matches('`').to_lowercase();
    if want.is_empty() {
        return None;
    }
    let mut header: Option<(&str, &str)> = None;
    let mut out: Vec<&str> = Vec::new();
    let mut lines = section.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("| ") && !line.starts_with("| `") && lines.peek().is_some_and(|n| n.starts_with("|---")) {
            header = Some((line, lines.peek().copied().unwrap_or("")));
        } else if line.starts_with("| `") {
            let cell = line[1..].split('|').next().unwrap_or("");
            if cell.split('/').any(|n| n.trim().trim_matches('`').to_lowercase() == want) {
                out.push(line);
            }
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(match header {
        Some((h, sep)) => format!("{h}\n{sep}\n{}", out.join("\n")),
        None => out.join("\n"),
    })
}

/// The section as the AI should get it. `topic` is the word it asked with.
/// A short section, `full`, or one with no parts is returned whole; a
/// `part` (or several, comma-separated) gives those parts, or the table
/// rows of the ops it names; otherwise the
/// opening and the outline.
pub fn serve(section: &str, topic: &str, part: Option<&str>, full: bool) -> Result<String, String> {
    if full || (part.is_none() && section.len() <= BIG) {
        return Ok(section.to_owned());
    }
    let (intro, parts) = split(section);
    if parts.len() < 3 {
        return Ok(section.to_owned());
    }
    if let Some(want) = part.filter(|p| !p.trim().is_empty()) {
        let mut out: Vec<&str> = Vec::new();
        let mut found_rows: Vec<String> = Vec::new();
        // A title may hold a comma: the whole request first, then each.
        let wants: Vec<&str> = if find(&parts, want).is_empty() { want.split(',').collect() } else { vec![want] };
        for w in wants {
            let hit = find(&parts, w);
            if hit.is_empty() {
                if let Some(r) = rows(section, w) {
                    found_rows.push(r);
                    continue;
                }
                return Err(format!("no part \"{}\" in {topic}; parts: {}", w.trim(), parts.iter().map(|p| p.title).collect::<Vec<_>>().join(" | ")));
            }
            for p in hit {
                if !out.contains(&p.body) {
                    out.push(p.body);
                }
            }
        }
        out.extend(found_rows.iter().map(String::as_str));
        return Ok(out.join("\n\n"));
    }
    let mut s = String::from(intro);
    s.push_str(&format!("\n\n**Parts of this section** ({} characters in all; read the ones the job needs with `guide {{\"topic\": \"{topic}\", \"part\": \"<title>\"}}`, several comma-separated; a name in a table below (an op) gives just its row; `\"full\": true` is everything):\n", section.len()));
    for p in &parts {
        s.push_str(&format!("- {} ({}): {}\n", p.title, p.body.len(), gist(p.body)));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "## Motion\n\nIntro text.\n\n**Text that moves**\n\nWords fly in.\n\n**Layers without taking them apart.** Edit in place.\n\n- one\n- two\n\n```\nA\n\n**Not a part**\n```\n\n**Cinematography**\n\n| op | fields |\n|---|---|\n| `shot` | a |\n| `camera` / `lens` | b |\n";

    #[test]
    fn a_section_splits_at_bold_titles_and_keeps_code_fences_whole() {
        let (intro, parts) = split(SAMPLE);
        assert_eq!(intro, "## Motion\n\nIntro text.");
        let titles: Vec<&str> = parts.iter().map(|p| p.title).collect();
        assert_eq!(titles, ["Text that moves", "Layers without taking them apart", "Cinematography"]);
        assert!(parts[1].body.contains("**Not a part**"), "a bold line inside a fence is not a title");
    }

    #[test]
    fn a_part_is_found_by_title_prefix_or_word() {
        let (_, parts) = split(SAMPLE);
        assert_eq!(find(&parts, "cinema")[0].title, "Cinematography");
        assert_eq!(find(&parts, "LAYERS")[0].title, "Layers without taking them apart");
        assert_eq!(find(&parts, "apart")[0].title, "Layers without taking them apart");
        assert!(find(&parts, "zzz").is_empty());
    }

    #[test]
    fn outline_names_every_part_and_the_ops_of_a_table() {
        let long = format!("{SAMPLE}\n\n{}", "x".repeat(BIG));
        let out = serve(&long, "motion", None, false).unwrap();
        assert!(out.contains("Text that moves") && out.contains("Cinematography") && out.contains("shot camera lens"), "{out}");
        assert!(out.len() < 1_500, "the outline is short, not the section");
        assert!(serve(&long, "motion", Some("text, cinema"), false).unwrap().contains("| `shot` | a |"));
        assert_eq!(serve(&long, "motion", None, true).unwrap(), long);
        assert!(serve(&long, "motion", Some("nothing"), false).unwrap_err().contains("parts:"));
        assert_eq!(serve("short", "x", None, false).unwrap(), "short");
        let rows = serve(&long, "motion", Some("lens"), false).unwrap();
        assert!(rows.contains("| op | fields |") && rows.contains("| `camera` / `lens` | b |") && !rows.contains("`shot`"), "{rows}");
    }
}
