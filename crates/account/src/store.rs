//! Durable storage for the cookie jar (SEC-01).
//!
//! The jar is serialized, then encrypted with XChaCha20-Poly1305 (a vetted AEAD; no custom
//! construction). The random 256-bit key lives in the OS vault; the ciphertext lives in a
//! private (user-only) directory. If the vault is unavailable nothing durable is written: the
//! caller can choose memory-only mode, but there is never a plaintext fallback.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use zeroize::Zeroize;

use serde::{Deserialize, Serialize};

use crate::jar::CookieJar;
use crate::vault::{Vault, VaultError};

/// What is persisted: the jar plus the identity the owner explicitly selected (an opaque key:
/// page id when the service gives one, else the display name).
#[derive(Serialize, Deserialize, Default)]
pub struct StoredAccount {
    pub jar: CookieJar,
    #[serde(default)]
    pub selected_identity: Option<String>,
}

const KEY_NAME: &str = "jar-key-v1";
const FILE_NAME: &str = "session.v1";
const MAGIC: &[u8; 4] = b"APJ1";
const AAD: &[u8] = b"apollo/cookie-jar/v1";
const NONCE_LEN: usize = 24;

#[derive(Debug)]
pub enum StoreError {
    /// The OS vault could not be used. Nothing was written or read.
    VaultUnavailable(String),
    /// Ciphertext exists but its key is gone from the vault (entry deleted by the user, new
    /// Windows profile...). The data is unrecoverable and should be discarded.
    KeyMissing,
    /// Authentication failed or the file is malformed: tampered, truncated or from another key.
    Corrupt,
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::VaultUnavailable(e) => write!(f, "{e}"),
            StoreError::KeyMissing => write!(f, "stored session cannot be decrypted (key missing)"),
            StoreError::Corrupt => write!(f, "stored session is corrupt"),
            StoreError::Io(e) => write!(f, "storage error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<VaultError> for StoreError {
    fn from(e: VaultError) -> Self {
        StoreError::VaultUnavailable(e.to_string())
    }
}

/// What `delete` managed to remove (logout reports this honestly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteReport {
    pub key_deleted: bool,
    pub ciphertext_deleted: bool,
}

pub struct EncryptedStore {
    vault: Arc<dyn Vault>,
    path: PathBuf,
}

impl EncryptedStore {
    /// `dir` is created as a private (user-only) directory on Windows.
    pub fn new(vault: Arc<dyn Vault>, dir: &Path) -> Result<Self, StoreError> {
        #[cfg(windows)]
        apollo_services_common::win::ensure_private_dir(dir).map_err(|e| StoreError::Io(e.to_string()))?;
        #[cfg(not(windows))]
        fs::create_dir_all(dir).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Self {
            vault,
            path: dir.join(FILE_NAME),
        })
    }

    pub fn ciphertext_path(&self) -> &Path {
        &self.path
    }

    fn key(&self, create: bool) -> Result<Option<[u8; 32]>, StoreError> {
        match self.vault.get(KEY_NAME)? {
            Some(mut k) if k.len() == 32 => {
                let mut out = [0u8; 32];
                out.copy_from_slice(&k);
                k.zeroize();
                Ok(Some(out))
            }
            Some(mut bad) => {
                bad.zeroize();
                Err(StoreError::Corrupt)
            }
            None if create => {
                let mut k = [0u8; 32];
                getrandom::fill(&mut k).map_err(|e| StoreError::Io(e.to_string()))?;
                self.vault.set(KEY_NAME, &k)?;
                Ok(Some(k))
            }
            None => Ok(None),
        }
    }

    pub fn save(&self, account: &StoredAccount) -> Result<(), StoreError> {
        let mut key = self.key(true)?.expect("created");
        let mut plain = serde_json::to_vec(account).map_err(|e| StoreError::Io(e.to_string()))?;
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| StoreError::Io(e.to_string()))?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &plain, aad: AAD })
            .map_err(|_| StoreError::Io("encryption failed".into()))?;
        key.zeroize();
        plain.zeroize();

        let mut blob = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ct.len());
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ct);
        // Atomic replace: a crash leaves either the old or the new ciphertext, never half.
        let tmp = self.path.with_extension("tmp");
        let write = || -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&blob)?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        };
        write().map_err(|e| {
            let _ = fs::remove_file(&tmp);
            StoreError::Io(e.to_string())
        })
    }

    pub fn load(&self) -> Result<Option<StoredAccount>, StoreError> {
        let blob = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::Io(e.to_string())),
        };
        if blob.len() < MAGIC.len() + NONCE_LEN + 16 || &blob[..4] != MAGIC {
            return Err(StoreError::Corrupt);
        }
        let Some(mut key) = self.key(false)? else {
            return Err(StoreError::KeyMissing);
        };
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
        let nonce = XNonce::from_slice(&blob[4..4 + NONCE_LEN]);
        let plain = cipher.decrypt(
            nonce,
            Payload {
                msg: &blob[4 + NONCE_LEN..],
                aad: AAD,
            },
        );
        key.zeroize();
        let mut plain = plain.map_err(|_| StoreError::Corrupt)?;
        let account: Result<StoredAccount, _> = serde_json::from_slice(&plain);
        plain.zeroize();
        account.map(Some).map_err(|_| StoreError::Corrupt)
    }

    /// Remove the vault key and the ciphertext. Both are attempted even if one fails.
    pub fn delete(&self) -> DeleteReport {
        let key_deleted = self.vault.delete(KEY_NAME).is_ok();
        let ciphertext_deleted = match fs::remove_file(&self.path) {
            Ok(()) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        let _ = fs::remove_file(self.path.with_extension("tmp"));
        DeleteReport {
            key_deleted,
            ciphertext_deleted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jar::Cookie;
    use crate::vault::{MemoryVault, UnavailableVault};

    fn jar() -> StoredAccount {
        StoredAccount { selected_identity: Some("page-1".into()), jar: CookieJar {
            cookies: vec![Cookie {
                name: "SAPISID".into(),
                value: "VALUECANARY-1234567890".into(),
                domain: ".youtube.com".into(),
                path: "/".into(),
                secure: true,
                http_only: false,
                expires_unix: Some(4_102_444_800),
            }],
            ephemeral_seed: false,
        } }
    }

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("apollo-store-test-{}", apollo_services_common::random_hex(6)));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn roundtrip_and_no_plaintext_on_disk() {
        let dir = tmp();
        let vault = Arc::new(MemoryVault::new());
        let s = EncryptedStore::new(vault.clone(), &dir).unwrap();
        s.save(&jar()).unwrap();
        let raw = fs::read(s.ciphertext_path()).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("VALUECANARY") && !text.contains("SAPISID"), "plaintext on disk");
        assert_eq!(&raw[..4], MAGIC);
        // The vault holds only the 32-byte key.
        assert_eq!(vault.get(KEY_NAME).unwrap().unwrap().len(), 32);
        let back = s.load().unwrap().unwrap();
        assert_eq!(back.jar.get("SAPISID").unwrap().value, "VALUECANARY-1234567890");
        assert_eq!(back.selected_identity.as_deref(), Some("page-1"));
        // Every save uses a fresh nonce.
        s.save(&jar()).unwrap();
        assert_ne!(raw, fs::read(s.ciphertext_path()).unwrap());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tampering_and_wrong_key_are_detected() {
        let dir = tmp();
        let vault = Arc::new(MemoryVault::new());
        let s = EncryptedStore::new(vault.clone(), &dir).unwrap();
        s.save(&jar()).unwrap();
        let mut raw = fs::read(s.ciphertext_path()).unwrap();
        let n = raw.len();
        raw[n - 1] ^= 1;
        fs::write(s.ciphertext_path(), &raw).unwrap();
        assert!(matches!(s.load(), Err(StoreError::Corrupt)));
        // Header damage too.
        fs::write(s.ciphertext_path(), b"garbage").unwrap();
        assert!(matches!(s.load(), Err(StoreError::Corrupt)));
        // A different key cannot decrypt a valid file.
        s.save(&jar()).unwrap();
        vault.set(KEY_NAME, &[7u8; 32]).unwrap();
        assert!(matches!(s.load(), Err(StoreError::Corrupt)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_key_is_distinct_from_corruption() {
        let dir = tmp();
        let vault = Arc::new(MemoryVault::new());
        let s = EncryptedStore::new(vault.clone(), &dir).unwrap();
        s.save(&jar()).unwrap();
        vault.delete(KEY_NAME).unwrap();
        assert!(matches!(s.load(), Err(StoreError::KeyMissing)));
        let _ = fs::remove_dir_all(dir);
    }

    // TEST-16: with the vault unavailable nothing durable is written, ever.
    #[test]
    fn unavailable_vault_never_falls_back_to_plaintext() {
        let dir = tmp();
        let s = EncryptedStore::new(Arc::new(UnavailableVault("locked".into())), &dir).unwrap();
        assert!(matches!(s.save(&jar()), Err(StoreError::VaultUnavailable(_))));
        assert!(!s.ciphertext_path().exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "no file of any kind was written");
        assert!(matches!(s.load(), Ok(None)), "nothing stored means nothing to load");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_removes_key_and_ciphertext() {
        let dir = tmp();
        let vault = Arc::new(MemoryVault::new());
        let s = EncryptedStore::new(vault.clone(), &dir).unwrap();
        s.save(&jar()).unwrap();
        let r = s.delete();
        assert_eq!(r, DeleteReport { key_deleted: true, ciphertext_deleted: true });
        assert!(vault.get(KEY_NAME).unwrap().is_none());
        assert!(!s.ciphertext_path().exists());
        assert!(matches!(s.load(), Ok(None)));
        // Idempotent.
        assert_eq!(s.delete(), DeleteReport { key_deleted: true, ciphertext_deleted: true });
        let _ = fs::remove_dir_all(dir);
    }
}
