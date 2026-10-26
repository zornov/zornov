// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `repeat`: one op written once, made many times inside the edit, so an
//! agent never needs a script outside Studio to write out a sequence.
//!
//! `{"op": "repeat", "times": 10, "do": [ops]}` or
//! `{"op": "repeat", "for": [{"a": 3, "b": 5}, …], "do": [ops]}` (a list
//! of plain values names each one `item`, or what `as` says). In the ops,
//! `"{expr}"` is replaced: the whole string becomes the value (a number
//! stays a number), and `{expr}` inside longer text becomes its text.
//! `expr` is arithmetic (`+ - * / %`, brackets) over numbers and the
//! variables: `i` (0, 1, 2…), `n` (how many), each item's fields, any
//! list indexed as `heights[i]`, and the variables of a repeat around it.
//! A `"let"` object on the repeat names lists or numbers shared by every
//! turn. Repeats nest.

use serde_json::{Map, Value, json};

/// The ops with every `repeat` written out in full.
pub fn expand(ops: &[Value]) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for op in ops {
        expand_one(op, &Map::new(), &mut out)?;
    }
    Ok(out)
}

fn expand_one(op: &Value, vars: &Map<String, Value>, out: &mut Vec<Value>) -> Result<(), String> {
    let op = match op.as_object().and_then(|o| o.get("repeat").filter(|_| o.len() == 1)) {
        Some(Value::Object(inner)) => {
            let mut o = inner.clone();
            o.insert("op".into(), json!("repeat"));
            Value::Object(o)
        }
        _ => super::args::unwrap_op(op),
    };
    if op.get("op").and_then(Value::as_str) != Some("repeat") {
        out.push(fill(&op, vars)?);
        return Ok(());
    }
    let body = match op.get("do") {
        Some(Value::Array(a)) => a.clone(),
        Some(o @ Value::Object(_)) => vec![o.clone()],
        _ => return Err("repeat needs \"do\": the op (or list of ops) to repeat".into()),
    };
    let mut shared = vars.clone();
    if let Some(Value::Object(l)) = op.get("let") {
        for (k, v) in l {
            let v = fill(v, &shared)?;
            shared.insert(k.clone(), v);
        }
    }
    let items: Vec<Value> = match (op.get("for"), op.get("times")) {
        (Some(f), _) => match fill(f, &shared)? {
            Value::Array(a) => a,
            _ => return Err("repeat \"for\" is a list".into()),
        },
        (None, Some(t)) => {
            let n = match fill(t, &shared)? {
                Value::Number(x) => x.as_f64().unwrap_or(0.0),
                _ => return Err("repeat \"times\" is a number".into()),
            };
            if !(0.0..=10_000.0).contains(&n) {
                return Err(format!("repeat \"times\" is {n}; 0 to 10000"));
            }
            (0..n as usize).map(|i| json!(i)).collect()
        }
        _ => return Err("repeat needs \"times\": n or \"for\": [items]".into()),
    };
    if items.len() > 10_000 {
        return Err("repeat makes at most 10000 turns".into());
    }
    let name = op.get("as").and_then(Value::as_str).unwrap_or("item");
    let n = items.len();
    for (i, item) in items.into_iter().enumerate() {
        let mut v = shared.clone();
        v.insert("i".into(), json!(i));
        v.insert("n".into(), json!(n));
        match item {
            Value::Object(o) if op.get("as").is_none() => v.extend(o),
            other => {
                v.insert(name.into(), other);
            }
        }
        for b in &body {
            expand_one(b, &v, out)?;
        }
    }
    Ok(())
}

/// `v` with every `{expr}` in its strings worked out.
fn fill(v: &Value, vars: &Map<String, Value>) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) => text(s, vars)?,
        Value::Array(a) => Value::Array(a.iter().map(|x| fill(x, vars)).collect::<Result<_, _>>()?),
        Value::Object(o) => {
            // A nested repeat fills its own body turn by turn.
            if o.get("op").and_then(Value::as_str) == Some("repeat") {
                return Ok(v.clone());
            }
            Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), fill(x, vars)?))).collect::<Result<_, String>>()?)
        }
        other => other.clone(),
    })
}

fn text(s: &str, vars: &Map<String, Value>) -> Result<Value, String> {
    if !s.contains('{') || vars.is_empty() {
        return Ok(Value::String(s.to_owned()));
    }
    // The whole string one expression: its value, kind and all.
    if s.starts_with('{') && s.ends_with('}') && s[1..].find('}') == Some(s.len() - 2) {
        if let Some(v) = eval(&s[1..s.len() - 1], vars)? {
            return Ok(v);
        }
        return Ok(Value::String(s.to_owned()));
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('{') {
        let Some(b) = rest[a..].find('}') else { break };
        out.push_str(&rest[..a]);
        let inner = &rest[a + 1..a + b];
        match eval(inner, vars)? {
            Some(Value::String(t)) => out.push_str(&t),
            Some(Value::Number(x)) => out.push_str(&number_text(x.as_f64().unwrap_or(0.0))),
            Some(other) => out.push_str(&other.to_string()),
            None => out.push_str(&rest[a..=a + b]),
        }
        rest = &rest[a + b + 1..];
    }
    out.push_str(rest);
    Ok(Value::String(out))
}

fn number_text(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{}", (x * 1e6).round() / 1e6) }
}

/// Works out `src`; `None` when it is not an expression at all (literal
/// braces in a title), an error when it is one that does not work.
fn eval(src: &str, vars: &Map<String, Value>) -> Result<Option<Value>, String> {
    let src = src.trim();
    if src.is_empty() || !src.chars().all(|c| c.is_ascii_alphanumeric() || " _+-*/%().[]".contains(c)) {
        return Ok(None);
    }
    let toks = lex(src).ok_or_else(|| format!("\"{{{src}}}\" is not a sum Studio can work out"))?;
    // One unknown word in braces is text ("{curly}"), not a mistake.
    if let [Tok::Name(n)] = toks.as_slice() {
        if !vars.contains_key(n) {
            return Ok(None);
        }
    }
    let mut p = Parser { toks: &toks, at: 0, vars };
    let v = p.sum().map_err(|e| format!("\"{{{src}}}\": {e}"))?;
    if p.at != toks.len() {
        return Err(format!("\"{{{src}}}\" has something extra after the sum"));
    }
    Ok(Some(match v {
        Val::Num(x) => {
            if x.fract() == 0.0 && x.abs() < 1e15 { json!(x as i64) } else { json!((x * 1e6).round() / 1e6) }
        }
        Val::Json(j) => j,
    }))
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(char),
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == ' ' {
            i += 1;
        } else if ch.is_ascii_digit() || ch == '.' {
            let j = (i..c.len()).find(|&j| !(c[j].is_ascii_digit() || c[j] == '.')).unwrap_or(c.len());
            out.push(Tok::Num(c[i..j].iter().collect::<String>().parse().ok()?));
            i = j;
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let j = (i..c.len()).find(|&j| !(c[j].is_ascii_alphanumeric() || c[j] == '_')).unwrap_or(c.len());
            out.push(Tok::Name(c[i..j].iter().collect()));
            i = j;
        } else {
            out.push(Tok::Op(ch));
            i += 1;
        }
    }
    Some(out)
}

enum Val {
    Num(f64),
    Json(Value),
}

impl Val {
    fn num(&self) -> Result<f64, String> {
        match self {
            Val::Num(x) => Ok(*x),
            Val::Json(Value::Number(x)) => Ok(x.as_f64().unwrap_or(0.0)),
            Val::Json(Value::Bool(b)) => Ok(*b as u8 as f64),
            Val::Json(other) => Err(format!("{other} is not a number")),
        }
    }
}

struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
    vars: &'a Map<String, Value>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }
    fn sum(&mut self) -> Result<Val, String> {
        let mut v = self.product()?;
        while let Some(Tok::Op(o @ ('+' | '-'))) = self.peek().cloned() {
            self.at += 1;
            let r = self.product()?.num()?;
            v = Val::Num(if o == '+' { v.num()? + r } else { v.num()? - r });
        }
        Ok(v)
    }
    fn product(&mut self) -> Result<Val, String> {
        let mut v = self.unary()?;
        while let Some(Tok::Op(o @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.at += 1;
            let r = self.unary()?.num()?;
            let l = v.num()?;
            v = Val::Num(match o {
                '*' => l * r,
                '/' if r == 0.0 => return Err("division by zero".into()),
                '/' => l / r,
                _ if r == 0.0 => return Err("% by zero".into()),
                _ => l.rem_euclid(r),
            });
        }
        Ok(v)
    }
    fn unary(&mut self) -> Result<Val, String> {
        if self.peek() == Some(&Tok::Op('-')) {
            self.at += 1;
            return Ok(Val::Num(-self.unary()?.num()?));
        }
        self.atom()
    }
    fn atom(&mut self) -> Result<Val, String> {
        let t = self.peek().cloned().ok_or("the sum ends too soon")?;
        self.at += 1;
        let mut v = match t {
            Tok::Num(x) => Val::Num(x),
            Tok::Op('(') => {
                let v = self.sum()?;
                if self.peek() != Some(&Tok::Op(')')) {
                    return Err("a ( is not closed".into());
                }
                self.at += 1;
                v
            }
            Tok::Name(n) => {
                let known: Vec<&str> = self.vars.keys().map(String::as_str).collect();
                Val::Json(self.vars.get(&n).cloned().ok_or_else(|| format!("no variable \"{n}\" (there are: {})", known.join(", ")))?)
            }
            Tok::Op(o) => return Err(format!("unexpected \"{o}\"")),
        };
        // Indexing a list: heights[i], steps[i][0].
        while self.peek() == Some(&Tok::Op('[')) {
            self.at += 1;
            let k = self.sum()?.num()?;
            if self.peek() != Some(&Tok::Op(']')) {
                return Err("a [ is not closed".into());
            }
            self.at += 1;
            let Val::Json(Value::Array(a)) = &v else { return Err("only a list can be indexed with [ ]".into()) };
            let idx = if k < 0.0 { a.len() as f64 + k } else { k };
            v = Val::Json(a.get(idx as usize).filter(|_| idx >= 0.0).cloned().ok_or_else(|| format!("index {k} is outside a list of {}", a.len()))?);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_fill_numbers_text_and_lists() {
        let ops = vec![json!({"op": "repeat", "let": {"h": [3, 1, 2]}, "times": 3, "do": [
            {"op": "card", "at": "{i*0.5}", "height": "{h[i]*100}", "x": "{(i - (n-1)/2) * 120}", "colour": "#ffffff"},
            {"op": "text", "text": "bar {i+1} of {n}", "at": "{i*0.5}"}
        ]})];
        let out = expand(&ops).unwrap();
        assert_eq!(out.len(), 6);
        assert_eq!(out[2]["at"], json!(0.5));
        assert_eq!(out[2]["height"], json!(100));
        assert_eq!(out[0]["x"], json!(-120));
        assert_eq!(out[5]["text"], json!("bar 3 of 3"));
        assert_eq!(out[1]["colour"], Value::Null);
    }

    #[test]
    fn for_items_nest_and_say_what_is_wrong() {
        let ops = vec![json!({"repeat": {"for": [{"a": 0, "b": 1}, {"a": 2, "b": 3}], "do": {"op": "repeat", "times": 2, "do": {"op": "marker", "at": "{a + b + i}", "note": "{x}"}}}})];
        let out = expand(&ops).unwrap();
        let ats: Vec<f64> = out.iter().map(|o| o["at"].as_f64().unwrap()).collect();
        assert_eq!(ats, vec![1.0, 2.0, 5.0, 6.0]);
        // A literal brace in text is left alone; a bad name is not.
        assert_eq!(out[0]["note"], json!("{x}"));
        let bad = expand(&[json!({"op": "repeat", "times": 1, "do": {"op": "marker", "at": "{i + y}"}})]).unwrap_err();
        assert!(bad.contains("no variable \"y\""), "{bad}");
        // Ops outside a repeat are untouched.
        assert_eq!(expand(&[json!({"op": "text", "text": "{curly}"})]).unwrap()[0]["text"], json!("{curly}"));
    }
}
