//! GitHub app credentials: the token the loop mints for an app, and the one
//! question the connections list asks about a stored key — can it sign?
//!
//! A GitHub app authenticates as itself with a short-lived RS256 JWT signed by
//! the app's private key; GitHub swaps that for an installation token. Nothing
//! here talks to GitHub, so the loop mints and hands over the half it can prove
//! it owns: the signature only verifies against this app's own public key.

use anyhow::{anyhow, bail, Context, Result};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use sha2::Sha256;

pub const PROVIDER: &str = "github";

/// The stored app with its credentials decrypted for the moment they are used.
/// It never leaves this module's callers: the screen sees `provider`, `app_id`,
/// and `connected`, the runner sees a minted token.
pub struct StoredApp {
    pub app_id: String,
    pub private_key: String,
    /// GitHub's slug for the app, once `GET /app` has named it.
    pub slug: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Ten minutes is GitHub's ceiling for an app JWT; a token minted for a run is
/// used immediately, so being wrong about the clock is the only real risk.
const TOKEN_TTL_SECS: i64 = 600;

/// Parse either PEM form, because the screen accepts what the user copied out of
/// GitHub, and the two forms are one line apart.
fn parse_key(private_key: &str) -> Result<RsaPrivateKey> {
    RsaPrivateKey::from_pkcs1_pem(private_key)
        .or_else(|_| RsaPrivateKey::from_pkcs8_pem(private_key))
        .map_err(|error| anyhow!("this is not an RSA private key in PEM form: {error}"))
}

/// Whether the stored key can still produce a signature. Parsing is not enough:
/// the honest answer comes from signing something, which is what a token would
/// have to do anyway.
pub fn can_sign(private_key: &str) -> bool {
    match parse_key(private_key) {
        Ok(key) => sign(SigningKey::new(key), b"hulahoop-connection-probe").is_ok(),
        Err(_) => false,
    }
}

fn sign(signing_key: SigningKey<Sha256>, message: &[u8]) -> Result<rsa::pkcs1v15::Signature> {
    signing_key
        .try_sign(message)
        .map_err(|error| anyhow!("the key could not sign: {error}"))
}

/// The bearer token a role uses to act as the app: a JWT whose `iss` is the app
/// id and whose signature only the app's key can make.
pub fn mint_token(app_id: &str, private_key: &str) -> Result<String> {
    if app_id.trim().is_empty() {
        bail!("a github app needs an app id to identify itself");
    }
    let key = parse_key(private_key)?;
    let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
    let now = chrono::Utc::now().timestamp();
    let claims = serde_json::json!({
        "iat": now - 60,
        "exp": now + TOKEN_TTL_SECS,
        "iss": app_id,
    });
    let signing_input = format!(
        "{}.{}",
        base64url(serde_json::to_vec(&header)?.as_slice()),
        base64url(serde_json::to_vec(&claims)?.as_slice())
    );
    let signature = sign(SigningKey::new(key), signing_input.as_bytes())
        .context("minting a github app token")?;
    Ok(format!(
        "{signing_input}.{}",
        base64url(&signature.to_bytes())
    ))
}

/// The app's own record, which is how the slug behind the bot login is learned.
///
/// Asking GitHub beats deriving it from the name: the slug is what the name became
/// after collisions, and only the slug makes a commit show up as `<slug>[bot]`.
pub async fn fetch_slug(http: &reqwest::Client, jwt: &str) -> Result<String> {
    let response = http
        .get("https://api.github.com/app")
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(jwt)
        .send()
        .await
        .context("asking github which app this key belongs to")?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        bail!(
            "github did not accept the app id and key together ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    body["slug"]
        .as_str()
        .map(str::to_string)
        .context("the app response carried no slug")
}

/// A repository the installed app can reach, as GitHub describes it. Enough to
/// attach it to a project without a local checkout.
#[derive(Debug, Clone, serde::Serialize, async_graphql::SimpleObject)]
pub struct GithubRepo {
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub private: bool,
    pub archived: bool,
    pub default_branch: String,
    pub clone_url: String,
}

/// Everything the installation can reach, in one listing.
///
/// `GET /installation/repositories` needs the app JWT: the installation token is
/// already narrowed to one repository, so it cannot answer this question. The
/// endpoint needs `Metadata: read`, which every app has whether it asks or not.
pub async fn list_repositories(http: &reqwest::Client, jwt: &str) -> Result<Vec<GithubRepo>> {
    // A JWT can read the app, its installations and nothing else: the repository
    // list hangs off an installation token. So: installations, then their repos.
    let installations = installation_ids(http, jwt).await?;
    if installations.is_empty() {
        bail!(
            "the GitHub app is not installed anywhere yet, so it can reach no repositories. Install it on the account or organisation that holds them (Settings → GitHub Apps → Install)."
        );
    }
    let mut out: Vec<GithubRepo> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for installation in installations {
        let token = installation_token_by_id(http, jwt, installation).await?;
        for page in 1..=20 {
            let response = http
                .get("https://api.github.com/installation/repositories")
                .query(&[("per_page", "100"), ("page", &page.to_string())])
                .header("accept", "application/vnd.github+json")
                .header("user-agent", "hulahoop")
                .bearer_auth(&token)
                .send()
                .await
                .context("asking github which repositories the installation can reach")?;
            let status = response.status();
            let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
            if !status.is_success() {
                bail!(
                    "github would not list the installation's repositories ({status}): {}",
                    body["message"].as_str().unwrap_or("no reason given")
                );
            }
            let batch = body["repositories"].as_array().cloned().unwrap_or_default();
            let count = batch.len();
            for entry in batch {
                let full_name = entry["full_name"].as_str().unwrap_or_default().to_string();
                if full_name.is_empty() || !seen.insert(full_name.clone()) {
                    continue;
                }
                out.push(GithubRepo {
                    full_name,
                    name: entry["name"].as_str().unwrap_or_default().to_string(),
                    owner: entry["owner"]["login"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    private: entry["private"].as_bool().unwrap_or(false),
                    archived: entry["archived"].as_bool().unwrap_or(false),
                    default_branch: entry["default_branch"]
                        .as_str()
                        .unwrap_or("main")
                        .to_string(),
                    clone_url: entry["clone_url"].as_str().unwrap_or_default().to_string(),
                });
            }
            if count < 100 {
                break;
            }
        }
    }
    out.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    Ok(out)
}

/// The installations this app has. One per account or organisation it was installed
/// on, which is why a repository list has to be assembled from several tokens.
pub async fn installation_ids(http: &reqwest::Client, jwt: &str) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for page in 1..=10 {
        let response = http
            .get("https://api.github.com/app/installations")
            .query(&[("per_page", "100"), ("page", &page.to_string())])
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "hulahoop")
            .bearer_auth(jwt)
            .send()
            .await
            .context("asking github where this app is installed")?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
        if !status.is_success() {
            bail!(
                "github would not list the app's installations ({status}): {}",
                body["message"].as_str().unwrap_or("no reason given")
            );
        }
        let batch = body.as_array().cloned().unwrap_or_default();
        let count = batch.len();
        for entry in batch {
            if let Some(id) = entry["id"].as_i64() {
                ids.push(id);
            }
        }
        if count < 100 {
            break;
        }
    }
    Ok(ids)
}

pub async fn installation_token_by_id(
    http: &reqwest::Client,
    jwt: &str,
    installation: i64,
) -> Result<String> {
    let response = http
        .post(format!(
            "https://api.github.com/app/installations/{installation}/access_tokens"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(jwt)
        .send()
        .await
        .context("minting an installation token")?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        bail!(
            "github would not mint a token for installation {installation} ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    body["token"]
        .as_str()
        .map(str::to_string)
        .context("the token response carried no token")
}

/// `owner/repo` out of a remote URL, so a run can ask for a token scoped to it.
pub fn split_remote(remote: &str) -> Option<(String, String)> {
    let trimmed = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.strip_prefix("https://github.com/"))
        .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))?;
    let mut parts = path.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let name = parts.next()?.to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

/// A token that can clone and push one repository. Narrowed on purpose: a token
/// that reaches every repository the app was installed on is a token worth leaking.
pub async fn installation_token(
    http: &reqwest::Client,
    jwt: &str,
    owner: &str,
    repo: &str,
) -> Result<String> {
    let response = http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repo}/installation"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(jwt)
        .send()
        .await
        .context("asking github for the installation that covers this repository")?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        bail!(
            "github has no installation covering {owner}/{repo} ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    let installation = body["id"]
        .as_i64()
        .context("the installation response carried no id")?;
    let response = http
        .post(format!(
            "https://api.github.com/app/installations/{installation}/access_tokens"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(jwt)
        .send()
        .await
        .context("minting an installation token")?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        bail!(
            "github would not mint a token for {owner}/{repo} ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    body["token"]
        .as_str()
        .map(str::to_string)
        .context("the token response carried no token")
}

/// base64url without padding, which is what a JWT segment is.
fn base64url(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 0x3f] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = include_str!("../tests/fixtures/github_app_key.pem");

    #[test]
    fn a_complete_key_can_sign() {
        assert!(can_sign(KEY));
    }

    #[test]
    fn a_truncated_key_cannot_sign() {
        let mut lines = KEY.lines();
        let begin = lines.next().unwrap();
        let first = lines.next().unwrap();
        assert!(!can_sign(&format!(
            "{begin}\n{first}\n-----END RSA PRIVATE KEY-----"
        )));
    }

    #[test]
    fn a_token_has_three_segments_and_starts_with_a_header() {
        let token = mint_token("271828182845", KEY).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0].starts_with("eyJ"));
        assert!(parts[2].len() > 16);
    }

    #[test]
    fn base64url_matches_the_known_alphabet_and_drops_padding() {
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
    }

    #[test]
    fn a_key_that_is_not_rsa_is_refused() {
        assert!(mint_token(
            "1",
            "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----"
        )
        .is_err());
    }
}

/// What GitHub says about a pull request's merge.
#[derive(Debug, Clone)]
pub struct MergeState {
    pub merged: bool,
    pub merge_commit_sha: Option<String>,
    pub merged_at: Option<String>,
    pub state: String,
}

/// The pull request's own account of whether it was merged, and into what.
///
/// The revision a merge produced is a fact GitHub holds; asking the person who
/// pressed the button to paste it back invites a typo and a record that lies.
pub async fn pull_request(
    http: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: i64,
) -> Result<MergeState> {
    let response = http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repo}/pulls/{number}"
        ))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "hulahoop")
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("asking github about {owner}/{repo}#{number}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        bail!(
            "github would not describe {owner}/{repo}#{number} ({status}): {}",
            body["message"].as_str().unwrap_or("no reason given")
        );
    }
    Ok(MergeState {
        merged: body["merged"].as_bool().unwrap_or(false),
        merge_commit_sha: body["merge_commit_sha"]
            .as_str()
            .filter(|sha| sha.len() >= 7)
            .map(str::to_string),
        merged_at: body["merged_at"].as_str().map(str::to_string),
        state: body["state"].as_str().unwrap_or("unknown").to_string(),
    })
}
