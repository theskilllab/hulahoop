//! The vault: one key per data directory, AES-256-GCM for every secret the
//! server keeps on someone else's behalf.
//!
//! A secret in this database is a credential for an account the user owns. The
//! key lives beside the database under `secrets/`, mode 0600, so a copy of the
//! SQLite file alone is not a copy of the credentials — and a `grep` over the
//! data directory finds ciphertext.

use anyhow::{anyhow, bail, Context, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::{Path, PathBuf};

/// Binds a ciphertext to this format, so a blob from another version or another
/// purpose cannot be decrypted just because the key happens to match.
const AAD: &[u8] = b"hulahoop-vault-v1";
const PREFIX: &str = "v1:";

pub struct Vault {
    key: LessSafeKey,
}

impl Vault {
    /// Open the vault for a data directory, creating the key on first use. The
    /// key is a file rather than a row because a row would be as readable as the
    /// ciphertext it protects.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let dir = data_dir.join("secrets");
        std::fs::create_dir_all(&dir)?;
        restrict_dir(&dir)?;
        let path = dir.join("vault.key");
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = vec![0u8; 32];
                SystemRandom::new()
                    .fill(&mut bytes)
                    .map_err(|_| anyhow!("the system random source refused to make a vault key"))?;
                std::fs::write(&path, &bytes)?;
                restrict_file(&path)?;
                bytes
            }
            Err(error) => return Err(error).context(format!("reading {}", path.display())),
        };
        Self::from_key(&raw)
    }

    pub fn from_key(raw: &[u8]) -> Result<Self> {
        let unbound = UnboundKey::new(&AES_256_GCM, raw)
            .map_err(|_| anyhow!("the vault key must be 32 bytes of AES-256 key material"))?;
        Ok(Vault {
            key: LessSafeKey::new(unbound),
        })
    }

    /// `v1:` plus the nonce and the sealed plaintext, hex encoded so the value is
    /// safe in a text column and cannot be mistaken for the plaintext.
    pub fn seal(&self, plaintext: &[u8]) -> Result<String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| anyhow!("the system random source refused to make a nonce"))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut sealed = plaintext.to_vec();
        self.key
            .seal_in_place_append_tag(nonce, Aad::from(AAD), &mut sealed)
            .map_err(|_| anyhow!("sealing a secret failed"))?;
        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&sealed);
        Ok(format!("{PREFIX}{}", hex::encode(blob)))
    }

    pub fn open_secret(&self, blob: &str) -> Result<Vec<u8>> {
        let body = blob
            .strip_prefix(PREFIX)
            .ok_or_else(|| anyhow!("this is not a vault ciphertext"))?;
        let mut bytes = hex::decode(body).context("a vault ciphertext is hex")?;
        if bytes.len() < NONCE_LEN {
            bail!("a vault ciphertext is shorter than its nonce");
        }
        let sealed = bytes.split_off(NONCE_LEN);
        let nonce_bytes: [u8; NONCE_LEN] = bytes
            .try_into()
            .map_err(|_| anyhow!("a vault ciphertext has a malformed nonce"))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut sealed = sealed;
        let opened = self
            .key
            .open_in_place(nonce, Aad::from(AAD), &mut sealed)
            .map_err(|_| anyhow!("a stored secret could not be decrypted with this vault key"))?;
        Ok(opened.to_vec())
    }

    pub fn seal_string(&self, plaintext: &str) -> Result<String> {
        self.seal(plaintext.as_bytes())
    }

    pub fn open_string(&self, blob: &str) -> Result<String> {
        String::from_utf8(self.open_secret(blob)?).context("a stored secret is not UTF-8")
    }
}

pub fn secret_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("secrets")
}

fn restrict_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

fn restrict_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_round_trips_and_never_appears_in_the_blob() {
        let vault = Vault::from_key(&[7u8; 32]).unwrap();
        let blob = vault.seal_string("271828182845").unwrap();
        assert!(!blob.contains("271828182845"));
        assert_eq!(vault.open_string(&blob).unwrap(), "271828182845");
    }

    #[test]
    fn a_tampered_blob_is_refused() {
        let vault = Vault::from_key(&[7u8; 32]).unwrap();
        let mut blob = vault.seal_string("secret").unwrap();
        let last = blob.len() - 1;
        let flipped = if blob.ends_with('0') { '1' } else { '0' };
        blob.replace_range(last.., &flipped.to_string());
        assert!(vault.open_string(&blob).is_err());
    }

    #[test]
    fn another_key_cannot_open_the_blob() {
        let sealed = Vault::from_key(&[7u8; 32])
            .unwrap()
            .seal_string("secret")
            .unwrap();
        assert!(Vault::from_key(&[9u8; 32])
            .unwrap()
            .open_string(&sealed)
            .is_err());
    }
}
