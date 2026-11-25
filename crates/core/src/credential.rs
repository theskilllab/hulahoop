//! The credential formats the server and the runner both read.
//!
//! The server builds a run's login files and the runner hands back what the
//! harness rewrote in them, so both sides must agree on the file names, on the
//! refresh descriptor, and on how a Claude Code session file maps to a Pi
//! `auth.json` entry. Keeping that here means the two halves cannot drift.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

/// Where Pi reads its logins, relative to `HOME`.
pub const PI_AUTH_FILE: &str = ".pi/agent/auth.json";
/// Where Claude Code reads its session login, relative to `HOME`.
pub const CLAUDE_CREDENTIALS_FILE: &str = ".claude/.credentials.json";

/// The refresh descriptor's format for a Pi `auth.json` entry.
pub const FORMAT_PI: &str = "pi";
/// The refresh descriptor's format for a Claude Code `.credentials.json`.
pub const FORMAT_CLAUDE: &str = "claude";

/// The scopes Pi asks Anthropic for. A Claude Code credentials file needs a scope
/// list, and an entry Pi wrote does not always carry one.
pub const CLAUDE_OAUTH_SCOPES: [&str; 3] = [
    "user:inference",
    "user:profile",
    "user:sessions:claude_code",
];

/// Which stored OAuth login a run's file came from. A harness refreshes an
/// expired token mid-run and writes the new one into the scratch home; the
/// runner uses this to find it and hand it back, so the next run starts with a
/// refresh token that still works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refresh {
    /// The connector entry id, e.g. `claude-sub`.
    pub connector: String,
    /// The stored row the run was given. A row replaced since (a new sign-in, a
    /// disconnect) has a new id, and a hand-back for the old one is ignored.
    pub row: Uuid,
    /// The file in the scratch home, relative to `HOME`.
    pub file: String,
    /// The provider key inside a Pi `auth.json`; none for Claude's file.
    pub key: Option<String>,
    /// `pi` or `claude`: how to read the entry out of `file`.
    pub format: String,
}

impl Refresh {
    /// The OAuth entry this descriptor names, read from a file's contents and
    /// given in Pi's entry format whatever the file's own format is.
    pub fn entry_in(&self, contents: &str) -> Option<Value> {
        let file: Value = serde_json::from_str(contents).ok()?;
        let entry = match self.format.as_str() {
            FORMAT_CLAUDE => pi_entry_from_claude(&file)?,
            _ => file.get(self.key.as_deref()?)?.clone(),
        };
        is_oauth(&entry).then_some(entry)
    }
}

/// Whether a Pi `auth.json` entry is an OAuth login with a token in it.
pub fn is_oauth(entry: &Value) -> bool {
    entry["type"] == "oauth" && entry["access"].as_str().is_some_and(|a| !a.is_empty())
}

/// When an OAuth entry's access token expires, in milliseconds since the epoch.
pub fn oauth_expires(entry: &Value) -> Option<i64> {
    entry["expires"]
        .as_i64()
        .or_else(|| entry["expires"].as_f64().map(|expires| expires as i64))
}

/// A Claude Code session file (`{"claudeAiOauth": {...}}`) as a Pi entry.
/// The scopes travel along so the reverse conversion can keep them.
pub fn pi_entry_from_claude(credentials: &Value) -> Option<Value> {
    let oauth = credentials.get("claudeAiOauth")?;
    let access = oauth["accessToken"].as_str()?;
    let mut entry = json!({
        "type": "oauth",
        "access": access,
        "refresh": oauth["refreshToken"],
        "expires": oauth["expiresAt"],
    });
    if let Some(scopes) = oauth.get("scopes").filter(|scopes| scopes.is_array()) {
        entry["scopes"] = scopes.clone();
    }
    Some(entry)
}

/// A Pi OAuth entry as the session file Claude Code reads. One Claude
/// subscription login serves both harnesses this way.
pub fn claude_credentials_from_pi(entry: &Value) -> Value {
    let scopes = entry
        .get("scopes")
        .filter(|scopes| scopes.is_array())
        .cloned()
        .unwrap_or_else(|| json!(CLAUDE_OAUTH_SCOPES));
    json!({
        "claudeAiOauth": {
            "accessToken": entry["access"],
            "refreshToken": entry["refresh"],
            "expiresAt": entry["expires"],
            "scopes": scopes,
            "subscriptionType": Value::Null,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pi_entry_becomes_a_claude_session_file_with_default_scopes() {
        let entry = json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 5});
        let file = claude_credentials_from_pi(&entry);
        assert_eq!(file["claudeAiOauth"]["accessToken"], "a");
        assert_eq!(file["claudeAiOauth"]["refreshToken"], "r");
        assert_eq!(file["claudeAiOauth"]["expiresAt"], 5);
        assert_eq!(file["claudeAiOauth"]["scopes"], json!(CLAUDE_OAUTH_SCOPES));
        assert!(file["claudeAiOauth"]["subscriptionType"].is_null());
        assert_eq!(pi_entry_from_claude(&file).unwrap()["access"], "a");
    }

    #[test]
    fn a_refresh_reads_its_entry_in_either_format() {
        let row = Uuid::now_v7();
        let pi = Refresh {
            connector: "codex-sub".into(),
            row,
            file: PI_AUTH_FILE.into(),
            key: Some("openai-codex".into()),
            format: FORMAT_PI.into(),
        };
        let contents =
            r#"{"openai-codex":{"type":"oauth","access":"x","refresh":"y","expires":9}}"#;
        assert_eq!(pi.entry_in(contents).unwrap()["expires"], 9);
        assert!(pi
            .entry_in(r#"{"openai-codex":{"type":"api_key","key":"k"}}"#)
            .is_none());
        let claude = Refresh {
            connector: "claude-sub".into(),
            row,
            file: CLAUDE_CREDENTIALS_FILE.into(),
            key: None,
            format: FORMAT_CLAUDE.into(),
        };
        let contents = r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":7}}"#;
        assert_eq!(claude.entry_in(contents).unwrap()["refresh"], "r");
        assert!(claude.entry_in("not json").is_none());
    }
}
