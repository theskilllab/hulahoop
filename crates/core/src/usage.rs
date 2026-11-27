//! What a run spent, read from the adapter's JSONL stream.
//!
//! pi puts `usage` on every `message_update`, and the value is cumulative
//! within one assistant message: the same snapshot repeats on later updates, so
//! the last update of a message is that message's cost. Summing every update
//! would roughly double the bill, which is why a message is added once, when it
//! finishes.
//!
//! claude can write repeated `assistant` snapshots for one message ID. Keep the
//! latest per ID during interrupted streams; a final `result.usage` supplies
//! authoritative totals. Its usage has provider snake_case counter names.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// A provider's final usage for one response.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Usage {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    cost_usd: Option<f64>,
}

/// Nullable metrics distinguish an unmetered stream from a measured zero.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Metrics {
    pub input: i64,
    pub output: i64,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub turns: Option<i64>,
    pub tool_calls: Option<i64>,
    pub reported_cost_usd: Option<f64>,
    pub actual_model: Option<String>,
    pub measured: bool,
}

impl Metrics {
    /// Everything the model read: fresh input plus cached input read and written.
    /// Providers report the cached part separately, so `input` alone undercounts.
    pub fn input_total(&self) -> i64 {
        self.input
            .saturating_add(self.cache_read.unwrap_or(0))
            .saturating_add(self.cache_write.unwrap_or(0))
    }
}

/// Sum Pi's final assistant snapshots; Claude's result record overrides its
/// repeated assistant events when present. Interrupted streams retain partial
/// distinct-message usage, with no invented provider-reported cost.
pub fn metrics(stream: &str) -> Metrics {
    let mut result = Metrics::default();
    let mut open: Option<Usage> = None;
    let mut seen_messages = HashSet::new();
    let mut claude_snapshots: HashMap<String, Usage> = HashMap::new();
    let mut seen_tools = HashSet::new();
    let mut claude_result: Option<Usage> = None;
    let mut claude_cost: Option<f64> = None;

    for line in stream.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => open = None,
            Some("message_update") => {
                if let Some(usage) = usage_of(&event) {
                    open = Some(usage);
                }
            }
            Some("message_end") => {
                let message = event.get("message");
                if let Some(message) = message {
                    if message.get("role").and_then(Value::as_str) == Some("assistant") {
                        result.turns = Some(result.turns.unwrap_or(0) + 1);
                        count_tools(&mut result, &mut seen_tools, message);
                        model_of(&mut result, message);
                    }
                }
                if let Some(usage) = message.and_then(usage_of).or(open) {
                    add_usage(&mut result, usage);
                }
                open = None;
            }
            Some("assistant") => {
                if let Some(message) = event.get("message") {
                    let id = message.get("id").and_then(Value::as_str);
                    count_tools(&mut result, &mut seen_tools, message);
                    model_of(&mut result, message);
                    if id.is_none() || seen_messages.insert(id.unwrap().to_owned()) {
                        result.turns = Some(result.turns.unwrap_or(0) + 1);
                    }
                    if let Some(usage) = usage_of(message) {
                        if let Some(id) = id {
                            claude_snapshots.insert(id.to_owned(), usage);
                        } else {
                            add_usage(&mut result, usage);
                        }
                    }
                }
            }
            Some("result") => {
                claude_result = usage_of(&event);
                claude_cost = event.get("total_cost_usd").and_then(Value::as_f64);
            }
            _ => {}
        }
    }
    if let Some(usage) = open {
        add_usage(&mut result, usage);
    }
    for usage in claude_snapshots.into_values() {
        add_usage(&mut result, usage);
    }
    if let Some(usage) = claude_result {
        result.input = usage.input;
        result.output = usage.output;
        result.cache_read = Some(usage.cache_read);
        result.cache_write = Some(usage.cache_write);
        result.measured = true;
    }
    if let Some(cost) = claude_cost.filter(|cost| cost.is_finite() && *cost >= 0.0) {
        result.reported_cost_usd = Some(cost);
    }
    result
}

pub fn totals(stream: &str) -> (i64, i64) {
    let usage = metrics(stream);
    (usage.input, usage.output)
}

fn add_usage(result: &mut Metrics, usage: Usage) {
    result.measured = true;
    result.input = result.input.saturating_add(usage.input);
    result.output = result.output.saturating_add(usage.output);
    result.cache_read = Some(
        result
            .cache_read
            .unwrap_or(0)
            .saturating_add(usage.cache_read),
    );
    result.cache_write = Some(
        result
            .cache_write
            .unwrap_or(0)
            .saturating_add(usage.cache_write),
    );
    if let Some(cost) = usage
        .cost_usd
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
    {
        result.reported_cost_usd = Some(result.reported_cost_usd.unwrap_or(0.0) + cost);
    }
}

fn model_of(result: &mut Metrics, message: &Value) {
    if let Some(model) = message.get("model").and_then(Value::as_str) {
        match &result.actual_model {
            None => result.actual_model = Some(model.to_owned()),
            Some(previous) if previous != model => result.actual_model = Some("mixed".to_owned()),
            _ => {}
        }
    }
}

fn count_tools(result: &mut Metrics, seen: &mut HashSet<String>, message: &Value) {
    result.tool_calls.get_or_insert(0);
    for block in message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = block.get("type").and_then(Value::as_str);
        if kind != Some("toolCall") && kind != Some("tool_use") {
            continue;
        }
        let id = block
            .get("id")
            .or_else(|| block.get("toolCallId"))
            .and_then(Value::as_str);
        if id.is_none() || seen.insert(id.unwrap().to_owned()) {
            result.tool_calls = Some(result.tool_calls.unwrap_or(0) + 1);
        }
    }
}

fn usage_of(value: &Value) -> Option<Usage> {
    // An explicit null (or a non-object) is an adapter saying nothing, so the
    // message keeps the last snapshot it reported instead of reading as zero.
    let usage = value.get("usage").filter(|usage| usage.is_object())?;
    Some(Usage {
        input: counter(usage, "input", "input_tokens"),
        output: counter(usage, "output", "output_tokens"),
        cache_read: counter(usage, "cacheRead", "cache_read_input_tokens"),
        cache_write: usage
            .get("cacheWrite")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                counter(
                    usage,
                    "cache_creation_input_tokens",
                    "cache_creation_input_tokens",
                )
            }),
        cost_usd: usage
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64),
    })
}

/// pi names the counters `input`/`output`; claude's provider names them
/// `input_tokens`/`output_tokens`. A missing counter is zero.
fn counter(usage: &Value, camel: &str, snake: &str) -> i64 {
    usage
        .get(camel)
        .or_else(|| usage.get(snake))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{metrics, totals};

    #[test]
    fn claude_repeated_snapshot_counts_new_tools_once() {
        let got = metrics(concat!(
            "{\"type\":\"assistant\",\"message\":{\"id\":\"a\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tool1\"}]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"id\":\"a\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tool1\"},{\"type\":\"tool_use\",\"id\":\"tool2\"}]}}"
        ));
        assert_eq!(got.turns, Some(1));
        assert_eq!(got.tool_calls, Some(2));
    }

    #[test]
    fn measured_assistant_with_no_tools_reports_zero_calls() {
        let got = metrics(
            r#"{"type":"message_end","message":{"role":"assistant","content":[],"usage":{"input":1,"output":2}}}"#,
        );
        assert_eq!(got.tool_calls, Some(0));
    }

    #[test]
    fn pi_counts_final_usage_cache_cost_and_effort_once() {
        let stream = [
            r#"{"type":"message_update","usage":{"input":5,"output":2}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","model":"test","content":[{"type":"toolCall","id":"a"}],"usage":{"input":10,"output":4,"cacheRead":12,"cacheWrite":3,"cost":{"total":0.04}}}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[],"usage":{"input":7,"output":2,"cost":{"total":0.02}}}}"#,
        ].join("\n");
        let got = metrics(&stream);
        assert_eq!(
            (got.input, got.output, got.cache_read, got.cache_write),
            (17, 6, Some(12), Some(3))
        );
        assert_eq!((got.turns, got.tool_calls), (Some(2), Some(1)));
        assert!((got.reported_cost_usd.unwrap() - 0.06).abs() < 0.000001);
        assert_eq!(got.actual_model.as_deref(), Some("test"));
    }

    #[test]
    fn interrupted_claude_stream_keeps_latest_usage_per_message() {
        let stream = [
            r#"{"type":"assistant","message":{"id":"one","usage":{"input_tokens":5,"output_tokens":12}}}"#,
            r#"{"type":"assistant","message":{"id":"one","usage":{"input_tokens":5,"output_tokens":23}}}"#,
            r#"{"type":"assistant","message":{"id":"two","usage":{"input_tokens":9,"output_tokens":4}}}"#,
        ].join("\n");
        let got = metrics(&stream);
        assert_eq!((got.input, got.output), (14, 27));
        assert_eq!(got.turns, Some(2));
    }

    #[test]
    fn claude_result_overrides_repeated_message_snapshots() {
        let stream = [
            r#"{"type":"assistant","message":{"id":"same","content":[{"type":"tool_use","id":"call"}],"usage":{"input_tokens":2,"output_tokens":10}}}"#,
            r#"{"type":"assistant","message":{"id":"same","content":[{"type":"tool_use","id":"call"}],"usage":{"input_tokens":2,"output_tokens":15}}}"#,
            r#"{"type":"result","usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":9,"cache_creation_input_tokens":7},"total_cost_usd":0.12}"#,
        ].join("\n");
        let got = metrics(&stream);
        assert_eq!(
            (got.input, got.output, got.cache_read, got.cache_write),
            (3, 50, Some(9), Some(7))
        );
        assert_eq!((got.turns, got.tool_calls), (Some(1), Some(1)));
        assert_eq!(got.reported_cost_usd, Some(0.12));
    }

    #[test]
    fn missing_usage_is_unknown() {
        let got = metrics("{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\"}}");
        assert!(!got.measured);
        assert_eq!(got.reported_cost_usd, None);
    }

    #[test]
    fn a_repeated_snapshot_is_one_message_not_a_sum() {
        let stream = [
            r#"{"type":"message_start","message":{"role":"assistant"}}"#,
            r#"{"type":"message_update","usage":{"input":100,"output":10}}"#,
            r#"{"type":"message_update","usage":{"input":180,"output":30}}"#,
            r#"{"type":"message_update","usage":{"input":180,"output":30}}"#,
            r#"{"type":"message_update","usage":{"input":180,"output":30}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","usage":{"input":180,"output":30}}}"#,
        ]
        .join("\n");
        assert_eq!(totals(&stream), (180, 30));
    }

    #[test]
    fn finished_messages_add_up() {
        let stream = [
            r#"{"type":"message_update","usage":{"input":100,"output":10}}"#,
            r#"{"type":"message_end","message":{"usage":{"input":120,"output":25}}}"#,
            r#"{"type":"message_update","usage":{"input":300,"output":40}}"#,
            r#"{"type":"message_end","message":{"usage":{"input":350,"output":60}}}"#,
        ]
        .join("\n");
        assert_eq!(totals(&stream), (470, 85));
    }

    #[test]
    fn a_stream_without_usage_spent_nothing_recorded() {
        let stream = r#"{"type":"message_end","message":{"role":"assistant"}}"#;
        assert_eq!(totals(stream), (0, 0));
        assert_eq!(totals(""), (0, 0));
    }

    #[test]
    fn a_null_usage_at_the_end_keeps_the_last_snapshot() {
        let stream = [
            r#"{"type":"message_start","message":{"role":"assistant"}}"#,
            r#"{"type":"message_update","usage":{"input":70,"output":14}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","usage":null}}"#,
        ]
        .join("\n");
        assert_eq!(totals(&stream), (70, 14));
    }

    #[test]
    fn claude_records_add_up_with_their_own_counter_names() {
        let stream = [
            r#"{"type":"system","subtype":"init","session_id":"fixture"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":120,"output_tokens":25,"cache_read_input_tokens":0}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":350,"output_tokens":60,"cache_read_input_tokens":0}}}"#,
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":470,"output_tokens":85}}"#,
        ]
        .join("\n");
        assert_eq!(totals(&stream), (470, 85));
    }

    #[test]
    fn a_message_the_stream_never_finished_keeps_its_last_snapshot() {
        let stream = [
            r#"{"type":"message_update","usage":{"input":40,"output":8}}"#,
            r#"{"type":"message_update","usage":{"input":60,"output":12}}"#,
        ]
        .join("\n");
        assert_eq!(totals(&stream), (60, 12));
    }
}

#[cfg(test)]
mod input_total_tests {
    use super::Metrics;

    #[test]
    fn input_total_includes_cache_read_and_write() {
        let m = Metrics {
            input: 27,
            cache_read: Some(511_000),
            cache_write: Some(40),
            ..Default::default()
        };
        assert_eq!(m.input_total(), 511_067);
    }

    #[test]
    fn input_total_treats_unmetered_cache_as_zero_and_saturates() {
        assert_eq!(
            Metrics {
                input: 5,
                ..Default::default()
            }
            .input_total(),
            5
        );
        let m = Metrics {
            input: i64::MAX,
            cache_read: Some(1),
            ..Default::default()
        };
        assert_eq!(m.input_total(), i64::MAX);
    }
}
