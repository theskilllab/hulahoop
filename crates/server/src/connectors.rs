//! Every credential the loop can act with, in one place.
//!
//! A GitHub app is an app id and a private key. An agent CLI authenticates with an
//! API key or with the file its own `login` command wrote. The payloads differ; the
//! shape does not: a provider, a kind, and a secret that is sealed before it is
//! stored. Keeping them in one table means the screen has one list, and adding the
//! next provider is a match arm rather than a migration.

use hulahoop_core::Adapter;

pub const GITHUB: &str = "github";
pub const CLAUDE_CODE: &str = "claude_code";
pub const PI: &str = "pi";

/// Every provider the screen offers, stored or not: "available" and "connected"
/// are two different answers, and only a complete list can show the first.
pub const ALL: [&str; 3] = [GITHUB, CLAUDE_CODE, PI];

/// What the secret is. The kind decides which field the payload carries.
pub const KIND_GITHUB_APP: &str = "github_app";
pub const KIND_API_KEY: &str = "api_key";
pub const KIND_SUBSCRIPTION: &str = "subscription";

/// The connector a run of this adapter authenticates with. `None` for the fake
/// adapter, which talks to a script and needs nothing.
pub fn for_adapter(adapter: Adapter) -> Option<&'static str> {
    match adapter {
        Adapter::Pi => Some(PI),
        Adapter::ClaudeCode => Some(CLAUDE_CODE),
        Adapter::Fake => None,
    }
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
    fn an_adapter_names_its_connector() {
        assert_eq!(for_adapter(Adapter::Pi), Some(PI));
        assert_eq!(for_adapter(Adapter::ClaudeCode), Some(CLAUDE_CODE));
        assert_eq!(for_adapter(Adapter::Fake), None);
    }

    #[test]
    fn every_agent_connector_has_a_home_for_its_secret() {
        for provider in [CLAUDE_CODE, PI] {
            assert!(
                api_key_env(provider).is_some() || credentials_file(provider).is_some(),
                "{provider} has nowhere to put a credential"
            );
            assert!(ALL.contains(&provider));
        }
    }

    #[test]
    fn github_takes_neither_a_key_nor_a_file() {
        assert!(api_key_env(GITHUB).is_none());
        assert!(credentials_file(GITHUB).is_none());
    }
}
