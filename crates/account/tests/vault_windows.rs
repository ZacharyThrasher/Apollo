#![cfg(windows)]
//! The real Windows Credential Manager (per-user, generic credentials). Uses a throwaway service
//! name and removes every entry it creates.

use std::fs;
use std::sync::Arc;

use apollo_account::jar::{Cookie, CookieJar};
use apollo_account::store::{EncryptedStore, StoredAccount};
use apollo_account::vault::{Vault, WindowsCredentialVault};
use apollo_services_common::random_hex;

fn service() -> String {
    format!("ApolloTest-{}", random_hex(6))
}

#[test]
fn set_get_overwrite_delete_roundtrip() {
    let v = WindowsCredentialVault::new(service());
    assert_eq!(v.get("k").unwrap(), None, "absent entry reads as None, not an error");
    v.set("k", &[1, 2, 3, 4]).unwrap();
    assert_eq!(v.get("k").unwrap(), Some(vec![1, 2, 3, 4]));
    v.set("k", &[9; 32]).unwrap();
    assert_eq!(v.get("k").unwrap(), Some(vec![9; 32]), "overwrite");
    v.delete("k").unwrap();
    assert_eq!(v.get("k").unwrap(), None);
    v.delete("k").unwrap(); // deleting a missing entry is success
}

#[test]
fn binary_values_with_zero_bytes_survive() {
    let v = WindowsCredentialVault::new(service());
    let key: Vec<u8> = (0..32).map(|i| if i % 3 == 0 { 0 } else { i as u8 * 7 }).collect();
    v.set("bin", &key).unwrap();
    assert_eq!(v.get("bin").unwrap().unwrap(), key);
    v.delete("bin").unwrap();
}

// The shipped combination: key in Credential Manager, ciphertext in a private directory.
#[test]
fn encrypted_store_on_the_real_vault_roundtrips_and_cleans_up() {
    let dir = std::env::temp_dir().join(format!("apollo-realvault-test-{}", random_hex(6)));
    let svc = service();
    let store = EncryptedStore::new(Arc::new(WindowsCredentialVault::new(svc.clone())), &dir).unwrap();
    let acc = StoredAccount {
        premium_declared: true,
        jar: CookieJar {
            cookies: vec![Cookie {
                name: "SAPISID".into(),
                value: "VALUECANARY-realvault".into(),
                domain: ".youtube.com".into(),
                path: "/".into(),
                secure: true,
                http_only: false,
                expires_unix: Some(4_102_444_800),
            }],
            ephemeral_seed: false,
        },
        selected_identity: Some("PAGE0".into()),
    };
    store.save(&acc).unwrap();
    let raw = fs::read(store.ciphertext_path()).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("VALUECANARY"));
    let back = store.load().unwrap().unwrap();
    assert_eq!(back.jar.get("SAPISID").unwrap().value, "VALUECANARY-realvault");
    assert_eq!(back.selected_identity.as_deref(), Some("PAGE0"));

    // A second store object over the same real vault finds it again (process-restart stand-in).
    let again = EncryptedStore::new(Arc::new(WindowsCredentialVault::new(svc.clone())), &dir).unwrap();
    assert!(again.load().unwrap().is_some());

    let report = again.delete();
    assert!(report.key_deleted && report.ciphertext_deleted);
    assert!(WindowsCredentialVault::new(svc).get("jar-key-v1").unwrap().is_none(), "no entry left in Credential Manager");
    let _ = fs::remove_dir_all(dir);
}

// The directory holding the ciphertext is user-only (protected DACL, no inheritance).
#[test]
fn ciphertext_directory_is_user_only() {
    let dir = std::env::temp_dir().join(format!("apollo-realvault-test-{}", random_hex(6)));
    let _store = EncryptedStore::new(Arc::new(WindowsCredentialVault::new(service())), &dir).unwrap();
    let out = std::process::Command::new("icacls").arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let aces = text.lines().filter(|l| l.contains(":(")).count();
    assert_eq!(aces, 1, "exactly one ACE expected: {text}");
    assert!(!text.contains("(I)"), "inherited ACE present: {text}");
    let _ = fs::remove_dir_all(dir);
}
