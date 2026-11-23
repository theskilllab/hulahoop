//! A readable transcript of a run, rebuilt from the adapter's JSONL stream.
//!
//! The two harnesses speak different dialects. Claude Code's `stream-json`
//! writes one `assistant` record per content block (and may repeat a snapshot
//! of the same message), answers tool calls in `user` records carrying
//! `tool_result` blocks, and closes with a `result` record. pi's `--mode json`
//! streams deltas in `message_update`, but every message also arrives whole in
//! `message_end`, with tool output as a `toolResult` message; `agent_end`
//! closes the run.
//!
//! Both are folded into one small vocabulary so a client can render a run
//! without knowing which harness produced it. Deltas are ignored on purpose:
//! a finished message is the unit people read, and repeating the same text at
//! every keystroke would make a live view jitter.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool inputs and outputs can be whole files. A transcript is for reading, so
/// anything longer than this is cut and marked.
pub const MAX_FIELD_CHARS: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    /// One of `prompt`, `thinking`, `text`, `tool_call`, `tool_result`,
    /// `error`, `done`.
    pub kind: String,
    pub text: String,
    pub tool: Option<String>,
    /// Tool arguments, as compact JSON.
    pub input: Option<String>,
    pub output: Option<String>,
    pub tool_id: Option<String>,
    pub is_error: bool,
}

impl Entry {
    fn new(kind: &str, text: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            text: clip(&text.into()),
            tool: None,
            input: None,
            output: None,
            tool_id: None,
            is_error: false,
        }
    }
}

/// Reads a stream line by line. It keeps a little state, because Claude
/// repeats snapshots and names a tool only on the call, never on its result.
#[derive(Debug, Default)]
pub struct Parser {
    seen: HashSet<String>,
    tool_names: HashMap<String, String>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The entries one stdout line contributes. Blank lines contribute
    /// nothing; a line that is not JSON is plain output from the harness and
    /// is shown as text so nothing the agent printed disappears.
    pub fn push(&mut self, line: &str) -> Vec<Entry> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return vec![Entry::new("text", line)];
        };
        match event.get("type").and_then(Value::as_str) {
            // Claude Code.
            Some("assistant") => self.claude_assistant(&event),
            Some("user") => self.claude_user(&event),
            Some("result") => claude_result(&event),
            // pi.
            Some("message_end") => self.pi_message(&event),
            Some("agent_end") => vec![Entry::new("done", "")],
            Some("error") => vec![Entry::new("error", error_text(&event))],
            _ => Vec::new(),
        }
    }

    fn claude_assistant(&mut self, event: &Value) -> Vec<Entry> {
        let Some(message) = event.get("message") else {
            return Vec::new();
        };
        let id = message.get("id").and_then(Value::as_str).unwrap_or("");
        let mut entries = Vec::new();
        for block in blocks(message) {
            // A repeated snapshot carries the blocks already shown, so a block
            // is keyed by its message and content and shown once.
            let key = format!("{id}:{block}");
            if !id.is_empty() && !self.seen.insert(key) {
                continue;
            }
            if let Some(entry) = self.assistant_block(block) {
                entries.push(entry);
            }
        }
        entries
    }

    fn claude_user(&mut self, event: &Value) -> Vec<Entry> {
        let Some(message) = event.get("message") else {
            return Vec::new();
        };
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            return vec![Entry::new("prompt", text)];
        }
        let mut entries = Vec::new();
        for block in blocks(message) {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    let id = block.get("tool_use_id").and_then(Value::as_str);
                    let is_error = block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    entries.push(self.tool_result(
                        id,
                        None,
                        &content_text(block.get("content")),
                        is_error,
                    ));
                }
                Some("text") => {
                    entries.push(Entry::new("prompt", str_field(block, "text")));
                }
                _ => {}
            }
        }
        entries
    }

    fn pi_message(&mut self, event: &Value) -> Vec<Entry> {
        let Some(message) = event.get("message") else {
            return Vec::new();
        };
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                let text = content_text(message.get("content"));
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![Entry::new("prompt", text)]
                }
            }
            Some("assistant") => {
                let mut entries: Vec<Entry> = blocks(message)
                    .filter_map(|block| self.assistant_block(block))
                    .collect();
                if message.get("stopReason").and_then(Value::as_str) == Some("error") {
                    let text = message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .unwrap_or("the model returned an error");
                    entries.push(Entry::new("error", text));
                }
                entries
            }
            Some("toolResult") => {
                let id = message.get("toolCallId").and_then(Value::as_str);
                let name = message.get("toolName").and_then(Value::as_str);
                let is_error = message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                vec![self.tool_result(id, name, &content_text(message.get("content")), is_error)]
            }
            _ => Vec::new(),
        }
    }

    /// One block of an assistant message, in either dialect.
    fn assistant_block(&mut self, block: &Value) -> Option<Entry> {
        match block.get("type").and_then(Value::as_str)? {
            "text" => {
                let text = str_field(block, "text");
                (!text.trim().is_empty()).then(|| Entry::new("text", text))
            }
            "thinking" => {
                let text = str_field(block, "thinking");
                (!text.trim().is_empty()).then(|| Entry::new("thinking", text))
            }
            "tool_use" | "toolCall" => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                let id = block.get("id").and_then(Value::as_str);
                let input = block.get("input").or_else(|| block.get("arguments"));
                if let Some(id) = id {
                    self.tool_names.insert(id.to_owned(), name.to_owned());
                }
                let mut entry = Entry::new("tool_call", "");
                entry.tool = Some(name.to_owned());
                entry.tool_id = id.map(str::to_owned);
                entry.input = input.map(|input| clip(&input.to_string()));
                Some(entry)
            }
            _ => None,
        }
    }

    fn tool_result(
        &self,
        id: Option<&str>,
        name: Option<&str>,
        output: &str,
        is_error: bool,
    ) -> Entry {
        let mut entry = Entry::new("tool_result", "");
        entry.tool_id = id.map(str::to_owned);
        entry.tool = name
            .map(str::to_owned)
            .or_else(|| id.and_then(|id| self.tool_names.get(id).cloned()));
        entry.output = Some(clip(output));
        entry.is_error = is_error;
        entry
    }
}

/// Parse a whole stream at once.
pub fn parse(stream: &str) -> Vec<Entry> {
    let mut parser = Parser::new();
    stream.lines().flat_map(|line| parser.push(line)).collect()
}

fn claude_result(event: &Value) -> Vec<Entry> {
    let is_error = event
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || event
            .get("subtype")
            .and_then(Value::as_str)
            .is_some_and(|subtype| subtype.starts_with("error"));
    let text = event.get("result").and_then(Value::as_str).unwrap_or("");
    if is_error {
        let text = if text.is_empty() {
            error_text(event)
        } else {
            text.to_owned()
        };
        vec![Entry::new("error", text)]
    } else {
        vec![Entry::new("done", text)]
    }
}

fn blocks(message: &Value) -> impl Iterator<Item = &Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Content is either a string or a list of blocks; only text is kept.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn str_field(block: &Value, key: &str) -> String {
    block
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn error_text(event: &Value) -> String {
    for key in ["error", "message", "subtype"] {
        match event.get(key) {
            Some(Value::String(text)) => return text.clone(),
            Some(Value::Object(object)) => {
                if let Some(text) = object.get("message").and_then(Value::as_str) {
                    return text.to_owned();
                }
            }
            _ => {}
        }
    }
    "the agent reported an error".to_owned()
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_FIELD_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_FIELD_CHARS).collect();
    format!("{cut}\n[truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.kind.as_str()).collect()
    }

    #[test]
    fn claude_stream_json_reads_as_a_conversation() {
        let stream = [
            r#"{"type":"system","subtype":"init","session_id":"fixture"}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"thinking","thinking":"Look at calc.py first."}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Reading the file."}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"calc.py"}}]}}"#,
            // A repeated snapshot of the same message adds nothing new.
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Reading the file."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"def add(a, b): ..."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"no such file"}],"is_error":true}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Added mul."}"#,
        ]
        .join("\n");
        let entries = parse(&stream);
        assert_eq!(
            kinds(&entries),
            vec![
                "thinking",
                "text",
                "tool_call",
                "tool_result",
                "tool_result",
                "done"
            ]
        );
        assert_eq!(entries[2].tool.as_deref(), Some("Read"));
        assert_eq!(
            entries[2].input.as_deref(),
            Some(r#"{"file_path":"calc.py"}"#)
        );
        // The result names the tool its call used.
        assert_eq!(entries[3].tool.as_deref(), Some("Read"));
        assert_eq!(entries[3].output.as_deref(), Some("def add(a, b): ..."));
        assert!(entries[4].is_error);
        assert_eq!(entries[5].text, "Added mul.");
    }

    #[test]
    fn claude_error_result_is_an_error() {
        let entries = parse(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#);
        assert_eq!(kinds(&entries), vec!["error"]);
        assert_eq!(entries[0].text, "error_max_turns");
    }

    #[test]
    fn pi_json_mode_reads_whole_messages_and_skips_deltas() {
        let stream = [
            r#"{"type":"agent_start"}"#,
            r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"Read input.json first."}]}}"#,
            r#"{"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":{"type":"text_delta","delta":"Read"}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Plan the change."},{"type":"text","text":"Running the tests."},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"just test"}}],"usage":{"input":10,"output":4}}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"bash","result":{"content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"message_end","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":"2 passed"}],"isError":false}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"rate limited"}}"#,
            r#"{"type":"agent_end"}"#,
        ]
        .join("\n");
        let entries = parse(&stream);
        assert_eq!(
            kinds(&entries),
            vec![
                "prompt",
                "thinking",
                "text",
                "tool_call",
                "tool_result",
                "error",
                "done"
            ]
        );
        assert_eq!(entries[0].text, "Read input.json first.");
        assert_eq!(entries[3].tool.as_deref(), Some("bash"));
        assert_eq!(entries[3].tool_id.as_deref(), Some("c1"));
        assert_eq!(
            entries[3].input.as_deref(),
            Some(r#"{"command":"just test"}"#)
        );
        assert_eq!(entries[4].output.as_deref(), Some("2 passed"));
        assert_eq!(entries[5].text, "rate limited");
    }

    #[test]
    fn plain_output_is_kept_and_long_fields_are_cut() {
        let mut parser = Parser::new();
        assert_eq!(kinds(&parser.push("warming up")), vec!["text"]);
        assert!(parser.push("   ").is_empty());
        let long = "z".repeat(MAX_FIELD_CHARS + 10);
        let line = serde_json::json!({
            "type": "message_end",
            "message": {"role": "toolResult", "toolCallId": "x", "content": long},
        })
        .to_string();
        let entries = parser.push(&line);
        assert!(entries[0].output.as_ref().unwrap().ends_with("[truncated]"));
    }
}
