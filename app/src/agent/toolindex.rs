// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tools, listed for reading: `guide {topic: "tools"}` gives every tool
//! with its fields in a line each, and `guide {topic: "tool:<name>"}` adds
//! that tool's fields with their descriptions to its long form.
//!
//! An agent that wants to know what a tool takes would otherwise dump the
//! tool definitions (tens of thousands of tokens of JSON schema, again each
//! time it forgets). Both views are made from the same list the tools are
//! served from, so they cannot go stale.

use serde_json::Value;

/// The first sentence of `d`, at most `max` characters.
fn gist(d: &str, max: usize) -> String {
    let first = d.split_inclusive(". ").next().unwrap_or(d).trim().trim_end_matches('.');
    let mut out: String = first.chars().take(max).collect();
    if first.chars().count() > max {
        out.push_str("...");
    }
    out
}

fn type_of(p: &Value) -> String {
    let t = match &p["type"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("|"),
        _ => String::new(),
    };
    match p["enum"].as_array() {
        Some(e) => format!("one of {}", e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
        None => t,
    }
}

/// Every tool, one line each: `name(fields, required*)  what it is for`.
pub fn index(tools: &[Value]) -> String {
    let mut s = String::from("# The tools\nOne line each: the tool, its fields (* required) and what it is for. A tool's fields in full, with what each means: `guide {\"topic\": \"tool:<name>\"}`. A field a tool does not have is refused with the right ones.\n\n");
    for t in tools {
        let name = t["name"].as_str().unwrap_or("");
        let required: Vec<&str> = t["inputSchema"]["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let fields: Vec<String> = t["inputSchema"]["properties"].as_object().map(|o| o.keys().map(|k| if required.contains(&k.as_str()) { format!("{k}*") } else { k.clone() }).collect()).unwrap_or_default();
        let shown = fields.iter().take(12).cloned().collect::<Vec<_>>().join(", ");
        let more = if fields.len() > 12 { format!(", +{}", fields.len() - 12) } else { String::new() };
        s.push_str(&format!("- {name}({shown}{more}): {}\n", gist(t["description"].as_str().unwrap_or(""), 90)));
    }
    s
}

/// One tool's fields with their descriptions, or `None` for an unknown
/// tool.
pub fn fields(tools: &[Value], name: &str) -> Option<String> {
    let t = tools.iter().find(|t| t["name"] == name)?;
    let required: Vec<&str> = t["inputSchema"]["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let props = t["inputSchema"]["properties"].as_object()?;
    if props.is_empty() {
        return Some("Fields: none.".into());
    }
    let mut s = String::from("Fields:\n");
    for (k, p) in props {
        let req = if required.contains(&k.as_str()) { ", required" } else { "" };
        let d = p["description"].as_str().unwrap_or("");
        s.push_str(&format!("- `{k}` ({}{req}){}{d}\n", type_of(p), if d.is_empty() { "" } else { ": " }));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<Value> {
        vec![json!({"name": "frame", "description": "Look at the picture at time t. Returns a JPEG.", "inputSchema": {"type": "object", "properties": {"t": {"type": "number", "description": "seconds"}, "detail": {"type": "boolean"}, "mode": {"enum": ["a", "b"]}}, "required": ["t"]}})]
    }

    #[test]
    fn the_index_lists_fields_and_marks_required_ones() {
        let i = index(&tools());
        assert!(i.contains("- frame(t*, detail, mode): Look at the picture at time t"), "{i}");
    }

    #[test]
    fn a_tool_gives_its_fields_with_types_and_meaning() {
        let f = fields(&tools(), "frame").unwrap();
        assert!(f.contains("- `t` (number, required): seconds") && f.contains("- `detail` (boolean)\n") && f.contains("one of a, b"), "{f}");
        assert!(fields(&tools(), "nope").is_none());
    }
}
