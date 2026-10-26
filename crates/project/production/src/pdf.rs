// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small PDF writer: pages of text, lines and filled boxes in the three
//! built-in fonts, and a flowing layout (headings, paragraphs, tables that
//! break across pages) on top. Enough for call sheets, breakdown sheets,
//! stripboards and budgets.
//!
//! The built-in fonts hold Latin text only: letters outside it print as
//! `?`. Names written in other scripts need a romanised form on the sheet.

use std::fmt::Write as _;

/// A built-in font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// Helvetica.
    Sans,
    /// Helvetica Bold.
    Bold,
    /// Courier.
    Mono,
}

impl Font {
    fn res(self) -> &'static str {
        match self {
            Font::Sans => "F1",
            Font::Bold => "F2",
            Font::Mono => "F3",
        }
    }
}

// Helvetica advance widths for ASCII 32..=126, in 1/1000 em.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667,
    556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722,
    500, 500, 500, 334, 260, 334, 584,
];

/// The width of text in points.
pub fn text_width(text: &str, size: f32, font: Font) -> f32 {
    let em: f32 = text
        .chars()
        .map(|c| match font {
            Font::Mono => 600.0,
            _ => {
                let w = if (' '..='~').contains(&c) { HELV[c as usize - 32] as f32 } else { 556.0 };
                if font == Font::Bold { w * 1.05 } else { w }
            }
        })
        .sum();
    em * size / 1000.0
}

fn winansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{a0}'..='\u{ff}' => c as u32 as u8,
        '€' => 0x80,
        '–' => 0x96,
        '—' => 0x97,
        '‘' | '’' => 0x27,
        '“' | '”' => 0x22,
        '•' => 0x95,
        '…' => 0x85,
        '→' => b'>',
        '\t' | '\n' => b' ',
        _ => b'?',
    }
}

/// One page's drawing.
#[derive(Clone, Debug, Default)]
pub struct Page {
    ops: String,
}

impl Page {
    /// Draws text with its baseline at (x, y) (points from the bottom left).
    pub fn text(&mut self, x: f32, y: f32, size: f32, font: Font, rgb: [f32; 3], text: &str) {
        let mut hex = String::new();
        for c in text.chars() {
            let _ = write!(hex, "{:02X}", winansi(c));
        }
        let _ = writeln!(self.ops, "BT /{} {size:.2} Tf {:.3} {:.3} {:.3} rg {x:.2} {y:.2} Td <{hex}> Tj ET", font.res(), rgb[0], rgb[1], rgb[2]);
    }

    /// A filled box.
    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [f32; 3]) {
        let _ = writeln!(self.ops, "{:.3} {:.3} {:.3} rg {x:.2} {y:.2} {w:.2} {h:.2} re f", rgb[0], rgb[1], rgb[2]);
    }

    /// A line.
    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, rgb: [f32; 3]) {
        let _ = writeln!(self.ops, "{:.3} {:.3} {:.3} RG {width:.2} w {x1:.2} {y1:.2} m {x2:.2} {y2:.2} l S", rgb[0], rgb[1], rgb[2]);
    }
}

/// A PDF document.
#[derive(Clone, Debug)]
pub struct Pdf {
    /// Page width in points.
    pub width: f32,
    /// Page height in points.
    pub height: f32,
    /// The pages.
    pub pages: Vec<Page>,
    /// Document title.
    pub title: String,
}

impl Pdf {
    /// A4 (portrait or landscape).
    pub fn a4(landscape: bool, title: &str) -> Pdf {
        let (w, h) = if landscape { (841.89, 595.28) } else { (595.28, 841.89) };
        Pdf { width: w, height: h, pages: Vec::new(), title: title.to_owned() }
    }

    /// Starts a page and returns it.
    pub fn page(&mut self) -> &mut Page {
        self.pages.push(Page::default());
        self.pages.last_mut().unwrap()
    }

    /// The finished file.
    pub fn finish(&self) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets: Vec<usize> = Vec::new();
        let n = self.pages.len().max(1);
        // 1 catalog, 2 pages, 3-5 fonts, 6 info, then page/content pairs.
        let mut obj = |out: &mut Vec<u8>, body: &str| {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", offsets.len()).as_bytes());
        };
        let kids: String = (0..n).map(|i| format!("{} 0 R", 7 + i * 2)).collect::<Vec<_>>().join(" ");
        obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>");
        obj(&mut out, &format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>"));
        for f in ["Helvetica", "Helvetica-Bold", "Courier"] {
            obj(&mut out, &format!("<< /Type /Font /Subtype /Type1 /BaseFont /{f} /Encoding /WinAnsiEncoding >>"));
        }
        let title: String = self.title.chars().map(|c| winansi(c) as char).filter(|c| !matches!(c, '(' | ')' | '\\')).collect();
        obj(&mut out, &format!("<< /Title ({title}) /Producer (Studio production office) >>"));
        let blank = Page::default();
        for i in 0..n {
            let page = self.pages.get(i).unwrap_or(&blank);
            let pid = 7 + i * 2;
            obj(
                &mut out,
                &format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> /Contents {} 0 R >>",
                    self.width,
                    self.height,
                    pid + 1
                ),
            );
            obj(&mut out, &format!("<< /Length {} >>\nstream\n{}endstream", page.ops.len(), page.ops));
        }
        let xref = out.len();
        let mut x = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
        for o in &offsets {
            let _ = writeln!(x, "{o:010} 00000 n ");
        }
        let _ = write!(x, "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1);
        out.extend_from_slice(x.as_bytes());
        out
    }
}

const INK: [f32; 3] = [0.1, 0.1, 0.12];
const GREY: [f32; 3] = [0.45, 0.45, 0.5];
const RULE: [f32; 3] = [0.8, 0.8, 0.84];

/// A table column.
#[derive(Clone, Debug)]
pub struct Col {
    /// Heading.
    pub title: String,
    /// Share of the width (relative).
    pub weight: f32,
    /// Right-aligned (numbers).
    pub right: bool,
}

impl Col {
    /// A left-aligned column.
    pub fn new(title: &str, weight: f32) -> Col {
        Col { title: title.into(), weight, right: false }
    }
    /// A right-aligned column.
    pub fn num(title: &str, weight: f32) -> Col {
        Col { title: title.into(), weight, right: true }
    }
}

/// A table row.
#[derive(Clone, Debug, Default)]
pub struct Row {
    /// The cells.
    pub cells: Vec<String>,
    /// Background.
    pub fill: Option<[f32; 3]>,
    /// Bold text.
    pub bold: bool,
}

impl Row {
    /// A plain row.
    pub fn new(cells: Vec<String>) -> Row {
        Row { cells, fill: None, bold: false }
    }
}

/// Breaks text into lines that fit `width` points.
pub fn wrap(text: &str, width: f32, size: f32, font: Font) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let mut word = word.to_owned();
            // A word wider than the column is cut.
            while text_width(&word, size, font) > width && word.chars().count() > 1 {
                let mut cut = String::new();
                for c in word.chars() {
                    if text_width(&format!("{cut}{c}"), size, font) > width && !cut.is_empty() {
                        break;
                    }
                    cut.push(c);
                }
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                }
                lines.push(cut.clone());
                word = word.chars().skip(cut.chars().count()).collect();
            }
            let t = if cur.is_empty() { word.clone() } else { format!("{cur} {word}") };
            if text_width(&t, size, font) <= width || cur.is_empty() {
                cur = t;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word;
            }
        }
        lines.push(cur);
    }
    lines
}

/// A flowing layout over a [`Pdf`]: content goes down the page and onto
/// the next when it runs out.
pub struct Flow {
    /// The document being written.
    pub pdf: Pdf,
    margin: f32,
    y: f32,
    footer: String,
}

impl Flow {
    /// A new document, with `footer` printed on each page.
    pub fn new(title: &str, landscape: bool, footer: &str) -> Flow {
        let mut pdf = Pdf::a4(landscape, title);
        pdf.page();
        let margin = 36.0;
        let y = pdf.height - margin;
        Flow { pdf, margin, y, footer: footer.to_owned() }
    }

    fn inner_w(&self) -> f32 {
        self.pdf.width - 2.0 * self.margin
    }

    fn page(&mut self) -> &mut Page {
        self.pdf.pages.last_mut().unwrap()
    }

    fn need(&mut self, h: f32) {
        if self.y - h < self.margin + 14.0 {
            self.pdf.page();
            self.y = self.pdf.height - self.margin;
        }
    }

    /// Starts a new page.
    pub fn new_page(&mut self) {
        self.pdf.page();
        self.y = self.pdf.height - self.margin;
    }

    /// Vertical space.
    pub fn space(&mut self, pts: f32) {
        self.y -= pts;
    }

    /// A large title.
    pub fn title(&mut self, text: &str) {
        self.need(30.0);
        let (m, y) = (self.margin, self.y - 20.0);
        self.page().text(m, y, 20.0, Font::Bold, INK, text);
        self.y -= 28.0;
    }

    /// A section heading with a rule under it.
    pub fn heading(&mut self, text: &str) {
        self.need(30.0);
        self.y -= 8.0;
        let (m, w, y) = (self.margin, self.inner_w(), self.y - 12.0);
        self.page().text(m, y, 12.0, Font::Bold, INK, text);
        self.page().line(m, y - 4.0, m + w, y - 4.0, 0.6, RULE);
        self.y -= 22.0;
    }

    /// A paragraph of text.
    pub fn para(&mut self, text: &str, size: f32) {
        let lines = wrap(text, self.inner_w(), size, Font::Sans);
        for l in lines {
            self.need(size + 3.0);
            let (m, y) = (self.margin, self.y - size);
            self.page().text(m, y, size, Font::Sans, INK, &l);
            self.y -= size + 3.0;
        }
    }

    /// Small grey text.
    pub fn note(&mut self, text: &str) {
        let lines = wrap(text, self.inner_w(), 8.5, Font::Sans);
        for l in lines {
            self.need(12.0);
            let (m, y) = (self.margin, self.y - 8.5);
            self.page().text(m, y, 8.5, Font::Sans, GREY, &l);
            self.y -= 11.5;
        }
    }

    /// `label: value` pairs in a row of columns.
    pub fn facts(&mut self, pairs: &[(&str, String)], per_row: usize) {
        let colw = self.inner_w() / per_row as f32;
        for chunk in pairs.chunks(per_row) {
            self.need(26.0);
            for (i, (k, v)) in chunk.iter().enumerate() {
                let x = self.margin + colw * i as f32;
                let y = self.y;
                self.page().text(x, y - 8.0, 7.5, Font::Sans, GREY, &k.to_uppercase());
                let v = wrap(v, colw - 8.0, 10.5, Font::Bold).into_iter().next().unwrap_or_default();
                self.page().text(x, y - 21.0, 10.5, Font::Bold, INK, &v);
            }
            self.y -= 28.0;
        }
    }

    /// A table whose header repeats on each page.
    pub fn table(&mut self, cols: &[Col], rows: &[Row], size: f32) {
        let total: f32 = cols.iter().map(|c| c.weight).sum();
        let widths: Vec<f32> = cols.iter().map(|c| self.inner_w() * c.weight / total).collect();
        let pad = 3.0;
        let lead = size + 2.5;
        let header = |f: &mut Flow| {
            f.need(lead + 10.0);
            let mut x = f.margin;
            let (y, m, w) = (f.y, f.margin, f.inner_w());
            f.page().fill(m, y - lead - 4.0, w, lead + 4.0, [0.93, 0.93, 0.95]);
            for (c, cw) in cols.iter().zip(&widths) {
                let tx = if c.right { x + cw - pad - text_width(&c.title, size, Font::Bold) } else { x + pad };
                f.page().text(tx, y - size - 3.0, size, Font::Bold, INK, &c.title);
                x += cw;
            }
            f.y -= lead + 5.0;
        };
        header(self);
        for row in rows {
            let font = if row.bold { Font::Bold } else { Font::Sans };
            let wrapped: Vec<Vec<String>> = row.cells.iter().zip(&widths).map(|(c, w)| wrap(c, w - 2.0 * pad, size, font)).collect();
            let n = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
            let h = n as f32 * lead + 4.0;
            if self.y - h < self.margin + 14.0 {
                self.pdf.page();
                self.y = self.pdf.height - self.margin;
                header(self);
            }
            let (m, w, y) = (self.margin, self.inner_w(), self.y);
            if let Some(f) = row.fill {
                self.page().fill(m, y - h, w, h, f);
            }
            let mut x = self.margin;
            for ((cell, c), cw) in wrapped.iter().zip(cols).zip(&widths) {
                for (li, line) in cell.iter().enumerate() {
                    let tx = if c.right { x + cw - pad - text_width(line, size, font) } else { x + pad };
                    self.page().text(tx, y - size - 2.0 - li as f32 * lead, size, font, INK, line);
                }
                x += cw;
            }
            self.page().line(m, y - h, m + w, y - h, 0.3, RULE);
            self.y -= h;
        }
        self.y -= 6.0;
    }

    /// Adds the page footers and returns the file.
    pub fn finish(mut self) -> Vec<u8> {
        let n = self.pdf.pages.len();
        let (w, m, footer) = (self.pdf.width, self.margin, self.footer.clone());
        for (i, p) in self.pdf.pages.iter_mut().enumerate() {
            p.text(m, 20.0, 7.5, Font::Sans, GREY, &footer);
            let t = format!("{} / {n}", i + 1);
            p.text(w - m - text_width(&t, 7.5, Font::Sans), 20.0, 7.5, Font::Sans, GREY, &t);
        }
        self.pdf.finish()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Checks the cross-reference table points at every object.
    pub(crate) fn check_structure(bytes: &[u8]) -> usize {
        // Non-ASCII bytes become ?, so string offsets are file offsets.
        let s: String = bytes.iter().map(|&b| if b < 128 { b as char } else { '?' }).collect();
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.trim_end().ends_with("%%EOF"));
        let sx = s.rfind("startxref").unwrap();
        let xref: usize = s[sx + 9..].split_whitespace().next().unwrap().parse().unwrap();
        assert!(s[xref..].starts_with("xref"), "startxref points at the table");
        let lines: Vec<&str> = s[xref..].lines().collect();
        let count: usize = lines[1].split_whitespace().nth(1).unwrap().parse().unwrap();
        for (i, l) in lines[3..2 + count].iter().enumerate() {
            let off: usize = l[..10].parse().unwrap();
            assert!(s[off..].starts_with(&format!("{} 0 obj", i + 1)), "object {} at {off}", i + 1);
        }
        s.matches("/Type /Page ").count()
    }

    #[test]
    fn a_document_has_a_valid_cross_reference_table() {
        let mut pdf = Pdf::a4(false, "Test (1)");
        let p = pdf.page();
        p.text(50.0, 700.0, 12.0, Font::Sans, [0.0; 3], "Hello (world) \\ café €");
        p.fill(50.0, 600.0, 100.0, 20.0, [1.0, 0.0, 0.0]);
        p.line(50.0, 590.0, 200.0, 590.0, 1.0, [0.0; 3]);
        pdf.page();
        let b = pdf.finish();
        assert_eq!(check_structure(&b), 2);
        assert!(String::from_utf8_lossy(&b).contains("<48656C6C6F20"), "text is hex-encoded so brackets cannot break it");
    }

    #[test]
    fn widths_and_wrapping() {
        assert!((text_width("Hello", 10.0, Font::Sans) - 22.78).abs() < 0.01, "{}", text_width("Hello", 10.0, Font::Sans));
        assert_eq!(text_width("abcd", 10.0, Font::Mono), 24.0);
        let l = wrap("the quick brown fox jumps over the lazy dog", 80.0, 10.0, Font::Sans);
        assert!(l.len() >= 3);
        assert!(l.iter().all(|x| text_width(x, 10.0, Font::Sans) <= 80.5), "{l:?}");
        let l = wrap("supercalifragilisticexpialidocious", 40.0, 10.0, Font::Sans);
        assert!(l.len() > 1 && l.iter().all(|x| text_width(x, 10.0, Font::Sans) <= 40.5), "{l:?}");
        assert_eq!(wrap("a\n\nb", 100.0, 10.0, Font::Sans), vec!["a", "", "b"]);
    }

    #[test]
    fn tables_break_across_pages() {
        let mut f = Flow::new("T", true, "footer");
        f.title("Big table");
        let rows: Vec<Row> = (0..120).map(|i| Row::new(vec![i.to_string(), format!("row number {i} with some words to fill it")])).collect();
        f.table(&[Col::num("#", 1.0), Col::new("Text", 6.0)], &rows, 9.0);
        let b = f.finish();
        assert!(check_structure(&b) >= 3);
    }
}
