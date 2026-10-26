// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small Fluent-style message format.
//!
//! ```text
//! # comment
//! export = Export
//! clips = { $n ->
//!     [one] { $n } clip
//!    *[other] { $n } clips
//!  }
//! welcome = Hello, { $name }. { open-project }
//! brace = Use { "{" } and { "}" } for placeables.
//! ```
//!
//! Supported: text, `{ $var }`, `{ message-id }`, `{ "literal" }`, selectors
//! on a variable with `[category]`, `[exact-number]` or `[string]` variants
//! and one `*` default, multi-line values (indented continuation lines).
//! Not supported: terms, attributes, functions (numbers are formatted by
//! the caller's locale).

use crate::Lang;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Num(i64),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_owned())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Num(n)
    }
}
impl From<i32> for Value {
    fn from(n: i32) -> Self {
        Value::Num(n as i64)
    }
}
impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::Num(n as i64)
    }
}
impl From<u32> for Value {
    fn from(n: u32) -> Self {
        Value::Num(n as i64)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    Var(String),
    Msg(String),
    Select { var: String, variants: Vec<(String, Pattern)>, default: usize },
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Pattern(Vec<Part>);

impl Pattern {
    /// The text, when the pattern has no placeables at all.
    pub fn plain(&self) -> Option<&str> {
        match self.0.as_slice() {
            [] => Some(""),
            [Part::Text(t)] => Some(t),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Parse a resource into `(id, pattern)` pairs, in file order.
pub fn parse(src: &str) -> Result<Vec<(String, Pattern)>, ParseError> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        if indented(line) {
            return Err(ParseError { line: i + 1, message: "unexpected indented line".into() });
        }
        let Some((id, rest)) = trimmed.split_once('=') else {
            return Err(ParseError { line: i + 1, message: format!("expected `id = value`, got `{trimmed}`") });
        };
        let id = id.trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(ParseError { line: i + 1, message: format!("bad message id `{id}`") });
        }
        let start = i + 1;
        // The value: text after `=`, plus every following indented line.
        let mut raw = String::new();
        let first = rest.trim_start();
        raw.push_str(first);
        i += 1;
        let mut block_indent: Option<usize> = None;
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() {
                // A blank line inside a value belongs to it only when more
                // indented text follows.
                let mut j = i;
                while j < lines.len() && lines[j].trim().is_empty() {
                    j += 1;
                }
                if j < lines.len() && indented(lines[j]) {
                    raw.push('\n');
                    i += 1;
                    continue;
                }
                break;
            }
            if !indented(l) {
                break;
            }
            // Indentation is ASCII spaces and tabs, so byte offsets are char boundaries.
            let indent = l.len() - l.trim_start_matches([' ', '\t']).len();
            let bi = *block_indent.get_or_insert(indent);
            let body = if indent >= bi { &l[bi..] } else { l.trim_start_matches([' ', '\t']) };
            if !raw.is_empty() && !raw.ends_with('\n') {
                raw.push('\n');
            }
            raw.push_str(body.trim_end());
            i += 1;
        }
        let pat = parse_pattern(raw.trim_end(), start)?;
        out.push((id.to_owned(), pat));
    }
    Ok(out)
}

/// Does the line start with indentation (ASCII space or tab)?
fn indented(l: &str) -> bool {
    l.starts_with([' ', '\t'])
}

fn parse_pattern(src: &str, line: usize) -> Result<Pattern, ParseError> {
    let chars: Vec<char> = src.chars().collect();
    let mut pos = 0;
    let parts = parse_parts(&chars, &mut pos, false, line)?;
    Ok(Pattern(parts))
}

fn err<T>(line: usize, m: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError { line, message: m.into() })
}

/// Parse text and placeables until the end, or (inside a variant) until a
/// line starting a new variant or the closing brace of the selector.
fn parse_parts(c: &[char], pos: &mut usize, in_variant: bool, line: usize) -> Result<Vec<Part>, ParseError> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, parts: &mut Vec<Part>| {
        if !text.is_empty() {
            parts.push(Part::Text(std::mem::take(text)));
        }
    };
    while *pos < c.len() {
        let ch = c[*pos];
        if in_variant && ch == '\n' {
            // Does a new variant (`[`, `*[`) or the closing `}` follow?
            let mut k = *pos + 1;
            while k < c.len() && (c[k] == ' ' || c[k] == '\t') {
                k += 1;
            }
            if k < c.len() && (c[k] == '[' || c[k] == '}' || (c[k] == '*' && c.get(k + 1) == Some(&'['))) {
                break;
            }
            text.push('\n');
            *pos += 1;
            continue;
        }
        if in_variant && ch == '}' {
            break;
        }
        if ch == '{' {
            *pos += 1;
            match parse_placeable(c, pos, line)? {
                // A quoted literal is just text (it lets a line start with `[`).
                Part::Text(s) => text.push_str(&s),
                part => {
                    flush(&mut text, &mut parts);
                    parts.push(part);
                }
            }
            continue;
        }
        text.push(ch);
        *pos += 1;
    }
    flush(&mut text, &mut parts);
    Ok(parts)
}

fn skip_ws(c: &[char], pos: &mut usize) {
    while *pos < c.len() && c[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn take_ident(c: &[char], pos: &mut usize) -> String {
    let s = *pos;
    while *pos < c.len() && (c[*pos].is_ascii_alphanumeric() || c[*pos] == '-' || c[*pos] == '_') {
        *pos += 1;
    }
    c[s..*pos].iter().collect()
}

fn parse_placeable(c: &[char], pos: &mut usize, line: usize) -> Result<Part, ParseError> {
    skip_ws(c, pos);
    let head = if *pos < c.len() && c[*pos] == '"' {
        *pos += 1;
        let mut s = String::new();
        while *pos < c.len() && c[*pos] != '"' {
            s.push(c[*pos]);
            *pos += 1;
        }
        if *pos >= c.len() {
            return err(line, "unterminated string literal");
        }
        *pos += 1;
        Part::Text(s)
    } else if *pos < c.len() && c[*pos] == '$' {
        *pos += 1;
        let v = take_ident(c, pos);
        if v.is_empty() {
            return err(line, "empty variable name");
        }
        Part::Var(v)
    } else {
        let id = take_ident(c, pos);
        if id.is_empty() {
            return err(line, "empty placeable");
        }
        Part::Msg(id)
    };
    skip_ws(c, pos);
    if *pos < c.len() && c[*pos] == '}' {
        *pos += 1;
        return Ok(head);
    }
    // A selector: `->` then variants.
    if !(*pos + 1 < c.len() && c[*pos] == '-' && c[*pos + 1] == '>') {
        return err(line, "expected `}` or `->`");
    }
    let Part::Var(var) = head else {
        return err(line, "only a $variable can be selected on");
    };
    *pos += 2;
    let mut variants = Vec::new();
    let mut default = None;
    loop {
        skip_ws(c, pos);
        if *pos >= c.len() {
            return err(line, "selector is missing `}`");
        }
        if c[*pos] == '}' {
            *pos += 1;
            break;
        }
        let star = c[*pos] == '*';
        if star {
            *pos += 1;
        }
        if *pos >= c.len() || c[*pos] != '[' {
            return err(line, "expected `[key]`");
        }
        *pos += 1;
        let mut key = String::new();
        while *pos < c.len() && c[*pos] != ']' {
            key.push(c[*pos]);
            *pos += 1;
        }
        if *pos >= c.len() {
            return err(line, "unterminated `[key`");
        }
        *pos += 1;
        // The variant's text runs to the end of the line (and onward until
        // the next variant).
        while *pos < c.len() && (c[*pos] == ' ' || c[*pos] == '\t') {
            *pos += 1;
        }
        let parts = parse_parts(c, pos, true, line)?;
        if star {
            default = Some(variants.len());
        }
        variants.push((key.trim().to_owned(), Pattern(parts)));
    }
    let Some(default) = default else {
        return err(line, "selector has no `*` default variant");
    };
    Ok(Part::Select { var, variants, default })
}

/// Everything a pattern needs to be formatted.
pub struct Env<'a> {
    pub lang: Lang,
    pub args: &'a [(&'a str, Value)],
    pub messages: &'a HashMap<&'static str, Pattern>,
}

impl Env<'_> {
    fn arg(&self, name: &str) -> Option<&Value> {
        self.args.iter().find(|(k, _)| *k == name).map(|(_, v)| v)
    }
}

/// Digits for the language: Persian text uses Persian digits.
pub fn localise_digits(lang: Lang, s: &str) -> String {
    let base = match lang {
        Lang::Fa => '۰',
        Lang::Ar => '٠',
        _ => return s.to_owned(),
    };
    s.chars().map(|c| if c.is_ascii_digit() { char::from_u32(base as u32 + (c as u32 - '0' as u32)).unwrap_or(c) } else { c }).collect()
}

pub fn format(p: &Pattern, env: &Env, depth: u8, out: &mut String) {
    if depth > 8 {
        out.push_str("{…}");
        return;
    }
    for part in &p.0 {
        match part {
            Part::Text(t) => out.push_str(t),
            Part::Var(v) => match env.arg(v) {
                Some(Value::Str(s)) => out.push_str(s),
                Some(Value::Num(n)) => out.push_str(&localise_digits(env.lang, &n.to_string())),
                None => {
                    out.push_str("{$");
                    out.push_str(v);
                    out.push('}');
                }
            },
            Part::Msg(id) => match env.messages.get(id.as_str()) {
                Some(m) => format(m, env, depth + 1, out),
                None => {
                    out.push_str("{");
                    out.push_str(id);
                    out.push('}');
                }
            },
            Part::Select { var, variants, default } => {
                let chosen = match env.arg(var) {
                    Some(Value::Num(n)) => {
                        let exact = n.to_string();
                        let cat = crate::plural::category(env.lang, *n);
                        variants.iter().position(|(k, _)| *k == exact).or_else(|| variants.iter().position(|(k, _)| k == cat))
                    }
                    Some(Value::Str(s)) => variants.iter().position(|(k, _)| k == s),
                    None => None,
                };
                format(&variants[chosen.unwrap_or(*default)].1, env, depth + 1, out);
            }
        }
    }
}

/// Every `$variable` a pattern uses (for the catalogue consistency tests).
/// The literal pieces and variable names of a pattern made only of text and
/// plain variables (`Undid { $v1 }`): `literals.len() == vars.len() + 1`.
pub fn template(p: &Pattern) -> Option<(Vec<String>, Vec<String>)> {
    let mut lits = vec![String::new()];
    let mut vars = Vec::new();
    for part in &p.0 {
        match part {
            Part::Text(t) => lits.last_mut().unwrap().push_str(t),
            Part::Var(v) => {
                vars.push(v.clone());
                lits.push(String::new());
            }
            _ => return None,
        }
    }
    (!vars.is_empty()).then_some((lits, vars))
}

/// Every `$variable` a pattern uses (for the catalogue consistency tests).
pub fn variables(p: &Pattern) -> Vec<String> {
    let mut v = Vec::new();
    fn walk(p: &Pattern, v: &mut Vec<String>) {
        for part in &p.0 {
            match part {
                Part::Var(n) => v.push(n.clone()),
                Part::Select { var, variants, .. } => {
                    v.push(var.clone());
                    for (_, vp) in variants {
                        walk(vp, v);
                    }
                }
                _ => {}
            }
        }
    }
    walk(p, &mut v);
    v.sort();
    v.dedup();
    v
}

/// Every message id a pattern refers to.
pub fn references(p: &Pattern) -> Vec<String> {
    let mut v = Vec::new();
    fn walk(p: &Pattern, v: &mut Vec<String>) {
        for part in &p.0 {
            match part {
                Part::Msg(n) => v.push(n.clone()),
                Part::Select { variants, .. } => variants.iter().for_each(|(_, vp)| walk(vp, v)),
                _ => {}
            }
        }
    }
    walk(p, &mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, id: &str, lang: Lang, args: &[(&str, Value)]) -> String {
        let parsed = parse(src).unwrap();
        let map: HashMap<&'static str, Pattern> = parsed.into_iter().map(|(k, v)| (&*Box::leak(k.into_boxed_str()), v)).collect();
        let mut out = String::new();
        format(&map[id], &Env { lang, args, messages: &map }, 0, &mut out);
        out
    }

    #[test]
    fn plain_and_variable() {
        let src = "a = Hello\nb = Hi { $name }!\n";
        assert_eq!(run(src, "a", Lang::En, &[]), "Hello");
        assert_eq!(run(src, "b", Lang::En, &[("name", "Sam".into())]), "Hi Sam!");
    }

    #[test]
    fn plural_selector() {
        let src = "clips = { $n ->\n    [one] { $n } clip\n   *[other] { $n } clips\n }\n";
        assert_eq!(run(src, "clips", Lang::En, &[("n", 1.into())]), "1 clip");
        assert_eq!(run(src, "clips", Lang::En, &[("n", 5.into())]), "5 clips");
        assert_eq!(run(src, "clips", Lang::En, &[("n", 0.into())]), "0 clips");
    }

    #[test]
    fn exact_number_beats_category() {
        let src = "m = { $n ->\n    [0] none\n    [one] one\n   *[other] many\n }\n";
        assert_eq!(run(src, "m", Lang::En, &[("n", 0.into())]), "none");
        assert_eq!(run(src, "m", Lang::En, &[("n", 1.into())]), "one");
    }

    #[test]
    fn references_and_literals() {
        let src = "name = Zornov\nb = { name } uses { \"{\" }braces{ \"}\" }\n";
        assert_eq!(run(src, "b", Lang::En, &[]), "Zornov uses {braces}");
    }

    #[test]
    fn multiline_value() {
        let src = "m =\n    first line\n    second line\nn = x\n";
        assert_eq!(run(src, "m", Lang::En, &[]), "first line\nsecond line");
        assert_eq!(run(src, "n", Lang::En, &[]), "x");
    }

    #[test]
    fn persian_digits() {
        let src = "m = { $n } clip\n";
        assert_eq!(run(src, "m", Lang::Fa, &[("n", 12.into())]), "۱۲ clip");
    }

    #[test]
    fn missing_arg_is_visible_not_a_panic() {
        assert_eq!(run("m = { $n } x\n", "m", Lang::En, &[]), "{$n} x");
    }

    #[test]
    fn errors_name_the_line() {
        assert!(parse("ok = fine\nbroken line\n").unwrap_err().line == 2);
        assert!(parse("m = { $n ->\n [one] x\n }\n").is_err(), "no default variant");
    }

    #[test]
    fn variables_are_listed() {
        let p = parse("m = { $n ->\n [one] { $n } of { $total }\n *[other] x\n }\n").unwrap();
        assert_eq!(variables(&p[0].1), ["n", "total"]);
    }
}

/// The message source for a pattern, on one line (selectors inline).
pub fn raw(p: &Pattern) -> String {
    let mut s = String::new();
    for part in &p.0 {
        match part {
            Part::Text(t) => s.push_str(&t.replace('\n', "\\n")),
            Part::Var(v) => s.push_str(&format!("{{ ${v} }}")),
            Part::Msg(m) => s.push_str(&format!("{{ {m} }}")),
            Part::Select { var, variants, default } => {
                s.push_str(&format!("{{ ${var} ->"));
                for (i, (k, vp)) in variants.iter().enumerate() {
                    s.push_str(&format!(" {}[{k}] {}", if i == *default { "*" } else { "" }, raw(vp)));
                }
                s.push_str(" }");
            }
        }
    }
    s
}

#[cfg(test)]
mod brace_tests {
    use super::*;

    #[test]
    fn selector_may_close_on_the_variant_line() {
        let src = "m = a{ $x ->\n    [yes] , b\n   *[no] }\n";
        let p = parse(src).unwrap();
        let map: HashMap<&'static str, Pattern> = p.into_iter().map(|(k, v)| (&*Box::leak(k.into_boxed_str()), v)).collect();
        let f = |x: &str| {
            let mut out = String::new();
            format(&map["m"], &Env { lang: Lang::En, args: &[("x", x.into())], messages: &map }, 0, &mut out);
            out
        };
        assert_eq!(f("yes"), "a, b");
        assert_eq!(f("no"), "a");
    }
}

/// Turn the one-line selector form `{ $n -> [one] a *[other] b }` (what
/// [`raw`] prints and translators return) into the multi-line form the
/// parser reads. Text without a selector is returned unchanged.
pub fn expand_selectors(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        // `{ $var ->`
        if c[i] == '{' && c.get(i + 1) == Some(&' ') && c.get(i + 2) == Some(&'$') {
            let mut j = i + 3;
            while j < c.len() && (c[j].is_ascii_alphanumeric() || c[j] == '_' || c[j] == '-') {
                j += 1;
            }
            let mut k = j;
            while k < c.len() && c[k] == ' ' {
                k += 1;
            }
            if c.get(k) == Some(&'-') && c.get(k + 1) == Some(&'>') {
                out.extend(&c[i..j]);
                out.push_str(" ->\n");
                i = k + 2;
                loop {
                    while i < c.len() && c[i] == ' ' {
                        i += 1;
                    }
                    if i >= c.len() || c[i] == '}' {
                        out.push_str(" }");
                        i += 1;
                        break;
                    }
                    let star = c[i] == '*';
                    if star {
                        i += 1;
                    }
                    // `[key]`
                    let mut key = String::new();
                    if c.get(i) == Some(&'[') {
                        i += 1;
                        while i < c.len() && c[i] != ']' {
                            key.push(c[i]);
                            i += 1;
                        }
                        i += 1;
                    }
                    while i < c.len() && c[i] == ' ' {
                        i += 1;
                    }
                    // Text up to the next variant or the closing brace.
                    let mut depth = 0;
                    let mut text = String::new();
                    while i < c.len() {
                        if c[i] == '{' {
                            depth += 1;
                        } else if c[i] == '}' {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                        } else if depth == 0 && c[i] == ' ' && (c.get(i + 1) == Some(&'[') || (c.get(i + 1) == Some(&'*') && c.get(i + 2) == Some(&'['))) {
                            break;
                        }
                        text.push(c[i]);
                        i += 1;
                    }
                    out.push_str(if star { "   *[" } else { "    [" });
                    out.push_str(&key);
                    out.push_str("] ");
                    out.push_str(text.trim_end());
                    out.push('\n');
                }
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod expand_tests {
    use super::*;

    #[test]
    fn one_line_selectors_round_trip_through_raw() {
        let src = "m = { $n ->\n    [one] { $n } clip\n   *[other] { $n } clips\n }\n";
        let p = parse(src).unwrap();
        let line = raw(&p[0].1);
        assert_eq!(line, "{ $n -> [one] { $n } clip *[other] { $n } clips }");
        let again = parse(&format!("m = {}\n", expand_selectors(&line))).unwrap();
        assert_eq!(raw(&again[0].1), line);
    }

    #[test]
    fn text_around_a_selector_is_kept() {
        let line = "Moved { $n -> [one] one frame *[other] { $n } frames } later.";
        let p = parse(&format!("m = {}\n", expand_selectors(line))).unwrap();
        assert_eq!(raw(&p[0].1), "Moved { $n -> [one] one frame *[other] { $n } frames } later.");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(expand_selectors("Hello { $name }"), "Hello { $name }");
    }
}

#[cfg(test)]
mod indent_tests {
    use super::*;

    #[test]
    fn unicode_whitespace_in_a_continuation_does_not_panic() {
        // An EM SPACE after one ASCII space of indentation must not be sliced mid-character.
        let r = std::panic::catch_unwind(|| parse("m =\n x\n\u{2003}y\n"));
        assert!(r.is_ok(), "parse must return, not panic");
    }
}
