// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Token counts as the panel shows them. Each brain reports usage in its
//! own shape; these readers turn all of them into one `Usage`. `input`
//! always includes the `cached` part.

use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Tokens read in, cached ones included.
    pub input: u64,
    /// The part of `input` served from the cache.
    pub cached: u64,
    pub output: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output
    }

    pub fn add(&mut self, o: Usage) {
        self.input += o.input;
        self.cached += o.cached;
        self.output += o.output;
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

fn n(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Codex's `usage` object: `{input_tokens, cached_input_tokens,
/// output_tokens}`; its input already counts the cached part.
pub fn codex_usage(u: &Value) -> Option<Usage> {
    if !u.is_object() {
        return None;
    }
    let x = Usage { input: n(u, "input_tokens"), cached: n(u, "cached_input_tokens").min(n(u, "input_tokens")), output: n(u, "output_tokens") };
    (!x.is_empty()).then_some(x)
}

/// One line of Codex's JSONL: the end of a turn (`turn.completed`) or a
/// `token_count` event, bare or wrapped in `msg` / `payload`. A
/// `token_count` carries the last call's usage in `last_token_usage`.
pub fn codex_event(v: &Value) -> Option<Usage> {
    match v["type"].as_str()? {
        "turn.completed" => codex_usage(&v["usage"]),
        "token_count" => codex_usage(&v["info"]["last_token_usage"]),
        "event_msg" if v["payload"]["type"] == "token_count" => codex_usage(&v["payload"]["info"]["last_token_usage"]),
        _ if v["msg"]["type"] == "token_count" => codex_usage(&v["msg"]["info"]["last_token_usage"]),
        _ => None,
    }
}

/// Anthropic's `usage`: fresh input, cache reads and cache writes are
/// counted apart; all three are input.
pub fn anthropic_usage(u: &Value) -> Usage {
    let cached = n(u, "cache_read_input_tokens");
    Usage { input: n(u, "input_tokens") + cached + n(u, "cache_creation_input_tokens"), cached, output: n(u, "output_tokens") }
}

/// OpenAI Responses `usage`: `input_tokens` with the cached part under
/// `input_tokens_details.cached_tokens`.
pub fn openai_usage(u: &Value) -> Usage {
    let input = n(u, "input_tokens");
    Usage { input, cached: n(&u["input_tokens_details"], "cached_tokens").min(input), output: n(u, "output_tokens") }
}

/// `850`, `3.4k`, `21k`, `1.2M`.
pub fn short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1e3),
        10_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_turn_completed() {
        let v = json!({"type": "turn.completed", "usage": {"input_tokens": 24763, "cached_input_tokens": 24448, "output_tokens": 122}});
        assert_eq!(codex_event(&v), Some(Usage { input: 24763, cached: 24448, output: 122 }));
        assert_eq!(codex_event(&json!({"type": "turn.completed"})), None);
    }

    #[test]
    fn codex_token_count_in_each_wrapping() {
        let info = json!({"total_token_usage": {"input_tokens": 900, "output_tokens": 90}, "last_token_usage": {"input_tokens": 500, "cached_input_tokens": 400, "output_tokens": 30, "reasoning_output_tokens": 5}});
        let want = Some(Usage { input: 500, cached: 400, output: 30 });
        assert_eq!(codex_event(&json!({"type": "token_count", "info": info})), want);
        assert_eq!(codex_event(&json!({"type": "event_msg", "payload": {"type": "token_count", "info": info}})), want);
        assert_eq!(codex_event(&json!({"type": "x", "msg": {"type": "token_count", "info": info}})), want);
        assert_eq!(codex_event(&json!({"type": "token_count", "info": null})), None);
        assert_eq!(codex_event(&json!({"type": "item.completed"})), None);
    }

    #[test]
    fn cached_never_exceeds_input() {
        assert_eq!(codex_usage(&json!({"input_tokens": 10, "cached_input_tokens": 99, "output_tokens": 1})).unwrap().cached, 10);
        assert_eq!(openai_usage(&json!({"input_tokens": 10, "input_tokens_details": {"cached_tokens": 99}})).cached, 10);
    }

    #[test]
    fn anthropic_counts_cache_reads_and_writes_as_input() {
        let u = anthropic_usage(&json!({"input_tokens": 10, "cache_read_input_tokens": 90, "cache_creation_input_tokens": 400, "output_tokens": 42}));
        assert_eq!(u, Usage { input: 500, cached: 90, output: 42 });
        assert_eq!(anthropic_usage(&json!({})), Usage::default());
    }

    #[test]
    fn openai_reads_cached_details() {
        let u = openai_usage(&json!({"input_tokens": 900, "input_tokens_details": {"cached_tokens": 640}, "output_tokens": 40}));
        assert_eq!(u, Usage { input: 900, cached: 640, output: 40 });
        assert_eq!(openai_usage(&json!({"input_tokens": 900, "output_tokens": 40})).cached, 0);
    }

    #[test]
    fn labels_are_short_and_calm() {
        assert_eq!(short(850), "850");
        assert_eq!(short(3400), "3.4k");
        assert_eq!(short(21_300), "21k");
        assert_eq!(short(1_300_000), "1.3M");
    }

    #[test]
    fn totals_add_up() {
        let mut a = Usage { input: 5, cached: 2, output: 1 };
        a.add(Usage { input: 5, cached: 3, output: 4 });
        assert_eq!(a, Usage { input: 10, cached: 5, output: 5 });
        assert_eq!(a.total(), 15);
    }
}
