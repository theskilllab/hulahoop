//! What a run spent, read from the adapter's JSONL stream.
//!
//! pi and claude put `usage` on every `message_update`, and the value is
//! cumulative within one assistant message: the same snapshot repeats on later
//! updates, so the last update of a message is that message's cost. Summing
//! every update would roughly double the bill, which is why a message is added
//! once, when it finishes.

use serde_json::Value;

/// One usage snapshot. The cache counters are part of the provider's report but
/// the run row only carries the two numbers the desktop shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Usage {
    input: i64,
    output: i64,
}

/// The input and output tokens a stream spent, summed over its messages.
///
/// A message contributes its final snapshot: the `message_end` usage when the
/// adapter writes one, otherwise the last `message_update` seen for it. A
/// message the stream never finished still spent its last snapshot, so a run
/// killed mid-answer reports what it used.
pub fn totals(stream: &str) -> (i64, i64) {
    let mut input = 0i64;
    let mut output = 0i64;
    let mut open: Option<Usage> = None;

    for line in stream.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
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
                if let Some(usage) = event.get("message").and_then(usage_of) {
                    input += usage.input;
                    output += usage.output;
                } else if let Some(usage) = open {
                    input += usage.input;
                    output += usage.output;
                }
                open = None;
            }
            _ => {}
        }
    }

    if let Some(usage) = open {
        input += usage.input;
        output += usage.output;
    }

    (input, output)
}

fn usage_of(value: &Value) -> Option<Usage> {
    let usage = value.get("usage")?;
    Some(Usage {
        input: usage.get("input").and_then(Value::as_i64).unwrap_or(0),
        output: usage.get("output").and_then(Value::as_i64).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::totals;

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
    fn a_message_the_stream_never_finished_keeps_its_last_snapshot() {
        let stream = [
            r#"{"type":"message_update","usage":{"input":40,"output":8}}"#,
            r#"{"type":"message_update","usage":{"input":60,"output":12}}"#,
        ]
        .join("\n");
        assert_eq!(totals(&stream), (60, 12));
    }
}
