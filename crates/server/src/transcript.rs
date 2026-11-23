//! A run's transcript as the desktop shows it: what the agent was asked, what
//! it thought and said, and which tools it called with what result.
//!
//! The runner forwards every stdout line as a `stdout` run event while the run
//! is alive, so those events are the transcript's source and their `seq` is the
//! cursor a polling client passes back. A run with no events (one recorded
//! before events existed) falls back to the `agent.jsonl` the runner wrote,
//! numbered by line. The stream is parsed from its start on every call: the
//! parser pairs tool results with the calls that made them, and a page that
//! started mid-stream would lose those names.

use anyhow::Result;
use hulahoop_core::transcript::Parser;
use serde_json::Value;

use crate::engine::Engine;
use crate::model::Run;

/// One normalized transcript entry, tagged with the line it came from.
#[derive(Debug, Clone, PartialEq, async_graphql::SimpleObject)]
pub struct TranscriptEntry {
    pub seq: i64,
    /// `prompt`, `thinking`, `text`, `tool_call`, `tool_result`, `error` or `done`.
    pub kind: String,
    pub text: String,
    pub tool: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub tool_id: Option<String>,
    pub is_error: bool,
}

/// A page of transcript after a cursor.
#[derive(Debug, Clone, PartialEq, async_graphql::SimpleObject)]
pub struct Transcript {
    pub entries: Vec<TranscriptEntry>,
    /// Pass this back as `afterSeq` to get only what came next.
    pub last_seq: i64,
    /// Whether the run may still add lines.
    pub live: bool,
}

/// How many entries one page carries at most; a client catches up in pages.
const PAGE: usize = 500;

impl Engine {
    pub async fn run_transcript(&self, run: &Run, after_seq: i64) -> Result<Transcript> {
        let live = matches!(run.status.as_str(), "queued" | "running");
        let events: Vec<(i64, String)> = sqlx::query_as(
            "select seq, payload from run_events where run_id = ? and kind = 'stdout' order by seq",
        )
        .bind(run.id)
        .fetch_all(&self.db.pool)
        .await?;
        let lines: Vec<(i64, String)> = if events.is_empty() && !live {
            let path = std::path::Path::new(&run.artifact_dir).join("agent.jsonl");
            let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            text.lines()
                .enumerate()
                .map(|(index, line)| (index as i64 + 1, line.to_string()))
                .collect()
        } else {
            events
                .into_iter()
                .map(|(seq, payload)| {
                    let line = serde_json::from_str::<Value>(&payload)
                        .ok()
                        .and_then(|value| {
                            value.get("line").and_then(Value::as_str).map(str::to_owned)
                        })
                        .unwrap_or(payload);
                    (seq, line)
                })
                .collect()
        };
        Ok(page(&lines, after_seq, live))
    }
}

/// Parse every line, keep the entries after the cursor, and stop at a page.
fn page(lines: &[(i64, String)], after_seq: i64, live: bool) -> Transcript {
    let mut parser = Parser::new();
    let mut entries = Vec::new();
    let mut last_seq = after_seq;
    for (seq, line) in lines {
        let parsed = parser.push(line);
        if *seq <= after_seq {
            continue;
        }
        if entries.len() >= PAGE {
            break;
        }
        last_seq = *seq;
        entries.extend(parsed.into_iter().map(|entry| TranscriptEntry {
            seq: *seq,
            kind: entry.kind,
            text: entry.text,
            tool: entry.tool,
            input: entry.input,
            output: entry.output,
            tool_id: entry.tool_id,
            is_error: entry.is_error,
        }));
    }
    Transcript {
        entries,
        last_seq,
        live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_resumes_after_the_cursor_and_keeps_tool_names() {
        let lines = vec![
            (
                1,
                r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#.to_string(),
            ),
            (
                3,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"README.md"}]}}"#.to_string(),
            ),
        ];
        let first = page(&lines, 0, true);
        assert_eq!(first.last_seq, 3);
        assert_eq!(first.entries.len(), 2);
        let rest = page(&lines, 1, true);
        assert_eq!(rest.entries.len(), 1);
        assert_eq!(rest.entries[0].kind, "tool_result");
        assert_eq!(rest.entries[0].tool.as_deref(), Some("Bash"));
        assert_eq!(page(&lines, 3, false).last_seq, 3);
    }
}
