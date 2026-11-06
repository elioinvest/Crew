//! Secrets at rest, so the hosts and providers already saved keep working: "v10", a 12-byte nonce, then AES-256-GCM with the key from "Local State"
//! in the data folder, itself protected by Windows data protection for this user.

use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, OsRng, rand_core::RngCore};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use serde_json::{Value, json};

use crate::config::Paths;

const PREFIX: &[u8] = b"v10";
const DPAPI: &[u8] = b"DPAPI";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData};

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: on success Windows hands over a buffer of cbData bytes, to free with LocalFree.
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            bytes
        }
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: input points at live memory for the call, and out is written by it.
        let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out) };
        (ok != 0).then(|| take(out))
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: as above.
        let ok = unsafe { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out) };
        (ok != 0).then(|| take(out))
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

fn local_state(dir: &Path) -> Value {
    std::fs::read_to_string(dir.join("Local State")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
}

fn read_key(dir: &Path) -> Option<Vec<u8>> {
    let encoded = local_state(dir).pointer("/os_crypt/encrypted_key")?.as_str()?.to_string();
    let wrapped = B64.decode(encoded).ok()?;
    dpapi::unprotect(wrapped.strip_prefix(DPAPI)?).filter(|k| k.len() == 32)
}

/// The key, made and saved if this data folder doesn't have one yet.
fn key_or_new(dir: &Path) -> Option<Vec<u8>> {
    if let Some(key) = read_key(dir) {
        return Some(key);
    }
    let mut key = vec![0u8; 32];
    OsRng.fill_bytes(&mut key);
    let wrapped = [DPAPI, &dpapi::protect(&key)?].concat();
    let mut state = local_state(dir);
    let map = state.as_object_mut()?;
    let os_crypt = map.entry("os_crypt").or_insert_with(|| json!({}));
    os_crypt.as_object_mut()?.insert("encrypted_key".into(), json!(B64.encode(wrapped)));
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(dir.join("Local State"), serde_json::to_string(&state).ok()?).ok()?;
    Some(key)
}

pub fn can_seal() -> bool {
    dpapi::protect(b"x").is_some()
}

pub fn seal(paths: &Paths, text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let key = key_or_new(&paths.data_dir)?;
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let body = cipher.encrypt(Nonce::from_slice(&nonce), text.as_bytes()).ok()?;
    Some(B64.encode([PREFIX, &nonce, &body].concat()))
}

pub fn unseal(paths: &Paths, sealed: &str) -> Option<String> {
    let bytes = B64.decode(sealed).ok()?;
    let rest = bytes.strip_prefix(PREFIX)?;
    if rest.len() < 12 {
        return None;
    }
    let (nonce, body) = rest.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(&read_key(&paths.data_dir)?).ok()?;
    String::from_utf8(cipher.decrypt(Nonce::from_slice(nonce), body).ok()?).ok()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn seals_and_unseals_and_keeps_the_rest_of_local_state() {
        let d = std::env::temp_dir().join(format!("crew-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Local State"), r#"{"other":1}"#).unwrap();
        let paths = Paths { home: d.clone(), data_dir: d.clone(), codex_home: d.join(".codex") };
        let sealed = seal(&paths, "secret").unwrap();
        assert_eq!(unseal(&paths, &sealed).as_deref(), Some("secret"));
        assert_eq!(local_state(&d).get("other"), Some(&json!(1)));
        assert_eq!(unseal(&paths, "not base64!"), None);
        let again = seal(&paths, "secret").unwrap();
        assert_ne!(sealed, again, "each seal has its own nonce");
    }
}
