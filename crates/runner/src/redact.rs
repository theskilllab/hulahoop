//! Keeping a run's credentials out of what it reports.
//!
//! An agent has a shell, so `env` or `cat ~/.pi/agent/auth.json` prints the
//! keys and OAuth tokens the run was handed. Every line the runner posts — run
//! events, the transcript artifact, a Lumi reply — goes through a [`Redactor`]
//! built from the run's own credential first.

use hulahoop_core::contract;
use serde_json::Value;

/// What a redacted secret reads as.
const REDACTED: &str = "[redacted]";
/// Shorter values are labels (`oauth`, `api_key`), not secrets, and replacing
/// them would mangle ordinary output.
const MIN_SECRET: usize = 8;
/// A run of token characters this long inside a credential value is redacted on
/// its own too, so one segment of a JWT or a token quoted without its prefix
/// still goes.
const MIN_TOKEN_PIECE: usize = 20;

#[derive(Debug, Default, Clone)]
pub struct Redactor {
    /// Longest first, so a secret that contains another is replaced whole.
    secrets: Vec<String>,
}

impl Redactor {
    /// The secrets in a spec's credential and in the environment the agent runs
    /// with: every credential env value, every string inside a credential file,
    /// and any host key or token passed through.
    pub fn new(spec: &Value, env: &[(String, String)]) -> Self {
        let mut found = Vec::new();
        let credential = &spec["credential"];
        for entry in credential["env"].as_array().into_iter().flatten() {
            if let Some(value) = entry.get(1).and_then(Value::as_str) {
                found.push(value.to_string());
            }
        }
        for entry in credential["files"].as_array().into_iter().flatten() {
            if let Some(contents) = entry.get(1).and_then(Value::as_str) {
                match serde_json::from_str::<Value>(contents) {
                    Ok(file) => strings_in(&file, &mut found),
                    Err(_) => found.push(contents.to_string()),
                }
            }
        }
        for (name, value) in env {
            let host_secret = contract::CREDENTIAL_ENV.contains(&name.as_str())
                && (name.contains("KEY") || name.contains("TOKEN"));
            if host_secret || name == "GITHUB_TOKEN" {
                found.push(value.clone());
            }
        }
        Self::from_values(found)
    }

    fn from_values(values: Vec<String>) -> Self {
        let mut secrets = Vec::new();
        for value in values {
            let value = value.trim();
            if value.len() >= MIN_SECRET {
                secrets.push(value.to_string());
            }
            secrets.extend(
                value
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                    .filter(|piece| piece.len() >= MIN_TOKEN_PIECE)
                    .map(str::to_string),
            );
        }
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        secrets.dedup();
        Redactor { secrets }
    }

    /// The line with every known secret replaced.
    pub fn redact(&self, line: &str) -> String {
        let mut line = line.to_string();
        for secret in &self.secrets {
            if line.contains(secret.as_str()) {
                line = line.replace(secret.as_str(), REDACTED);
            }
        }
        line
    }
}

fn strings_in(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(text) => found.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings_in(item, found)),
        Value::Object(map) => map.values().for_each(|item| strings_in(item, found)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credential_env_and_file_values_are_redacted() {
        let file = json!({"anthropic": {
            "type": "oauth",
            "access": "sk-ant-oat01-ACCESSTOKENVALUE0123456789",
            "refresh": "sk-ant-ort01-REFRESHTOKENVALUE0123456789",
            "expires": 1_700_000_000_000_i64,
            "email": "person@example.com",
        }});
        let spec = json!({ "credential": {
            "env": [["OPENCODE_KEY", "ocg-secret-key-value"]],
            "files": [[".pi/agent/auth.json", file.to_string()]],
        }});
        let env = vec![
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            (
                "ANTHROPIC_API_KEY".to_string(),
                "sk-ant-api03-hostkey".to_string(),
            ),
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://api.example.com".to_string(),
            ),
        ];
        let redactor = Redactor::new(&spec, &env);

        let line = format!(
            "{{\"type\":\"tool_result\",\"content\":\"{}\"}}",
            file.to_string().replace('"', "\\\"")
        );
        let clean = redactor.redact(&line);
        for secret in [
            "ACCESSTOKENVALUE",
            "REFRESHTOKENVALUE",
            "person@example.com",
        ] {
            assert!(!clean.contains(secret), "{secret} leaked: {clean}");
        }
        assert!(clean.contains("oauth"), "labels stay readable");
        assert!(clean.contains("1700000000000"), "numbers stay");

        let env_line = "OPENCODE_KEY=ocg-secret-key-value ANTHROPIC_API_KEY=sk-ant-api03-hostkey";
        let clean = redactor.redact(env_line);
        assert!(!clean.contains("ocg-secret-key-value"));
        assert!(!clean.contains("sk-ant-api03-hostkey"));
        assert!(redactor
            .redact("base https://api.example.com")
            .contains("https://api.example.com"));
    }

    #[test]
    fn a_token_piece_is_redacted_on_its_own() {
        let spec = json!({ "credential": {
            "env": [["ANTHROPIC_OAUTH_TOKEN", "prefix.AAAAAAAAAAAAAAAAAAAAAAAAAAAA.suffix"]],
            "files": [],
        }});
        let redactor = Redactor::new(&spec, &[]);
        assert_eq!(
            redactor.redact("payload AAAAAAAAAAAAAAAAAAAAAAAAAAAA here"),
            "payload [redacted] here"
        );
        assert_eq!(redactor.redact("nothing secret"), "nothing secret");
    }
}
