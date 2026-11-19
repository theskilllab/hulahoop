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
/// It never leaves this module's callers: the screen sees `provider` and
/// `connected`, the runner sees a minted token.
pub struct StoredApp {
    pub app_id: String,
    pub private_key: String,
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
