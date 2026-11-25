//! Every credential the loop can act with, in one place.
//!
//! An *entry* is one thing a person connects: a GitHub app, a Claude subscription,
//! a Claude API key, a Codex subscription, an OpenCode Go key, a Pi login. Two
//! entries can share a provider — Claude accepts a plan login or a billed key, and
//! they are separate rows because they are separate purchases. Keeping them in one
//! table means the screen has one list, and adding the next entry is a match arm
//! rather than a migration.

use hulahoop_core::credential::{
    claude_credentials_from_pi, pi_entry_from_claude, Refresh, CLAUDE_CREDENTIALS_FILE,
    FORMAT_CLAUDE, FORMAT_PI, PI_AUTH_FILE,
};
use hulahoop_core::Adapter;
use serde_json::{json, Value};
use uuid::Uuid;

pub const GITHUB: &str = "github";
pub const CLAUDE_CODE: &str = "claude_code";
pub const OPENAI_CODEX: &str = "openai_codex";
pub const OPENCODE_GO: &str = "opencode_go";
pub const PI: &str = "pi";

/// What the secret is. The kind decides which field the payload carries.
pub const KIND_GITHUB_APP: &str = "github_app";
pub const KIND_API_KEY: &str = "api_key";
pub const KIND_SUBSCRIPTION: &str = "subscription";

/// One thing a person connects, and what it unlocks.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Stable id the screen and the API use: `claude-api`, not `claude_code/api_key`.
    pub id: &'static str,
    /// Which CLI or service the secret is for. Rows are keyed by provider and
    /// kind, so existing rows keep these values for their secrets to keep working.
    pub provider: &'static str,
    /// What the secret is, which decides how it travels to a run.
    pub kind: &'static str,
    /// The heading this entry sits under.
    pub group: &'static str,
    pub name: &'static str,
    /// An icon name the desktop knows.
    pub icon: &'static str,
    pub desc: &'static str,
    /// The line under the description: what this makes possible.
    pub enables: &'static str,
    /// The key of this entry in Pi's `auth.json`, for entries that serve one
    /// provider's models in Pi. The Pi login is a whole file, so it has none.
    pub pi_provider: Option<&'static str>,
}

pub const GROUP_SOURCE: &str = "Source control";
pub const GROUP_AGENT: &str = "Agent access";

/// Every entry the screen offers, connected or not: "available" and "connected"
/// are two different answers, and only a complete list can show the first.
pub const ENTRIES: [Entry; 6] = [
    Entry {
        id: "github",
        provider: GITHUB,
        kind: KIND_GITHUB_APP,
        group: GROUP_SOURCE,
        name: "GitHub",
        icon: "pr",
        desc: "Connect with a GitHub App or a personal access token.",
        enables: "Pull requests and repositories",
        pi_provider: None,
    },
    Entry {
        id: "claude-sub",
        provider: CLAUDE_CODE,
        kind: KIND_SUBSCRIPTION,
        group: GROUP_AGENT,
        name: "Claude subscription",
        icon: "bot",
        desc: "Sign in with your Claude plan, or paste a token from `claude setup-token`.",
        enables: "Claude models in Claude Code and Pi",
        pi_provider: Some("anthropic"),
    },
    Entry {
        id: "claude-api",
        provider: CLAUDE_CODE,
        kind: KIND_API_KEY,
        group: GROUP_AGENT,
        name: "Claude API key",
        icon: "key",
        desc: "Pay per token with an Anthropic API key.",
        enables: "Claude models in Claude Code and Pi",
        pi_provider: Some("anthropic"),
    },
    Entry {
        id: "codex-sub",
        provider: OPENAI_CODEX,
        kind: KIND_SUBSCRIPTION,
        group: GROUP_AGENT,
        name: "Codex subscription",
        icon: "bot",
        desc: "Sign in with your ChatGPT plan to use Codex models.",
        enables: "OpenAI Codex models in Pi",
        pi_provider: Some("openai-codex"),
    },
    Entry {
        id: "opencode-go",
        provider: OPENCODE_GO,
        kind: KIND_API_KEY,
        group: GROUP_AGENT,
        name: "OpenCode Go key",
        icon: "key",
        desc: "Paste the API key from your OpenCode Go plan.",
        enables: "OpenCode Go models in Pi",
        pi_provider: Some("opencode-go"),
    },
    Entry {
        id: "pi",
        provider: PI,
        kind: KIND_SUBSCRIPTION,
        group: GROUP_AGENT,
        name: "Pi login",
        icon: "bot",
        desc: "Paste a Pi auth.json for providers without their own connector.",
        enables: "Enables Pi",
        pi_provider: None,
    },
];

/// One plan a subscription entry can be on. The fee only pre-fills Cost
/// control's plan; the Connectors screen shows names, never prices.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub id: &'static str,
    pub name: &'static str,
    /// Dollars a month, when the provider publishes one price for the plan.
    pub fee: Option<f64>,
}

const fn plan(id: &'static str, name: &'static str, fee: Option<f64>) -> Plan {
    Plan { id, name, fee }
}

const CLAUDE_PLANS: [Plan; 5] = [
    plan("pro", "Pro", Some(20.0)),
    plan("max", "Max", Some(100.0)),
    plan("ultra", "Ultra", Some(200.0)),
    plan("team", "Team", None),
    plan("enterprise", "Enterprise", None),
];

const CODEX_PLANS: [Plan; 4] = [
    plan("plus", "Plus", Some(20.0)),
    plan("pro", "Pro", Some(100.0)),
    plan("pro-max", "Pro (higher tier)", Some(200.0)),
    plan("business", "Business", None),
];

const OPENCODE_GO_PLANS: [Plan; 3] = [
    plan("go", "Go", Some(10.0)),
    plan("go-plus", "Go Plus", Some(40.0)),
    plan("go-teams", "Go Teams", Some(20.0)),
];

/// The plans a person can say they are on. Only subscriptions have plans; a
/// billed key is paid per token.
pub fn plans(entry_id: &str) -> &'static [Plan] {
    match entry_id {
        "claude-sub" => &CLAUDE_PLANS,
        "codex-sub" => &CODEX_PLANS,
        "opencode-go" => &OPENCODE_GO_PLANS,
        _ => &[],
    }
}

/// The entry a screen id names.
pub fn entry(id: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.id == id)
}

/// The entry a stored row belongs to.
pub fn find(provider: &str, kind: &str) -> Option<&'static Entry> {
    ENTRIES
        .iter()
        .find(|entry| entry.provider == provider && entry.kind == kind)
}

/// The entries that can power an adapter — what an agent picks from. Pi reads
/// every per-provider entry, because it builds its own `auth.json` from them.
pub fn for_adapter(adapter: Adapter) -> Vec<&'static Entry> {
    ENTRIES
        .iter()
        .filter(|entry| match adapter {
            Adapter::Pi => entry.provider == PI || entry.pi_provider.is_some(),
            Adapter::ClaudeCode => entry.provider == CLAUDE_CODE,
            Adapter::Fake => false,
        })
        .collect()
}

/// The harnesses an entry can power, by adapter name: every adapter whose
/// [`for_adapter`] lists it.
pub fn harnesses(entry_id: &str) -> Vec<&'static str> {
    [Adapter::ClaudeCode, Adapter::Pi]
        .into_iter()
        .filter(|adapter| {
            for_adapter(*adapter)
                .iter()
                .any(|entry| entry.id == entry_id)
        })
        .map(Adapter::as_str)
        .collect()
}

/// The provider a Pi model id names: the part before the `/`.
pub fn pi_provider_of(model: &str) -> Option<&str> {
    model
        .split_once('/')
        .map(|(provider, _)| provider)
        .filter(|provider| !provider.is_empty())
}

/// The per-provider entries for one Pi provider, in the order a run tries them:
/// a subscription before a billed key, as the entries are listed.
pub fn for_pi_provider(provider: &str) -> Vec<&'static Entry> {
    ENTRIES
        .iter()
        .filter(|entry| entry.pi_provider == Some(provider))
        .collect()
}

/// The environment variable an API key travels in. A subscription does not use it:
/// the CLI reads its own file, which is why the two kinds exist.
pub fn api_key_env(provider: &str) -> Option<&'static str> {
    match provider {
        CLAUDE_CODE => Some("ANTHROPIC_API_KEY"),
        PI => Some("PI_API_KEY"),
        _ => None,
    }
}

/// Where the CLI expects its own credentials file, relative to `HOME`. The runner
/// writes what was pasted there, into the run's scratch home, never the user's.
pub fn credentials_file(provider: &str) -> Option<&'static str> {
    match provider {
        CLAUDE_CODE => Some(CLAUDE_CREDENTIALS_FILE),
        PI => Some(PI_AUTH_FILE),
        _ => None,
    }
}

/// The opened secret fields of one stored row. Each is optional because a row
/// carries only the field its last connect wrote.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub row: Uuid,
    pub secret: Option<String>,
    pub token: Option<String>,
    pub contents: Option<String>,
    /// A Pi `auth.json` entry for this provider, as JSON text.
    pub oauth: Option<String>,
}

impl Secrets {
    fn oauth_entry(&self) -> Option<Value> {
        let entry: Value = serde_json::from_str(self.oauth.as_deref()?).ok()?;
        hulahoop_core::credential::is_oauth(&entry).then_some(entry)
    }

    /// The pasted Claude Code session file as a Pi entry.
    fn contents_entry(&self) -> Option<Value> {
        let contents: Value = serde_json::from_str(self.contents.as_deref()?).ok()?;
        pi_entry_from_claude(&contents)
    }

    /// The OAuth login a run of this row carries, as a Pi entry: none when a
    /// key or a setup token wins inside the row, which refresh nothing.
    pub fn refreshing_entry(&self) -> Option<Value> {
        if self.secret.is_some() || self.token.is_some() {
            return None;
        }
        self.oauth_entry().or_else(|| self.contents_entry())
    }

    /// The session file Claude Code reads. A pasted file stays as it was
    /// pasted — its plan and rate-limit fields included — with the tokens of a
    /// login refreshed since laid over its own; a sign-in with no pasted file
    /// is converted whole.
    fn claude_session_file(&self) -> Option<String> {
        let pasted = self
            .contents
            .as_deref()
            .and_then(|contents| serde_json::from_str::<Value>(contents).ok())
            .filter(|file| file["claudeAiOauth"].is_object());
        match (pasted, self.oauth_entry()) {
            (Some(mut file), Some(entry)) => {
                let session = &mut file["claudeAiOauth"];
                let pasted_expires = session["expiresAt"].as_f64();
                let newer = match (pasted_expires, entry["expires"].as_f64()) {
                    (Some(pasted), Some(refreshed)) => refreshed >= pasted,
                    (_, refreshed) => refreshed.is_some(),
                };
                if newer {
                    session["accessToken"] = entry["access"].clone();
                    session["refreshToken"] = entry["refresh"].clone();
                    session["expiresAt"] = entry["expires"].clone();
                }
                Some(file.to_string())
            }
            (None, Some(entry)) => Some(claude_credentials_from_pi(&entry).to_string()),
            (_, None) => self.contents.clone(),
        }
    }
}

/// The fields a stored OAuth login may carry. Anything else in an entry a run
/// hands back is dropped: the agent controls the file it came from.
pub const OAUTH_FIELDS: [&str; 7] = [
    "type",
    "access",
    "refresh",
    "expires",
    "accountId",
    "email",
    "scopes",
];

/// The longest an access token is believed to last. A handed-back login that
/// claims more is not a refresh.
pub const MAX_OAUTH_LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// An OAuth login a run (or a central refresh) produced, when it has the
/// shape of one and expires in the future but within [`MAX_OAUTH_LIFETIME_MS`]:
/// `{type: "oauth", access, refresh, expires, accountId?, email?, scopes?}`,
/// cut down to exactly those fields.
pub fn checked_oauth(entry: &Value, now_ms: i64) -> Option<serde_json::Map<String, Value>> {
    let object = entry.as_object()?;
    let text = |field: &str| object.get(field).and_then(Value::as_str);
    if text("type") != Some("oauth")
        || text("access").is_none_or(str::is_empty)
        || text("refresh").is_none_or(str::is_empty)
    {
        return None;
    }
    let expires = object.get("expires").and_then(Value::as_f64)?;
    if expires <= now_ms as f64 || expires >= (now_ms + MAX_OAUTH_LIFETIME_MS) as f64 {
        return None;
    }
    for optional in ["accountId", "email"] {
        if object.get(optional).is_some_and(|value| !value.is_string()) {
            return None;
        }
    }
    if let Some(scopes) = object.get("scopes") {
        if !scopes
            .as_array()
            .is_some_and(|scopes| scopes.iter().all(Value::is_string))
        {
            return None;
        }
    }
    Some(
        object
            .iter()
            .filter(|(field, _)| OAUTH_FIELDS.contains(&field.as_str()))
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect(),
    )
}

/// A refreshed login laid over the stored one, so a field the refresh did not
/// repeat (an account id, an email) survives. Only [`OAUTH_FIELDS`] are kept.
pub fn merge_oauth(stored: Option<&Value>, refreshed: serde_json::Map<String, Value>) -> Value {
    let mut merged: serde_json::Map<String, Value> = stored
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(field, _)| OAUTH_FIELDS.contains(&field.as_str()))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect();
    merged.extend(refreshed);
    Value::Object(merged)
}

/// A credential as the runner needs it: environment for a key, files for a
/// login. Both empty means nothing is stored and the run uses the host login.
#[derive(Debug, Clone, Default)]
pub struct Credential {
    pub env: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
    /// The entry the run started with; `None` when nothing stored applied.
    pub entry: Option<&'static Entry>,
    /// Which file to hand back after the run, when the login is one the
    /// harness refreshes itself.
    pub refresh: Option<Refresh>,
}

impl Credential {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.files.is_empty()
    }

    /// What a run records as its credential: the entry's name, or `host login`
    /// when the run falls back to the machine's own login.
    pub fn source(&self) -> &'static str {
        match self.entry {
            Some(entry) if !self.is_empty() => entry.name,
            _ => "host login",
        }
    }

    /// How a run's cost is paid: a billed key is cash, a plan is not. The Pi
    /// login may hold either, so it stays unknown, as it always has.
    pub fn billing(&self) -> &'static str {
        let Some(entry) = self.entry.filter(|_| !self.is_empty()) else {
            return "unknown";
        };
        if entry.provider == PI {
            let keyed = self.env.iter().any(|(name, _)| name == "PI_API_KEY");
            return if keyed { "api" } else { "unknown" };
        }
        // OpenCode Go is a monthly plan even though it connects with a key.
        if entry.kind == KIND_API_KEY && entry.provider != OPENCODE_GO {
            "api"
        } else {
            "subscription"
        }
    }

    /// The `credential` object of a runner spec. The refresh descriptor names
    /// files and rows, never a secret.
    pub fn to_spec(&self) -> Value {
        json!({
            "env": self.env,
            "files": self.files,
            "refresh": self.refresh,
        })
    }
}

/// What a Claude Code run gets from one row: a key, a long-lived token, or a
/// session file, in that order of preference within the row.
pub fn claude_code_credential(entry: &'static Entry, secrets: &Secrets) -> Credential {
    let mut credential = Credential {
        entry: Some(entry),
        ..Credential::default()
    };
    if let Some(secret) = &secrets.secret {
        credential
            .env
            .push(("ANTHROPIC_API_KEY".to_string(), secret.clone()));
    } else if let Some(token) = &secrets.token {
        // The setup token lasts a year and refreshes nothing, so it wins.
        credential
            .env
            .push(("CLAUDE_CODE_OAUTH_TOKEN".to_string(), token.clone()));
    } else if let Some(file) = secrets.claude_session_file() {
        credential
            .files
            .push((CLAUDE_CREDENTIALS_FILE.to_string(), file));
        credential.refresh = Some(Refresh {
            connector: entry.id.to_string(),
            row: secrets.row,
            file: CLAUDE_CREDENTIALS_FILE.to_string(),
            key: None,
            format: FORMAT_CLAUDE.to_string(),
        });
    }
    credential
}

/// What a Pi run gets from one per-provider row: an `auth.json` holding that
/// provider's entry and nothing else, or the setup token in the environment.
pub fn pi_credential(entry: &'static Entry, secrets: &Secrets) -> Credential {
    let mut credential = Credential {
        entry: Some(entry),
        ..Credential::default()
    };
    let Some(provider) = entry.pi_provider else {
        return credential;
    };
    let (login, refreshes) = if let Some(key) = &secrets.secret {
        (Some(json!({ "type": "api_key", "key": key })), false)
    } else if let Some(oauth) = secrets.oauth_entry() {
        (Some(oauth), true)
    } else if let Some(token) = &secrets.token {
        // Pi reads a Claude setup token from the environment; no file needed.
        credential
            .env
            .push(("ANTHROPIC_OAUTH_TOKEN".to_string(), token.clone()));
        (None, false)
    } else {
        (secrets.contents_entry(), true)
    };
    if let Some(login) = login {
        let file = json!({ provider: login }).to_string();
        credential.files.push((PI_AUTH_FILE.to_string(), file));
        if refreshes {
            credential.refresh = Some(Refresh {
                connector: entry.id.to_string(),
                row: secrets.row,
                file: PI_AUTH_FILE.to_string(),
                key: Some(provider.to_string()),
                format: FORMAT_PI.to_string(),
            });
        }
    }
    credential
}

/// What a run gets from the Pi login row: the pasted file as it is.
pub fn pi_login_credential(entry: &'static Entry, secrets: &Secrets) -> Credential {
    let mut credential = Credential {
        entry: Some(entry),
        ..Credential::default()
    };
    if let Some(secret) = &secrets.secret {
        credential
            .env
            .push(("PI_API_KEY".to_string(), secret.clone()));
    }
    if let Some(contents) = &secrets.contents {
        credential
            .files
            .push((PI_AUTH_FILE.to_string(), contents.clone()));
    }
    credential
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_has_a_unique_id_and_a_known_group() {
        let mut ids = std::collections::HashSet::new();
        for entry in ENTRIES {
            assert!(ids.insert(entry.id), "duplicate id {}", entry.id);
            assert!(
                entry.group == GROUP_SOURCE || entry.group == GROUP_AGENT,
                "{} has an unknown group",
                entry.id
            );
        }
    }

    #[test]
    fn claude_offers_two_ways_to_pay() {
        let kinds: Vec<_> = for_adapter(Adapter::ClaudeCode)
            .iter()
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(kinds, vec![KIND_SUBSCRIPTION, KIND_API_KEY]);
    }

    #[test]
    fn each_entry_names_the_harnesses_it_powers() {
        assert_eq!(harnesses("claude-sub"), ["claude_code", "pi"]);
        assert_eq!(harnesses("claude-api"), ["claude_code", "pi"]);
        assert_eq!(harnesses("codex-sub"), ["pi"]);
        assert_eq!(harnesses("opencode-go"), ["pi"]);
        assert_eq!(harnesses("pi"), ["pi"]);
        assert!(harnesses("github").is_empty());
    }

    #[test]
    fn a_handed_back_login_must_look_like_one() {
        let now = 1_000_000_000_000_i64;
        let good = json!({"type": "oauth", "access": "a", "refresh": "r", "expires": now + 3_600_000,
                          "accountId": "acc", "planted": "x"});
        let checked = checked_oauth(&good, now).expect("a login");
        assert!(
            checked.get("planted").is_none(),
            "unknown fields are dropped"
        );
        assert_eq!(checked["accountId"], "acc");
        for bad in [
            json!({"type": "api_key", "access": "a", "refresh": "r", "expires": now + 1}),
            json!({"type": "oauth", "access": "", "refresh": "r", "expires": now + 1}),
            json!({"type": "oauth", "access": "a", "expires": now + 1}),
            json!({"type": "oauth", "access": "a", "refresh": "r", "expires": "soon"}),
            json!({"type": "oauth", "access": "a", "refresh": "r", "expires": now - 1}),
            json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 9e15}),
            json!({"type": "oauth", "access": "a", "refresh": "r", "expires": now + 1, "email": 5}),
            json!({"type": "oauth", "access": "a", "refresh": "r", "expires": now + 1, "scopes": "all"}),
        ] {
            assert!(checked_oauth(&bad, now).is_none(), "{bad}");
        }
        let stored = json!({"type": "oauth", "access": "old", "refresh": "old", "expires": 1,
                            "email": "me@example.com", "extra": true});
        let merged = merge_oauth(Some(&stored), checked);
        assert_eq!(merged["access"], "a");
        assert_eq!(merged["email"], "me@example.com");
        assert!(merged.get("extra").is_none());
    }

    /// A refreshed login lands in a pasted session file without losing the
    /// file's own fields; a sign-in alone is converted whole.
    #[test]
    fn a_pasted_session_file_keeps_its_fields() {
        let pasted = json!({"claudeAiOauth": {"accessToken": "old", "refreshToken": "old", "expiresAt": 1,
                                              "scopes": ["user:inference"], "subscriptionType": "max",
                                              "rateLimitTier": "tier"}});
        let entry = super::entry("claude-sub").unwrap();
        let secrets = Secrets {
            contents: Some(pasted.to_string()),
            oauth: Some(
                json!({"type": "oauth", "access": "new", "refresh": "new2", "expires": 5})
                    .to_string(),
            ),
            ..Secrets::default()
        };
        let credential = claude_code_credential(entry, &secrets);
        let file: Value = serde_json::from_str(&credential.files[0].1).unwrap();
        assert_eq!(file["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(file["claudeAiOauth"]["refreshToken"], "new2");
        assert_eq!(file["claudeAiOauth"]["subscriptionType"], "max");
        assert_eq!(file["claudeAiOauth"]["rateLimitTier"], "tier");

        let pasted_only = Secrets {
            contents: Some(pasted.to_string()),
            ..Secrets::default()
        };
        let credential = claude_code_credential(entry, &pasted_only);
        assert_eq!(credential.files[0].1, pasted.to_string());
    }

    #[test]
    fn an_entry_is_found_by_id_and_by_row() {
        assert_eq!(
            entry("claude-api").map(|entry| entry.kind),
            Some(KIND_API_KEY)
        );
        assert_eq!(
            find(CLAUDE_CODE, KIND_API_KEY).map(|entry| entry.id),
            Some("claude-api")
        );
        assert!(entry("codex").is_none());
    }
}
