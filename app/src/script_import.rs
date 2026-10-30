// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Script files to text: plain text, Markdown, Fountain, SRT and WebVTT are
//! read as they are; a Word document (.docx, a zip of XML) is turned into
//! the Markdown-like text the story reader understands (headings stay
//! headings, a screenplay's character and dialogue paragraphs stay
//! together). The story model does the rest (`studio_model::story::import`).

use std::io::Read;
use std::path::Path;

/// The extensions of the files the Story panel takes.
pub fn is_script_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| studio_model::story::import::Format::EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// The text of a script file.
pub fn read_file(p: &Path) -> Result<String, String> {
    let bytes = std::fs::read(p).map_err(|e| format!("could not read {}: {e}", p.display()))?;
    if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("docx")) {
        return docx_text(&bytes);
    }
    Ok(decode(&bytes))
}

/// UTF-8, or UTF-16 with a byte order mark (some subtitle tools write it).
fn decode(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The text of a .docx: paragraphs in order, headings as `# `, Title as
/// `# `, a character cue and its dialogue (by their Word styles) kept
/// together, other paragraphs separated by a blank line.
pub fn docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a Word document: {e}"))?;
    let mut xml = String::new();
    let entry = zip.by_name("word/document.xml").map_err(|_| "not a Word document: no word/document.xml".to_owned())?;
    // A link is never followed: a document whose text is a link entry is refused.
    if entry.is_symlink() {
        return Err("not a Word document: word/document.xml is a link".to_owned());
    }
    entry.take(64 << 20).read_to_string(&mut xml).map_err(|e| format!("could not read the document: {e}"))?;
    let doc = roxmltree::Document::parse_with_options(&xml, roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() }).map_err(|e| format!("could not read the document: {e}"))?;
    let is = |n: &roxmltree::Node, name: &str| n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(W);
    let style_of = |p: &roxmltree::Node| -> String {
        p.children()
            .find(|n| is(n, "pPr"))
            .and_then(|pr| pr.children().find(|n| is(n, "pStyle")))
            .and_then(|s| s.attribute((W, "val")))
            .unwrap_or("")
            .to_lowercase()
            .replace([' ', '-', '_'], "")
    };
    // With a Title, headings are the scenes under it.
    let has_title = doc.descendants().filter(|n| is(n, "p")).any(|p| style_of(&p) == "title");
    let mut out = String::new();
    // After a cue or parenthetical the next paragraph follows on the next line.
    let mut glue = false;
    for p in doc.descendants().filter(|n| is(n, "p")) {
        let style = style_of(&p);
        let mut text = String::new();
        for n in p.descendants() {
            if is(&n, "t") {
                text.push_str(n.text().unwrap_or(""));
            } else if is(&n, "tab") || is(&n, "br") {
                text.push(' ');
            }
        }
        let text = text.trim();
        if text.is_empty() {
            glue = false;
            continue;
        }
        let level = match style.as_str() {
            "title" => Some(1),
            "heading1" => Some(if has_title { 2 } else { 1 }),
            x if x.starts_with("heading") => Some(2),
            _ => None,
        };
        let cue = style.contains("character") || style.contains("parenthetical");
        let line = match level {
            Some(n) => format!("{} {text}", "#".repeat(n)),
            None => text.to_owned(),
        };
        if out.is_empty() {
        } else if glue {
            out.push('\n');
        } else {
            out.push_str("\n\n");
        }
        out.push_str(&line);
        glue = cue;
    }
    if out.is_empty() {
        return Err("that document has no text".into());
    }
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn docx(body: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            write!(z, "<?xml version=\"1.0\"?><w:document xmlns:w=\"{W}\"><w:body>{body}</w:body></w:document>").unwrap();
            z.finish().unwrap();
        }
        buf
    }

    fn p(style: &str, text: &str) -> String {
        let st = if style.is_empty() { String::new() } else { format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>") };
        format!("<w:p>{st}<w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    #[test]
    fn a_word_document_becomes_script_text() {
        let body = [p("Title", "The Fox"), p("Heading1", "The morning"), p("", "[0:00-0:05] The fox woke early."), p("Character", "FERN"), p("Dialogue", "Hello?"), p("", "The end &amp; more.")].concat();
        let text = docx_text(&docx(&body)).unwrap();
        assert_eq!(text, "# The Fox\n\n## The morning\n\n[0:00-0:05] The fox woke early.\n\nFERN\nHello?\n\nThe end & more.\n");
        let st = studio_model::story::import::read_script(&text, Some("md"));
        assert_eq!(st.title, "The Fox");
        assert_eq!(st.scenes.len(), 1);
        assert_eq!(st.scenes[0].lines[0].start, Some(0));
        assert!(docx_text(b"not a zip").is_err());
    }

    /// A document with `entries` added before the real text.
    fn docx_with(entries: impl FnOnce(&mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>)) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            entries(&mut z);
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            write!(z, "<?xml version=\"1.0\"?><w:document xmlns:w=\"{W}\"><w:body>{}</w:body></w:document>", p("", "Safe text")).unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn a_path_that_escapes_is_never_written() {
        let dir = std::env::temp_dir().join(format!("zornov-docx-escape-{}", std::process::id()));
        let inner = dir.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let bytes = docx_with(|z| {
            z.start_file("../escaped.txt", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"escaped").unwrap();
        });
        std::fs::write(inner.join("a.docx"), &bytes).unwrap();
        // Only the one named entry is read; nothing is extracted anywhere.
        assert_eq!(read_file(&inner.join("a.docx")).unwrap(), "Safe text\n");
        assert!(!dir.join("escaped.txt").exists() && !inner.join("escaped.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_in_place_of_the_document_is_refused() {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.add_symlink("word/document.xml", "/etc/passwd", zip::write::SimpleFileOptions::default()).unwrap();
            z.finish().unwrap();
        }
        let err = docx_text(&buf).unwrap_err();
        assert!(err.contains("is a link"), "{err}");
    }

    #[test]
    fn utf16_subtitles_are_read() {
        let mut b = vec![0xFF, 0xFE];
        for u in "1\n00:00:01,000 --> 00:00:02,000\nHi\n".encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        assert!(decode(&b).contains("-->"));
        assert!(is_script_file(Path::new("a/b.SRT")) && is_script_file(Path::new("x.docx")) && !is_script_file(Path::new("x.mp4")));
    }
}
