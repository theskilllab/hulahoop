//! GitHub app credentials: the token the loop mints for an app, and the one
//! question the connections list asks about a stored key — can it sign?
//!
//! A GitHub app authenticates as itself with a short-lived RS256 JWT signed by
//! the app's private key; GitHub swaps that for an installation token.
//!
//! The pull request screen reads and writes as a person instead: [`Github`] is a
//! small REST client bound to one token and one API base, so a test can point it
//! at a fake GitHub on localhost.

use anyhow::{anyhow, bail, Context, Result};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use sha2::Sha256;

pub const PROVIDER: &str = "github";

/// The GitHub API base URL this process talks to: `HULAHOOP_GITHUB_API` when
/// set, GitHub itself otherwise.
pub fn api_base() -> String {
    hulahoop_core::github_api()
}

/// An HTTP client with deadlines. A GitHub call that hangs would otherwise hold a
/// sync, a merge or a person's button press for as long as the socket stays open.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

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
        .get(format!("{}/app", api_base()))
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
                .get(format!("{}/installation/repositories", api_base()))
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
            .get(format!("{}/app/installations", api_base()))
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
            "{}/app/installations/{installation}/access_tokens",
            api_base()
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
    installation_token_at(http, &api_base(), jwt, owner, repo).await
}

/// [`installation_token`] against an explicit API base, so a caller that was
/// given its GitHub (the engine, a test) never falls back to the environment.
pub async fn installation_token_at(
    http: &reqwest::Client,
    base: &str,
    jwt: &str,
    owner: &str,
    repo: &str,
) -> Result<String> {
    let base = base.trim_end_matches('/');
    let response = http
        .get(format!("{base}/repos/{owner}/{repo}/installation"))
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
            "{base}/app/installations/{installation}/access_tokens"
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
            "{}/repos/{owner}/{repo}/pulls/{number}",
            api_base()
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

/// A REST client bound to one token and one API base. Every call answers with
/// GitHub's JSON, or with an error that carries GitHub's own reason, because the
/// person pressing Merge needs to read why GitHub refused, not that it did.
///
/// The token never appears in an error: only the method, the path and GitHub's
/// message do.
#[derive(Clone)]
pub struct Github {
    http: reqwest::Client,
    base: String,
    token: String,
}

/// Why a call to GitHub failed, kept typed so callers can tell a pull request
/// that is gone (404/410) from one GitHub would not show right now, and a rate
/// limit from everything else. `Display` is the message a person reads.
#[derive(Debug, Clone)]
pub struct GithubError {
    /// The HTTP status, or `None` when GitHub could not be reached at all.
    pub status: Option<u16>,
    pub message: String,
    /// When GitHub said to stop asking: the moment the limit lifts.
    pub rate_limited_until: Option<chrono::DateTime<chrono::Utc>>,
}

impl std::fmt::Display for GithubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GithubError {}

impl GithubError {
    /// The typed error behind an `anyhow` chain, if GitHub produced it.
    pub fn of(error: &anyhow::Error) -> Option<&GithubError> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<GithubError>())
    }

    /// GitHub says the thing is not there (any more), as opposed to an outage,
    /// a timeout or a rate limit.
    pub fn is_gone(error: &anyhow::Error) -> bool {
        matches!(Self::of(error).and_then(|e| e.status), Some(404 | 410))
    }

    /// GitHub would not show this to the caller: the token cannot see it.
    pub fn is_denied(error: &anyhow::Error) -> bool {
        matches!(
            Self::of(error).and_then(|e| e.status),
            Some(401 | 403 | 404)
        ) && Self::rate_limit(error).is_none()
    }

    pub fn rate_limit(error: &anyhow::Error) -> Option<chrono::DateTime<chrono::Utc>> {
        Self::of(error).and_then(|e| e.rate_limited_until)
    }
}

/// When a refusal is a rate limit, the moment it lifts. GitHub signals a
/// secondary limit with `retry-after` and a primary one with
/// `x-ratelimit-remaining: 0` plus the reset time in epoch seconds.
fn rate_limited_until(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if status != reqwest::StatusCode::FORBIDDEN && status != reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return None;
    }
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
    };
    if let Some(seconds) = header("retry-after").and_then(|v| v.parse::<i64>().ok()) {
        return Some(chrono::Utc::now() + chrono::Duration::seconds(seconds.max(1)));
    }
    if header("x-ratelimit-remaining") == Some("0") {
        let reset = header("x-ratelimit-reset")
            .and_then(|v| v.parse::<i64>().ok())
            .and_then(|epoch| chrono::DateTime::from_timestamp(epoch, 0));
        return Some(reset.unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::minutes(1)));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Some(chrono::Utc::now() + chrono::Duration::minutes(1));
    }
    None
}

/// Pages of 100 are GitHub's ceiling; thirty of them is GitHub's own cap on the
/// files of one pull request, so no listing here needs more.
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 30;

impl Github {
    pub fn new(base: &str, token: &str) -> Self {
        Self {
            http: http_client(),
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .query(query)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "hulahoop")
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|error| {
            anyhow!(GithubError {
                status: None,
                message: format!(
                    "GitHub could not be reached ({method} {path}): {}",
                    error.without_url()
                ),
                rate_limited_until: None,
            })
        })?;
        let status = response.status();
        let limited = rate_limited_until(status, response.headers());
        let text = response.text().await.unwrap_or_default();
        let value: serde_json::Value = if text.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
        };
        if !status.is_success() {
            return Err(anyhow!(GithubError {
                status: Some(status.as_u16()),
                message: format!(
                    "GitHub refused {method} {path} ({status}): {}",
                    reason(&value)
                ),
                rate_limited_until: limited,
            }));
        }
        Ok(value)
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        self.send(reqwest::Method::GET, path, query, None).await
    }

    /// Every page of a listing, up to GitHub's own cap. `key` names the array in
    /// an envelope (`check_runs`); `None` means the response is the array.
    async fn list(
        &self,
        path: &str,
        query: &[(&str, String)],
        key: Option<&str>,
        max_pages: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let mut out = Vec::new();
        for page in 1..=max_pages {
            let mut paged = query.to_vec();
            paged.push(("per_page", PAGE_SIZE.to_string()));
            paged.push(("page", page.to_string()));
            let body = self.get(path, &paged).await?;
            let batch = match key {
                Some(key) => body[key].as_array().cloned().unwrap_or_default(),
                None => body.as_array().cloned().unwrap_or_default(),
            };
            let count = batch.len();
            out.extend(batch);
            if count < PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }

    /// Who the token belongs to.
    pub async fn user(&self) -> Result<serde_json::Value> {
        self.get("/user", &[]).await
    }

    /// The repository as this token sees it: `permissions` says what the person
    /// may do, `allow_*_merge` which merge methods the repository accepts.
    pub async fn repo(&self, owner: &str, repo: &str) -> Result<serde_json::Value> {
        self.get(&format!("/repos/{owner}/{repo}"), &[]).await
    }

    /// One page of pull requests, newest update first.
    pub async fn pulls_page(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        page: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let body = self
            .get(
                &format!("/repos/{owner}/{repo}/pulls"),
                &[
                    ("state", state.to_string()),
                    ("sort", "updated".to_string()),
                    ("direction", "desc".to_string()),
                    ("per_page", PAGE_SIZE.to_string()),
                    ("page", page.to_string()),
                ],
            )
            .await?;
        Ok(body.as_array().cloned().unwrap_or_default())
    }

    /// Every open pull request, however many pages that takes (up to the cap).
    pub async fn open_pulls(&self, owner: &str, repo: &str) -> Result<Vec<serde_json::Value>> {
        self.list(
            &format!("/repos/{owner}/{repo}/pulls"),
            &[
                ("state", "open".to_string()),
                ("sort", "updated".to_string()),
                ("direction", "desc".to_string()),
            ],
            None,
            MAX_PAGES,
        )
        .await
    }

    /// Whether the token may list pull requests at all. A fine-grained token can
    /// see a repository and still have no access to its pull requests, so this is
    /// asked separately, with the smallest page GitHub serves.
    pub async fn can_list_pulls(&self, owner: &str, repo: &str) -> Result<()> {
        self.get(
            &format!("/repos/{owner}/{repo}/pulls"),
            &[("per_page", "1".to_string())],
        )
        .await
        .map(|_| ())
    }

    pub async fn pull(&self, owner: &str, repo: &str, number: i64) -> Result<serde_json::Value> {
        self.get(&format!("/repos/{owner}/{repo}/pulls/{number}"), &[])
            .await
    }

    /// The changed files with their unified hunks. GitHub leaves `patch` out for
    /// binary files and very large ones.
    pub async fn pull_files(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<Vec<serde_json::Value>> {
        self.list(
            &format!("/repos/{owner}/{repo}/pulls/{number}/files"),
            &[],
            None,
            MAX_PAGES,
        )
        .await
    }

    /// Submitted reviews, oldest first.
    pub async fn reviews(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<Vec<serde_json::Value>> {
        self.list(
            &format!("/repos/{owner}/{repo}/pulls/{number}/reviews"),
            &[],
            None,
            10,
        )
        .await
    }

    /// Line comments on the diff, oldest first.
    pub async fn review_comments(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<Vec<serde_json::Value>> {
        self.list(
            &format!("/repos/{owner}/{repo}/pulls/{number}/comments"),
            &[],
            None,
            10,
        )
        .await
    }

    /// The people still asked to review: `{ users: [...], teams: [...] }`.
    pub async fn requested_reviewers(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<serde_json::Value> {
        self.get(
            &format!("/repos/{owner}/{repo}/pulls/{number}/requested_reviewers"),
            &[],
        )
        .await
    }

    /// Asks these people to review the pull request again.
    pub async fn request_reviewers(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        logins: &[String],
    ) -> Result<()> {
        self.send(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/pulls/{number}/requested_reviewers"),
            &[],
            Some(&serde_json::json!({ "reviewers": logins })),
        )
        .await?;
        Ok(())
    }

    /// The check runs reported against a commit.
    pub async fn check_runs(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<serde_json::Value>> {
        self.list(
            &format!("/repos/{owner}/{repo}/commits/{sha}/check-runs"),
            &[],
            Some("check_runs"),
            5,
        )
        .await
    }

    /// The older commit-status API, folded into one state by GitHub. A commit
    /// nobody reported on answers `pending` with no statuses at all.
    pub async fn combined_status(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<serde_json::Value> {
        self.get(
            &format!("/repos/{owner}/{repo}/commits/{sha}/status"),
            &[("per_page", PAGE_SIZE.to_string())],
        )
        .await
    }

    /// Submits a review with its line comments in one call, so GitHub shows
    /// them together and nothing is half-sent.
    pub async fn create_review(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        event: &str,
        body: Option<&str>,
        comments: &[serde_json::Value],
    ) -> Result<serde_json::Value> {
        let mut payload = serde_json::json!({ "event": event, "comments": comments });
        if let Some(body) = body.filter(|b| !b.trim().is_empty()) {
            payload["body"] = serde_json::Value::String(body.to_string());
        }
        self.send(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/pulls/{number}/reviews"),
            &[],
            Some(&payload),
        )
        .await
    }

    /// Merges on GitHub. `method` is `merge`, `squash` or `rebase`. `sha` pins
    /// the merge to the head the person reviewed: GitHub answers 409 instead of
    /// merging commits pushed after they looked.
    pub async fn merge(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        method: &str,
        sha: &str,
    ) -> Result<serde_json::Value> {
        self.send(
            reqwest::Method::PUT,
            &format!("/repos/{owner}/{repo}/pulls/{number}/merge"),
            &[],
            Some(&serde_json::json!({ "merge_method": method, "sha": sha })),
        )
        .await
    }

    /// Whether a branch is still on GitHub. The exact-match ref endpoint
    /// answers 404 once it is gone.
    pub async fn branch_exists(&self, owner: &str, repo: &str, branch: &str) -> Result<bool> {
        match self
            .get(
                &format!("/repos/{owner}/{repo}/git/ref/heads/{branch}"),
                &[],
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if GithubError::of(&error).and_then(|e| e.status) == Some(404) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Deletes a branch, as GitHub's own button does after a merge.
    pub async fn delete_branch(&self, owner: &str, repo: &str, branch: &str) -> Result<()> {
        self.send(
            reqwest::Method::DELETE,
            &format!("/repos/{owner}/{repo}/git/refs/heads/{branch}"),
            &[],
            None,
        )
        .await?;
        Ok(())
    }
}

/// GitHub's reason for a refusal: its `message`, plus the detail of each entry
/// in `errors` when it gave any.
fn reason(body: &serde_json::Value) -> String {
    let message = body["message"]
        .as_str()
        .unwrap_or("no reason given")
        .to_string();
    let details: Vec<String> = body["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| {
                    error
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| error["message"].as_str().map(str::to_string))
                })
                .collect()
        })
        .unwrap_or_default();
    if details.is_empty() {
        message
    } else {
        format!("{message}: {}", details.join("; "))
    }
}
