//! What a harness can run.
//!
//! The catalogue belongs to the harness, not to Hulahoop. pi keeps its store at
//! `~/.pi/agent/models-store.json` — providers, model ids, names and the prices a
//! cost report needs — and marks which providers are signed in in
//! `~/.pi/agent/auth.json`. Reading those two files means a model that pi learns
//! about appears in the app without a release, and a price that changes is read
//! fresh instead of being frozen into a constant.
//!
//! Claude Code is different: it ships aliases (`sonnet`, `opus`, `haiku`) that the
//! CLI resolves itself, so those are the honest options, not a model list.

use std::path::{Path, PathBuf};

use async_graphql::SimpleObject;

/// One choice in a model dropdown, with the numbers a cost estimate needs.
#[derive(Debug, Clone, SimpleObject)]
pub struct ModelOption {
    /// What to store on the agent: `provider/id` for pi, the alias for Claude.
    pub id: String,
    pub label: String,
    pub provider: String,
    /// Dollars per million input tokens, when the harness publishes one.
    pub cost_in: Option<f64>,
    pub cost_out: Option<f64>,
    pub context_window: Option<i64>,
}

fn pi_home() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".pi").join("agent"))
}

/// Every model a harness offers, in the order its owner lists them.
pub fn catalogue(harness: &str) -> Vec<ModelOption> {
    match harness {
        "pi" => pi_models(),
        "claude_code" => claude_models(),
        "fake" => vec![ModelOption {
            id: "fake".into(),
            label: "Fake (development)".into(),
            provider: "fake".into(),
            cost_in: None,
            cost_out: None,
            context_window: None,
        }],
        _ => Vec::new(),
    }
}

/// pi's store, filtered to the providers this machine is signed in to.
fn pi_models() -> Vec<ModelOption> {
    let Some(home) = pi_home() else {
        return Vec::new();
    };
    let store = std::fs::read_to_string(home.join("models-store.json")).unwrap_or_default();
    let Ok(store) = serde_json::from_str::<serde_json::Value>(&store) else {
        return Vec::new();
    };
    let signed_in = signed_in_providers(&home);
    let Some(providers) = store.as_object() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (provider, entry) in providers {
        // An empty sign-in list means we could not tell; show everything rather
        // than an empty dropdown.
        if !signed_in.is_empty() && !signed_in.iter().any(|name| name == provider) {
            continue;
        }
        let Some(models) = entry.get("models").and_then(|value| value.as_array()) else {
            continue;
        };
        for model in models {
            let Some(id) = model.get("id").and_then(|value| value.as_str()) else {
                continue;
            };
            let label = model
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or(id)
                .to_string();
            let cost = model.get("cost");
            out.push(ModelOption {
                id: format!("{provider}/{id}"),
                label,
                provider: provider.clone(),
                cost_in: cost
                    .and_then(|value| value.get("input"))
                    .and_then(|v| v.as_f64()),
                cost_out: cost
                    .and_then(|value| value.get("output"))
                    .and_then(|v| v.as_f64()),
                context_window: model.get("contextWindow").and_then(|v| v.as_i64()),
            });
        }
    }
    out
}

fn signed_in_providers(home: &Path) -> Vec<String> {
    let auth = std::fs::read_to_string(home.join("auth.json")).unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&auth) {
        Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn claude_models() -> Vec<ModelOption> {
    ["sonnet", "opus", "haiku"]
        .into_iter()
        .map(|alias| ModelOption {
            id: alias.into(),
            label: match alias {
                "sonnet" => "Sonnet".to_string(),
                "opus" => "Opus".to_string(),
                _ => "Haiku".to_string(),
            },
            provider: "claude_code".into(),
            cost_in: None,
            cost_out: None,
            context_window: None,
        })
        .collect()
}

/// Dollars per token for a model, when the catalogue publishes a price.
pub fn price_per_token(model: &str) -> Option<(f64, f64)> {
    let harness = if model.contains('/') {
        "pi"
    } else {
        "claude_code"
    };
    catalogue(harness)
        .into_iter()
        .find(|entry| entry.id == model)
        .and_then(|entry| entry.cost_in.zip(entry.cost_out))
        .map(|(input, output)| (input / 1_000_000.0, output / 1_000_000.0))
}
