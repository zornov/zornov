// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart data in the shapes people and AIs write it.

use serde_json::Value;

/// One row: a label, its value, and any further numbers (a map's
/// longitude and latitude come before the value).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: f64,
    pub more: Vec<f64>,
    pub colour: Option<String>,
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('%').replace(',', "").parse().ok(),
        _ => None,
    }
}

/// Rows from `[["label", value], …]`, `[{"label", "value", "colour"},
/// …]`, `[1, 2, 3]`, `{"labels": [...], "values": [...]}`, or text with a
/// row a line (`label, value` or `label: value`).
pub fn rows(v: &Value) -> Vec<Row> {
    match v {
        Value::Array(a) => a
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Value::Array(cells) => {
                    let label = match cells.first() {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => num(other).map(|x| x.to_string()).unwrap_or_default(),
                        None => return None,
                    };
                    let nums: Vec<f64> = cells.iter().skip(1).filter_map(num).collect();
                    let (value, more) = match nums.split_last() {
                        Some((v, rest)) => (*v, rest.to_vec()),
                        None => (0.0, Vec::new()),
                    };
                    Some(Row { label, value, more, colour: None })
                }
                Value::Object(o) => {
                    let label = o.get("label").or(o.get("name")).and_then(Value::as_str).unwrap_or("").to_owned();
                    let mut more = Vec::new();
                    if let (Some(lon), Some(lat)) = (o.get("lon").or(o.get("lng")).and_then(num), o.get("lat").and_then(num)) {
                        more = vec![lon, lat];
                    }
                    Some(Row { label, value: o.get("value").and_then(num).unwrap_or(0.0), more, colour: o.get("colour").or(o.get("color")).and_then(Value::as_str).map(str::to_owned) })
                }
                other => num(other).map(|x| Row { label: format!("{}", i + 1), value: x, more: Vec::new(), colour: None }),
            })
            .collect(),
        Value::Object(o) => {
            let labels: Vec<String> = o.get("labels").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect()).unwrap_or_default();
            let values: Vec<f64> = o.get("values").and_then(Value::as_array).map(|a| a.iter().filter_map(num).collect()).unwrap_or_default();
            values.iter().enumerate().map(|(i, v)| Row { label: labels.get(i).cloned().unwrap_or_else(|| format!("{}", i + 1)), value: *v, more: Vec::new(), colour: None }).collect()
        }
        Value::String(s) => s
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| {
                let (label, rest) = l.split_once(',').or_else(|| l.split_once(':')).or_else(|| l.split_once('\t'))?;
                let nums: Vec<f64> = rest.split(',').filter_map(|x| num(&Value::String(x.to_owned()))).collect();
                let (value, more) = nums.split_last().map(|(v, r)| (*v, r.to_vec()))?;
                Some(Row { label: label.trim().to_owned(), value, more, colour: None })
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rows_from_every_shape() {
        assert_eq!(rows(&json!([["A", 3], ["B", "4.5"]]))[1].value, 4.5);
        assert_eq!(rows(&json!([{"label": "x", "value": 2, "colour": "#f00"}]))[0].colour.as_deref(), Some("#f00"));
        assert_eq!(rows(&json!([1, 2, 3])).len(), 3);
        assert_eq!(rows(&json!({"labels": ["a", "b"], "values": [1, 2]}))[1].label, "b");
        let t = rows(&json!("Apples, 12\nPears: 30%\n"));
        assert_eq!((t[0].label.as_str(), t[1].value), ("Apples", 30.0));
        let m = rows(&json!([["London", -0.13, 51.5, 9], {"label": "Paris", "lon": 2.35, "lat": 48.86}]));
        assert_eq!(m[0].more, vec![-0.13, 51.5]);
        assert_eq!(m[1].more, vec![2.35, 48.86]);
    }
}
