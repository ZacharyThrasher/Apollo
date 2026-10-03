//! Where the small secret lives: a 32-byte key (never the cookie jar itself; Credential Manager
//! blobs are limited to 2.5 KB). `Vault` is a trait so the storage can be tested, and so an
//! unavailable vault is a normal, handled condition rather than a reason to write plaintext.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use zeroize::Zeroize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    /// The vault cannot be used right now (locked, service missing, access denied).
    Unavailable(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::Unavailable(e) => write!(f, "credential vault unavailable: {e}"),
        }
    }
}

impl std::error::Error for VaultError {}

pub trait Vault: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, VaultError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), VaultError>;
    /// Deleting something that does not exist is success.
    fn delete(&self, key: &str) -> Result<(), VaultError>;
}

/// Process-memory vault: the explicit "memory-only" mode and the test double.
#[derive(Default)]
pub struct MemoryVault {
    items: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryVault {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Vault for MemoryVault {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self.items.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), VaultError> {
        self.items.lock().unwrap().insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        if let Some(mut v) = self.items.lock().unwrap().remove(key) {
            v.zeroize();
        }
        Ok(())
    }
}

/// A vault that always fails: models a locked or missing credential store (TEST-16).
pub struct UnavailableVault(pub String);

impl Vault for UnavailableVault {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>, VaultError> {
        Err(VaultError::Unavailable(self.0.clone()))
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), VaultError> {
        Err(VaultError::Unavailable(self.0.clone()))
    }
    fn delete(&self, _: &str) -> Result<(), VaultError> {
        Err(VaultError::Unavailable(self.0.clone()))
    }
}

#[cfg(windows)]
pub use self::windows_vault::WindowsCredentialVault;

#[cfg(windows)]
mod windows_vault {
    use super::{Vault, VaultError};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    /// Windows Credential Manager (generic credentials, persisted per machine, readable only
    /// by this user). Entries are named `<service>/<key>`.
    pub struct WindowsCredentialVault {
        service: String,
    }

    impl WindowsCredentialVault {
        pub fn new(service: impl Into<String>) -> Self {
            Self {
                service: service.into(),
            }
        }

        fn target(&self, key: &str) -> Vec<u16> {
            format!("{}/{}", self.service, key)
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect()
        }
    }

    fn err(e: windows::core::Error) -> VaultError {
        VaultError::Unavailable(format!("{e}"))
    }

    impl Vault for WindowsCredentialVault {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, VaultError> {
            let target = self.target(key);
            unsafe {
                let mut out: *mut CREDENTIALW = std::ptr::null_mut();
                match CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut out) {
                    Ok(()) => {
                        let c = &*out;
                        let blob =
                            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
                                .to_vec();
                        CredFree(out as *const _);
                        Ok(Some(blob))
                    }
                    Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
                    Err(e) => Err(err(e)),
                }
            }
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), VaultError> {
            let mut target = self.target(key);
            let mut user: Vec<u16> = "apollo".encode_utf16().chain(std::iter::once(0)).collect();
            let mut blob = value.to_vec();
            let cred = CREDENTIALW {
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: windows::core::PWSTR(target.as_mut_ptr()),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: windows::core::PWSTR(user.as_mut_ptr()),
                ..Default::default()
            };
            let r = unsafe { CredWriteW(&cred, 0) }.map_err(err);
            use zeroize::Zeroize;
            blob.zeroize();
            r
        }

        fn delete(&self, key: &str) -> Result<(), VaultError> {
            let target = self.target(key);
            match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
                Err(e) => Err(err(e)),
            }
        }
    }
}
