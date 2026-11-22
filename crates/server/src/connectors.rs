//! Every credential the loop can act with, in one place.
//!
//! An *entry* is one thing a person connects: a GitHub app, a Claude subscription,
//! a Claude API key, a Pi login. Two entries can share a provider — Claude Code
//! accepts a plan login or a billed key, and they are separate rows because they
//! are separate purchases. Keeping them in one table means the screen has one
//! list, and adding the next entry is a match arm rather than a migration.

use hulahoop_core::Adapter;

pub const GITHUB: &str = "github";
pub const CLAUDE_CODE: &str = "claude_code";
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
    /// Which CLI or service the secret is for.
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
}

pub const GROUP_SOURCE: &str = "Source control";
pub const GROUP_AGENT: &str = "Agent access";

/// Every entry the screen offers, connected or not: "available" and "connected"
/// are two different answers, and only a complete list can show the first.
pub const ENTRIES: [Entry; 4] = [
    Entry {
        id: "github",
        provider: GITHUB,
        kind: KIND_GITHUB_APP,
        group: GROUP_SOURCE,
        name: "GitHub",
        icon: "pr",
        desc: "Connect with a GitHub App or a personal access token.",
        enables: "Pull requests and repositories",
    },
    Entry {
        id: "claude-sub",
        provider: CLAUDE_CODE,
        kind: KIND_SUBSCRIPTION,
        group: GROUP_AGENT,
        name: "Claude Code subscription",
        icon: "bot",
        desc: "Sign in with your Claude Pro or Max plan.",
        enables: "Enables Claude Code",
    },
    Entry {
        id: "claude-api",
        provider: CLAUDE_CODE,
        kind: KIND_API_KEY,
        group: GROUP_AGENT,
        name: "Claude API key",
        icon: "key",
        desc: "Pay per token with an Anthropic API key.",
        enables: "Enables Claude Code",
    },
    Entry {
        id: "pi",
        provider: PI,
        kind: KIND_SUBSCRIPTION,
        group: GROUP_AGENT,
        name: "Pi login",
        icon: "bot",
        desc: "Sign in to a provider through Pi, for example OpenCode Go.",
        enables: "Enables Pi",
    },
];

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

/// The entries that can power an adapter — what an agent picks from.
pub fn for_adapter(adapter: Adapter) -> Vec<&'static Entry> {
    let provider = match adapter {
        Adapter::Pi => PI,
        Adapter::ClaudeCode => CLAUDE_CODE,
        Adapter::Fake => return Vec::new(),
    };
    ENTRIES
        .iter()
        .filter(|entry| entry.provider == provider)
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
        CLAUDE_CODE => Some(".claude/.credentials.json"),
        PI => Some(".pi/agent/auth.json"),
        _ => None,
    }
}

/// A credential as the runner needs it: environment for a key, files for a
/// subscription. Both empty means nothing is stored.
#[derive(Debug, Clone, Default)]
pub struct Credential {
    pub env: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
}

impl Credential {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.files.is_empty()
    }
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
