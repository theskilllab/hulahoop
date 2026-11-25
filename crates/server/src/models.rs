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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

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
    store_models(&home, &signed_in_providers(&home))
}

/// Pi's models for a model dropdown: the host's own logins, plus every provider
/// a connected entry serves. A provider the host has never signed in to has no
/// models in the host's store, so those come from Pi's built-in catalogue.
pub async fn pi_catalogue(pi: &Path, connected: &[&str]) -> Vec<ModelOption> {
    let Some(home) = pi_home() else {
        return cached_catalogue_models(pi, connected).await;
    };
    let mut available = signed_in_providers(&home);
    available.extend(connected.iter().map(|provider| provider.to_string()));
    let mut out = store_models(&home, &available);
    let missing: Vec<&str> = connected
        .iter()
        .copied()
        .filter(|provider| !out.iter().any(|model| model.provider == *provider))
        .collect();
    if !missing.is_empty() {
        out.extend(cached_catalogue_models(pi, &missing).await);
    }
    out
}

/// The models in a Pi home's `models-store.json`, for the given providers. An
/// empty provider list means we could not tell, and shows everything.
fn store_models(home: &Path, available: &[String]) -> Vec<ModelOption> {
    let store = std::fs::read_to_string(home.join("models-store.json")).unwrap_or_default();
    let Ok(store) = serde_json::from_str::<serde_json::Value>(&store) else {
        return Vec::new();
    };
    let Some(providers) = store.as_object() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (provider, entry) in providers {
        // An empty sign-in list means we could not tell; show everything rather
        // than an empty dropdown.
        if !available.is_empty() && !available.iter().any(|name| name == provider) {
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

/// How long a listing from Pi is reused. Pi's built-in catalogue changes with
/// a Pi release, not between two opens of a model dropdown.
const CATALOGUE_TTL: Duration = Duration::from_secs(10 * 60);

type CatalogueKey = (PathBuf, Vec<String>);
type Catalogues = HashMap<CatalogueKey, (Instant, Vec<ModelOption>)>;

/// Listings by Pi binary and providers. The lock is held while Pi runs, so
/// screens asking at once share one start of Pi instead of one each.
static CATALOGUE_CACHE: LazyLock<tokio::sync::Mutex<Catalogues>> = LazyLock::new(Default::default);

/// [`catalogue_models`], reused for [`CATALOGUE_TTL`]: starting Node and Pi
/// on every models query held each request open for seconds.
async fn cached_catalogue_models(pi: &Path, providers: &[&str]) -> Vec<ModelOption> {
    let mut wanted: Vec<String> = providers
        .iter()
        .map(|provider| provider.to_string())
        .collect();
    wanted.sort();
    wanted.dedup();
    let key = (pi.to_path_buf(), wanted);
    let mut cache = CATALOGUE_CACHE.lock().await;
    if let Some((at, models)) = cache.get(&key) {
        if at.elapsed() < CATALOGUE_TTL {
            return models.clone();
        }
    }
    let models = catalogue_models(pi, providers).await;
    cache.retain(|_, (at, _)| at.elapsed() < CATALOGUE_TTL);
    cache.insert(key, (Instant::now(), models.clone()));
    models
}

/// Pi's built-in catalogue for some providers. `pi --list-models` lists only
/// the providers it holds a login for, so it runs offline in an empty home with
/// a placeholder login per provider — the placeholders authenticate nothing and
/// the host's own login is never read. The listing has no names or prices.
async fn catalogue_models(pi: &Path, providers: &[&str]) -> Vec<ModelOption> {
    let Ok(home) = tempdir() else {
        return Vec::new();
    };
    let listed = list_models(pi, &home, providers).await;
    let _ = std::fs::remove_dir_all(&home);
    let output = listed.unwrap_or_default();
    parse_model_list(&output, providers)
}

fn tempdir() -> std::io::Result<PathBuf> {
    let home = std::env::temp_dir().join(format!("hulahoop-pi-models-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(home.join(".pi/agent"))?;
    Ok(home)
}

async fn list_models(pi: &Path, home: &Path, providers: &[&str]) -> Option<String> {
    let mut auth = serde_json::Map::new();
    for provider in providers {
        // Pi lists a provider's models only for the login type it accepts, and
        // Codex accepts only an OAuth login.
        let placeholder = if *provider == "openai-codex" {
            serde_json::json!({"type": "oauth", "access": "placeholder", "refresh": "placeholder", "expires": 0})
        } else {
            serde_json::json!({"type": "api_key", "key": "placeholder"})
        };
        auth.insert(provider.to_string(), placeholder);
    }
    std::fs::write(
        home.join(".pi/agent/auth.json"),
        serde_json::Value::Object(auth).to_string(),
    )
    .ok()?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::process::Command::new(pi)
            .args(["--offline", "--list-models"])
            .env_clear()
            .env("HOME", home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("NO_COLOR", "1")
            .envs(crate::signin::pass_through_env())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Rows of `pi --list-models`: `provider model context max-out thinking images`.
fn parse_model_list(output: &str, providers: &[&str]) -> Vec<ModelOption> {
    output
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let provider = columns.next()?;
            let id = columns.next()?;
            if !providers.contains(&provider) {
                return None;
            }
            Some(ModelOption {
                id: format!("{provider}/{id}"),
                label: id.to_string(),
                provider: provider.to_string(),
                cost_in: None,
                cost_out: None,
                context_window: columns.next().and_then(parse_size),
            })
        })
        .collect()
}

/// `200K`, `1M`, `262.1K` as a token count.
fn parse_size(size: &str) -> Option<i64> {
    let (number, scale) = match size.chars().last()? {
        'K' => (&size[..size.len() - 1], 1_000.0),
        'M' => (&size[..size.len() - 1], 1_000_000.0),
        _ => (size, 1.0),
    };
    number
        .parse::<f64>()
        .ok()
        .map(|value| (value * scale).round() as i64)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_list_is_read_for_the_wanted_providers() {
        let output = "provider      model              context  max-out  thinking  images\n\
                      anthropic     claude-haiku-4-5   200K     64K      yes       yes\n\
                      openai-codex  gpt-5.1-codex      262.1K   128K     yes       no\n\
                      openrouter    some/model         1M       1M       no        no\n";
        let models = parse_model_list(output, &["openai-codex", "anthropic"]);
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            ["anthropic/claude-haiku-4-5", "openai-codex/gpt-5.1-codex"]
        );
        assert_eq!(models[0].context_window, Some(200_000));
        assert_eq!(models[1].context_window, Some(262_100));
        assert_eq!(models[1].provider, "openai-codex");
    }

    /// A provider only a connector serves is listed from Pi's catalogue, which
    /// a fake Pi stands in for here.
    #[tokio::test]
    async fn a_connected_provider_lists_models_from_pi() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hulahoop-models-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let pi = dir.join("pi");
        std::fs::write(
            &pi,
            "#!/bin/sh\ngrep -q opencode-go \"$HOME/.pi/agent/auth.json\" || exit 1\n\
             echo 'provider     model   context  max-out  thinking  images'\n\
             echo 'opencode-go  glm-5   200K     64K      yes       no'\n",
        )
        .unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let models = catalogue_models(&pi, &["opencode-go"]).await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "opencode-go/glm-5");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Asking again within the cache's lifetime, in any provider order, does
    /// not start Pi again.
    #[tokio::test]
    async fn a_listing_is_reused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hulahoop-models-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let pi = dir.join("pi");
        let count = dir.join("count");
        std::fs::write(
            &pi,
            format!(
                "#!/bin/sh\necho x >> '{}'\n\
                 echo 'opencode-go  glm-5   200K     64K      yes       no'\n\
                 echo 'openai-codex  gpt-5   200K     64K      yes       no'\n",
                count.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let first = cached_catalogue_models(&pi, &["opencode-go", "openai-codex"]).await;
        let (second, third) = tokio::join!(
            cached_catalogue_models(&pi, &["openai-codex", "opencode-go"]),
            cached_catalogue_models(&pi, &["opencode-go", "openai-codex"]),
        );
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        assert_eq!(third.len(), 2);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
