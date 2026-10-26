// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI editor on the OpenAI API (the Responses API: GPT and Codex
//! models), called with the system `curl` and read as server-sent events.
//!
//! The conversation lives on OpenAI's side: each request names the
//! previous response, so reasoning carries across tool calls without being
//! sent back. Tool results go back as `function_call_output` items, with
//! the pictures the tools return as images.

use super::Content;
use super::brain::{AgentEvent, Brain, Gate, one_line, read_lines};
use super::live::ToolResult;
use serde_json::{Value, json};
use std::sync::mpsc;

/// Models the panel offers: (id, name).
pub const MODELS: &[(&str, &str)] = &[("gpt-6.1-sol", "GPT-6.1 Sol"), ("gpt-6-astra", "GPT-6 Astra"), ("gpt-6-luna", "GPT-6 Luna")];

pub struct OpenAiBrain {
    pub key: String,
    pub model: String,
    /// The last response, which the next request continues.
    previous: Option<String>,
    tools: Vec<Value>,
    /// Its place on a crew, and the brief that goes with it.
    pub role: Option<super::crew::Role>,
    pub brief: String,
}

impl OpenAiBrain {
    pub fn new(key: String, model: String) -> Self {
        let mut tools: Vec<Value> = super::mcp::tools()
            .into_iter()
            .map(|t| json!({"type": "function", "name": t["name"], "description": t["description"], "parameters": t["inputSchema"], "strict": false}))
            .collect();
        tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Self { key, model, previous: None, tools, role: None, brief: String::new() }
    }

    fn body(&self, input: Vec<Value>) -> Value {
        let mut b = json!({
            "model": self.model,
            "instructions": if self.brief.is_empty() { super::brain::system_prompt() } else { format!("{}\n\n{}", self.brief, super::brain::system_prompt()) },
            "input": input,
            "tools": self.tools,
            "stream": true,
            "store": true,
        });
        if let Some(id) = &self.previous {
            b["previous_response_id"] = json!(id);
        }
        b
    }

    fn respond(&self, input: Vec<Value>, gate: &Gate) -> Result<Response, String> {
        let body = serde_json::to_vec(&self.body(input)).map_err(|e| e.to_string())?;
        let mut wait = 4u64;
        for attempt in 0..4 {
            match stream_once(&self.key, &body, gate)? {
                Reply::Ok(r) => return Ok(r),
                Reply::Retry(why) if attempt < 3 => {
                    gate.emit(AgentEvent::Note(format!("{why}; trying again in {wait}s")));
                    for _ in 0..wait * 10 {
                        if gate.stopped() {
                            return Err("stopped".into());
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    wait *= 2;
                }
                Reply::Retry(why) | Reply::Fail(why) => return Err(why),
            }
        }
        Err("the OpenAI API is not answering".into())
    }
}

impl Brain for OpenAiBrain {
    fn forget(&mut self) {
        self.previous = None;
    }

    fn turn(&mut self, request: &str, gate: &Gate) -> Result<(), String> {
        let mut input = vec![json!({"role": "user", "content": request})];
        loop {
            if gate.stopped() {
                return Ok(());
            }
            let r = match self.respond(std::mem::take(&mut input), gate) {
                Ok(r) => r,
                Err(_) if gate.stopped() => return Ok(()),
                Err(e) => return Err(e),
            };
            self.previous = Some(r.id.clone());
            gate.emit(AgentEvent::Usage(r.usage));
            if r.calls.is_empty() {
                return match r.status.as_str() {
                    "incomplete" => Err("The answer ran past its length limit.".into()),
                    _ => Ok(()),
                };
            }
            for (call_id, name, args) in r.calls {
                let res = match serde_json::from_str::<Value>(if args.trim().is_empty() { "{}" } else { &args }) {
                    Ok(a) => gate.call_as(self.role, &name, &a),
                    Err(e) => Err(format!("The arguments were not valid JSON ({e}); call it again.")),
                };
                input.push(json!({"type": "function_call_output", "call_id": call_id, "output": output_of(res)}));
            }
        }
    }
}

/// A tool's result as `function_call_output.output`: text, or text and
/// pictures.
fn output_of(r: ToolResult) -> Value {
    match r {
        Ok(parts) if parts.iter().all(|p| matches!(p, Content::Text(_))) => {
            let text: Vec<String> = parts.into_iter().filter_map(|p| if let Content::Text(t) = p { Some(t) } else { None }).collect();
            json!(if text.is_empty() { "ok".to_owned() } else { text.join("\n") })
        }
        Ok(parts) => json!(parts
            .into_iter()
            .map(|p| match p {
                Content::Text(t) => json!({"type": "input_text", "text": t}),
                Content::Jpeg(b) => json!({"type": "input_image", "image_url": format!("data:image/jpeg;base64,{}", super::mcp::base64(&b))}),
            })
            .collect::<Vec<_>>()),
        Err(e) => json!(format!("Error: {e}")),
    }
}

/// One finished response.
#[derive(Default)]
struct Response {
    id: String,
    status: String,
    /// (call id, tool name, arguments as JSON text)
    calls: Vec<(String, String, String)>,
    usage: super::usage::Usage,
}

enum Reply {
    Ok(Response),
    Retry(String),
    Fail(String),
}

/// Reads one `data:` payload; text deltas go to `emit`. Returns the
/// response once it is complete.
fn event(v: &Value, emit: &mut dyn FnMut(String), error: &mut Option<String>) -> Option<Response> {
    match v["type"].as_str().unwrap_or("") {
        "response.output_text.delta" => {
            emit(v["delta"].as_str().unwrap_or("").to_owned());
            None
        }
        "response.completed" | "response.incomplete" => {
            let r = &v["response"];
            let calls = r["output"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|o| o["type"] == "function_call")
                .map(|o| (o["call_id"].as_str().unwrap_or("").to_owned(), o["name"].as_str().unwrap_or("").to_owned(), o["arguments"].as_str().unwrap_or("").to_owned()))
                .collect();
            Some(Response {
                id: r["id"].as_str().unwrap_or("").to_owned(),
                status: r["status"].as_str().unwrap_or("completed").to_owned(),
                calls,
                usage: super::usage::openai_usage(&r["usage"]),
            })
        }
        "response.failed" => {
            *error = Some(v["response"]["error"]["message"].as_str().unwrap_or("the response failed").to_owned());
            None
        }
        "error" => {
            *error = Some(v["message"].as_str().or(v["error"]["message"].as_str()).unwrap_or("the OpenAI API sent an error").to_owned());
            None
        }
        _ => None,
    }
}

fn stream_once(key: &str, body: &[u8], gate: &Gate) -> Result<Reply, String> {
    // The key stays in memory, in a header.
    let auth = format!("Bearer {key}");
    let headers = [("content-type", "application/json"), ("authorization", auth.as_str())];
    let reply = match studio_net::post("https://api.openai.com/v1/responses", &headers, body, std::time::Duration::from_secs(300)) {
        Ok(r) => r,
        Err(_) => return Ok(Reply::Retry("no connection to the OpenAI API".into())),
    };
    let status = reply.status as u32;
    let lines = read_lines(Some(reply.body));
    let mut done = None;
    let mut error = None;
    let mut other = String::new();
    loop {
        match lines.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(data) = line.strip_prefix("data:") {
                    if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                        if let Some(r) = event(&v, &mut |t| gate.emit(AgentEvent::Text(t)), &mut error) {
                            done = Some(r);
                        }
                    }
                } else if !line.starts_with("event:") {
                    other.push_str(&line);
                }
            }
            // Stopped: leave the stream (its reader ends with the reply).
            Err(mpsc::RecvTimeoutError::Timeout) if gate.stopped() => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if gate.stopped() {
        return Err("stopped".into());
    }
    let api_msg = || serde_json::from_str::<Value>(&other).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_owned)).unwrap_or_else(|| one_line(&other, 200));
    Ok(match status {
        200 => match (done, error) {
            (_, Some(e)) => Reply::Fail(e),
            (Some(r), None) => Reply::Ok(r),
            (None, None) => Reply::Retry("the answer was cut off".into()),
        },
        401 | 403 => Reply::Fail(format!("The OpenAI API key was refused ({}). Check it in Settings ▸ AI.", api_msg())),
        429 => Reply::Retry("rate limited".into()),
        500..=599 => Reply::Retry("the OpenAI API is busy".into()),
        _ => Reply::Fail(format!("The OpenAI API said: {}", api_msg())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streamed_response_gives_its_text_and_calls() {
        let mut said = String::new();
        let mut err = None;
        let mut done = None;
        for e in [
            json!({"type": "response.output_text.delta", "delta": "Cutting "}),
            json!({"type": "response.output_text.delta", "delta": "now."}),
            json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "output": [
                {"type": "reasoning", "id": "rs_1"},
                {"type": "message", "content": [{"type": "output_text", "text": "Cutting now."}]},
                {"type": "function_call", "call_id": "call_1", "name": "edit", "arguments": "{\"ops\":[{\"op\":\"split\",\"at\":4.2}]}"}
            ], "usage": {"input_tokens": 900, "output_tokens": 40}}}),
        ] {
            if let Some(r) = event(&e, &mut |t| said.push_str(&t), &mut err) {
                done = Some(r);
            }
        }
        let r = done.unwrap();
        assert_eq!(said, "Cutting now.");
        assert_eq!(r.id, "resp_1");
        assert_eq!(r.calls, vec![("call_1".to_owned(), "edit".to_owned(), "{\"ops\":[{\"op\":\"split\",\"at\":4.2}]}".to_owned())]);
        assert_eq!((r.usage.input, r.usage.output), (900, 40));
        assert!(err.is_none());
    }

    #[test]
    fn pictures_go_back_as_images() {
        let v = output_of(Ok(vec![Content::Text("t=4".into()), Content::Jpeg(vec![1, 2, 3])]));
        assert_eq!(v[0]["type"], "input_text");
        assert_eq!(v[1]["image_url"], "data:image/jpeg;base64,AQID");
        assert_eq!(output_of(Ok(vec![Content::Text("ok".into())])), json!("ok"));
        assert_eq!(output_of(Err("no".into())), json!("Error: no"));
    }

    #[test]
    fn every_tool_is_offered() {
        let b = OpenAiBrain::new(String::new(), MODELS[0].0.into());
        assert!(b.tools.iter().all(|t| t["type"] == "function" && t["parameters"]["type"] == "object"));
        assert!(b.body(vec![]).get("previous_response_id").is_none());
    }
}
