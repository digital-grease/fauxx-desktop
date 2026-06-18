// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Database-key management.
//!
//! The SQLCipher database is encrypted with a random 32-byte key. This module
//! sources that key two ways, selected by [`KeySource`]:
//!
//! - [`KeySource::OsKeystore`]: the key lives in the platform credential store
//!   (Secret Service / Keychain / Windows Credential Manager) via `keyring`.
//!   Generated on first run, loaded thereafter.
//! - [`KeySource::EncryptedFile`]: a headless fallback for no-D-Bus hosts. A
//!   random key is wrapped with XChaCha20-Poly1305 under an Argon2id-derived
//!   key from a passphrase, and written to a file. This is the seam C8 #35
//!   hardens.
//!
//! Both paths **fail closed**: if the key cannot be loaded *and* cannot be
//! created/derived, the caller gets a [`CoreError`] and the database is never
//! opened unencrypted.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{CoreError, Result};

/// Length of the SQLCipher database key, in bytes (256-bit).
pub const KEY_LEN: usize = 32;

/// Service name used for the OS keystore entry.
const KEYRING_SERVICE: &str = "fauxx-desktop";
/// Account/user name used for the OS keystore entry (the SQLCipher DB key).
const KEYRING_USER: &str = "db-key";
/// Account/user name for the cross-device pairing keypair (C1 #7). Kept in the
/// OS keystore alongside the DB key, never in the SQLite plaintext.
const KEYRING_DEVICE_KEY_USER: &str = "device-pairing-key";
/// File-name suffix for the headless-fallback wrapped device-keypair file,
/// placed beside the DB key file so both share the passphrase path.
const DEVICE_KEY_FILE_SUFFIX: &str = ".device-key";
/// Magic + version header for the wrapped device-keypair file.
const DEVICE_FILE_MAGIC: &[u8; 8] = b"FAUXXDK1";

/// Account/user name for the persona-pack signing seed (C5 #27 P4). The 32-byte
/// ed25519 seed lives in the OS keystore beside the DB and pairing keys, never
/// in the SQLite plaintext. The ed25519 public key is derived from it on load.
const KEYRING_PACK_KEY_USER: &str = "pack-signing-key";
/// File-name suffix for the headless-fallback wrapped pack-signing-seed file,
/// placed beside the DB key file so both share the passphrase path.
const PACK_KEY_FILE_SUFFIX: &str = ".pack-key";
/// Magic + version header for the wrapped pack-signing-seed file.
const PACK_FILE_MAGIC: &[u8; 8] = b"FAUXXPK1";
/// Length of the persona-pack ed25519 signing SEED, in bytes (the secret half;
/// the public key is derived from it). Equals [`KEY_LEN`] (32) but named so the
/// pack-signing path reads independently of the DB key.
pub const PACK_SEED_LEN: usize = KEY_LEN;

/// Keyring service used for per-persona proxy CREDENTIALS (C7 #30 N1). The
/// secret username/password for an egress proxy live in the OS keystore under
/// this service, keyed by a per-egress account label, NEVER in the SQLite
/// plaintext and never in a log line. The headless fallback wraps them in a file
/// beside the DB key, exactly like the other secrets.
const KEYRING_PROXY_SERVICE: &str = "fauxx-desktop-proxy";
/// File-name suffix for the headless-fallback wrapped proxy-credential store.
const PROXY_CRED_FILE_SUFFIX: &str = ".proxy-creds";
/// Magic + version header for the wrapped proxy-credential file.
const PROXY_FILE_MAGIC: &[u8; 8] = b"FAUXXPC1";

/// Magic + version header for the encrypted key file, so a malformed or alien
/// file is rejected rather than misread.
const FILE_MAGIC: &[u8; 8] = b"FAUXXKF1";
/// Argon2id salt length, in bytes.
const SALT_LEN: usize = 16;
/// XChaCha20-Poly1305 nonce length, in bytes.
const NONCE_LEN: usize = 24;

/// The 32-byte database key, zeroized on drop.
///
/// Wraps the raw bytes so they are scrubbed from memory when no longer needed.
/// The bytes are exposed only to hand the hex key to SQLCipher's `PRAGMA key`.
pub struct DbKey {
    bytes: Zeroizing<[u8; KEY_LEN]>,
}

impl DbKey {
    /// Wrap raw key bytes.
    fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    /// Generate a fresh random key from the OS CSPRNG.
    fn generate() -> Result<Self> {
        let mut bytes = [0u8; KEY_LEN];
        fill_random(&mut bytes)?;
        Ok(Self::from_bytes(bytes))
    }

    /// The key as a 64-char lowercase hex string for `PRAGMA key = "x'..'"`.
    /// Returned in a [`Zeroizing`] wrapper so the formatted copy is scrubbed.
    pub(crate) fn to_hex(&self) -> Zeroizing<String> {
        let mut s = String::with_capacity(KEY_LEN * 2);
        for b in self.bytes.iter() {
            // Writing to a String cannot fail; ignore the formatter Result.
            let _ = std::fmt::write(&mut s, format_args!("{b:02x}"));
        }
        Zeroizing::new(s)
    }

    /// Borrow the raw key bytes (for wrapping/serialization only).
    fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.bytes
    }
}

/// Where the database key comes from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum KeySource {
    /// Use the OS-provided credential store (the default on desktops).
    OsKeystore,
    /// Headless fallback: an Argon2id-derived key unwraps a key file with
    /// XChaCha20-Poly1305. The passphrase is supplied by the caller (the CLI
    /// prompts; the core never prompts interactively).
    EncryptedFile {
        /// Path to the wrapped key file. Created on first use.
        path: PathBuf,
        /// Passphrase that unlocks the file.
        passphrase: String,
    },
}

impl KeySource {
    /// Load the database key, creating/deriving it on first use. Fails closed:
    /// any inability to obtain a usable key is an error, never a silent
    /// fallback to no key.
    pub fn load_or_create(&self) -> Result<DbKey> {
        match self {
            KeySource::OsKeystore => load_or_create_os(),
            KeySource::EncryptedFile { path, passphrase } => load_or_create_file(path, passphrase),
        }
    }
}

/// Fill `buf` with cryptographically secure random bytes via the OS CSPRNG,
/// using `chacha20poly1305`'s re-exported `OsRng` so no extra dependency is
/// pulled in.
fn fill_random(buf: &mut [u8]) -> Result<()> {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut rng = chacha20poly1305::aead::OsRng;
    rng.try_fill_bytes(buf)
        .map_err(|e| CoreError::Key(format!("CSPRNG failure: {e}")))
}

// --- OS keystore path -------------------------------------------------------

/// Register a platform credential store as the `keyring-core` default exactly
/// once per process. On Linux the persistent Secret Service backend is
/// preferred, falling back to the always-available in-kernel keyutils store on
/// headless boxes with no D-Bus.
///
/// The one-time outcome is cached in a [`OnceLock`](std::sync::OnceLock): `Ok`
/// for success, or the error message string (`keyring_core::Error` is not
/// `Clone`) for failure, so every caller observes the same result without any
/// `unsafe` shared mutable state.
fn ensure_default_store() -> Result<()> {
    use std::sync::OnceLock;
    static OUTCOME: OnceLock<std::result::Result<(), String>> = OnceLock::new();

    let cached = OUTCOME.get_or_init(register_platform_store);
    match cached {
        Ok(()) => Ok(()),
        Err(message) => Err(CoreError::Keystore(message.clone())),
    }
}

/// Set the OS-native credential store as the keyring default for this platform.
#[cfg(target_os = "linux")]
fn register_platform_store() -> std::result::Result<(), String> {
    use std::collections::HashMap;
    let cfg: HashMap<&str, &str> = HashMap::new();
    // Prefer the persistent Secret Service backend.
    match dbus_secret_service_keyring_store::Store::new_with_configuration(&cfg) {
        Ok(store) => {
            keyring_core::set_default_store(store);
            Ok(())
        }
        Err(secret_service_err) => {
            // No D-Bus / no Secret Service: fall back to the in-kernel store.
            match linux_keyutils_keyring_store::Store::new_with_configuration(&cfg) {
                Ok(store) => {
                    keyring_core::set_default_store(store);
                    Ok(())
                }
                Err(keyutils_err) => Err(format!(
                    "no OS keystore available (secret-service: {secret_service_err}; \
                     keyutils: {keyutils_err})"
                )),
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn register_platform_store() -> std::result::Result<(), String> {
    use std::collections::HashMap;
    let cfg: HashMap<&str, &str> = HashMap::new();
    let store = apple_native_keyring_store::keychain::Store::new_with_configuration(&cfg)
        .map_err(|e| format!("macOS keychain unavailable: {e}"))?;
    keyring_core::set_default_store(store);
    Ok(())
}

#[cfg(target_os = "windows")]
fn register_platform_store() -> std::result::Result<(), String> {
    use std::collections::HashMap;
    let cfg: HashMap<&str, &str> = HashMap::new();
    let store = windows_native_keyring_store::Store::new_with_configuration(&cfg)
        .map_err(|e| format!("Windows credential store unavailable: {e}"))?;
    keyring_core::set_default_store(store);
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn register_platform_store() -> std::result::Result<(), String> {
    Err("no OS keystore backend compiled for this platform".to_string())
}

/// Load the key from the OS keystore, generating and storing it on first run.
fn load_or_create_os() -> Result<DbKey> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| CoreError::Keystore(format!("keyring entry: {e}")))?;

    match entry.get_secret() {
        Ok(mut secret) => {
            let key = key_from_secret(&secret);
            secret.zeroize();
            key
        }
        Err(keyring_core::Error::NoEntry) => {
            // First run: generate, persist, return.
            let key = DbKey::generate()?;
            entry
                .set_secret(key.as_bytes())
                .map_err(|e| CoreError::Keystore(format!("keyring store: {e}")))?;
            Ok(key)
        }
        Err(e) => Err(CoreError::Keystore(format!("keyring load: {e}"))),
    }
}

/// Convert raw keystore bytes into a [`DbKey`], rejecting the wrong length
/// (fail closed rather than padding/truncating).
fn key_from_secret(secret: &[u8]) -> Result<DbKey> {
    let bytes: [u8; KEY_LEN] = secret.try_into().map_err(|_| {
        CoreError::Key(format!(
            "stored key is {} bytes, expected {KEY_LEN}",
            secret.len()
        ))
    })?;
    Ok(DbKey::from_bytes(bytes))
}

// --- Encrypted-file (passphrase) path ---------------------------------------

/// Derive a 32-byte wrapping key from the passphrase and salt with Argon2id.
fn derive_wrap_key(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut_slice())
        .map_err(|e| CoreError::Key(format!("argon2 derivation: {e}")))?;
    Ok(out)
}

/// Load the key from the encrypted file, creating it on first use.
fn load_or_create_file(path: &Path, passphrase: &str) -> Result<DbKey> {
    if path.exists() {
        read_key_file(path, passphrase)
    } else {
        create_key_file(path, passphrase)
    }
}

// --- Cross-device pairing keypair (C1 #7) -----------------------------------
//
// The pairing keypair is the device's long-lived sync identity. Per the issue,
// it lives in the OS keystore, NOT in the SQLite plaintext. We persist it
// through the same `KeySource` the store uses, so the OS-keystore desktop path
// and the headless passphrase-file path both work and stay testable. The blob
// is the 32-byte public key followed by the 32-byte secret key; the secret half
// is zeroized in the caller (`DeviceIdentity`) and the on-disk fallback wraps it
// under the passphrase with XChaCha20-Poly1305, exactly like the DB key.

/// Length of the persisted device keypair blob: public(32) || secret(32).
pub const DEVICE_KEYPAIR_LEN: usize = KEY_LEN * 2;

/// Store the device pairing keypair through `source`, failing closed. The blob
/// must be exactly [`DEVICE_KEYPAIR_LEN`] bytes.
pub fn store_device_keypair(source: &KeySource, keypair: &[u8]) -> Result<()> {
    if keypair.len() != DEVICE_KEYPAIR_LEN {
        return Err(CoreError::Key(format!(
            "device keypair is {} bytes, expected {DEVICE_KEYPAIR_LEN}",
            keypair.len()
        )));
    }
    match source {
        KeySource::OsKeystore => store_device_keypair_os(keypair),
        KeySource::EncryptedFile { path, passphrase } => {
            write_device_key_file(&device_key_path(path), passphrase, keypair)
        }
    }
}

/// Load the device pairing keypair through `source`, or `None` if none has been
/// stored yet (first run). A malformed or undecryptable blob fails closed.
pub fn load_device_keypair(source: &KeySource) -> Result<Option<Zeroizing<Vec<u8>>>> {
    match source {
        KeySource::OsKeystore => load_device_keypair_os(),
        KeySource::EncryptedFile { path, passphrase } => {
            let p = device_key_path(path);
            if p.exists() {
                read_device_key_file(&p, passphrase).map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

/// The wrapped device-keypair file path: the DB key-file path plus a suffix.
fn device_key_path(db_key_path: &Path) -> PathBuf {
    let mut name = db_key_path.as_os_str().to_os_string();
    name.push(DEVICE_KEY_FILE_SUFFIX);
    PathBuf::from(name)
}

/// Store the device keypair in the OS keystore under its own account.
fn store_device_keypair_os(keypair: &[u8]) -> Result<()> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_KEY_USER)
        .map_err(|e| CoreError::Keystore(format!("device keyring entry: {e}")))?;
    entry
        .set_secret(keypair)
        .map_err(|e| CoreError::Keystore(format!("device keyring store: {e}")))?;
    Ok(())
}

/// Load the device keypair from the OS keystore, or `None` on first run.
fn load_device_keypair_os() -> Result<Option<Zeroizing<Vec<u8>>>> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_KEY_USER)
        .map_err(|e| CoreError::Keystore(format!("device keyring entry: {e}")))?;
    match entry.get_secret() {
        Ok(secret) => {
            if secret.len() != DEVICE_KEYPAIR_LEN {
                return Err(CoreError::Key(format!(
                    "stored device keypair is {} bytes, expected {DEVICE_KEYPAIR_LEN}",
                    secret.len()
                )));
            }
            Ok(Some(Zeroizing::new(secret)))
        }
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Keystore(format!("device keyring load: {e}"))),
    }
}

/// Wrap and atomically write the device keypair to its file (headless fallback).
fn write_device_key_file(path: &Path, passphrase: &str, keypair: &[u8]) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    fill_random(&mut nonce_bytes)?;

    let wrap = derive_wrap_key(passphrase, &salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), keypair)
        .map_err(|e| CoreError::Key(format!("device key wrap: {e}")))?;

    let mut blob =
        Vec::with_capacity(DEVICE_FILE_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(DEVICE_FILE_MAGIC);
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);

    write_atomic(path, &blob)?;
    Ok(())
}

/// Read and unwrap the device keypair file (headless fallback). A wrong
/// passphrase or malformed file fails closed.
fn read_device_key_file(path: &Path, passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    let blob = std::fs::read(path)?;
    let header = DEVICE_FILE_MAGIC.len() + SALT_LEN + NONCE_LEN;
    if blob.len() < header || &blob[..DEVICE_FILE_MAGIC.len()] != DEVICE_FILE_MAGIC {
        return Err(CoreError::Key("malformed device key file".to_string()));
    }
    let salt = &blob[DEVICE_FILE_MAGIC.len()..DEVICE_FILE_MAGIC.len() + SALT_LEN];
    let nonce = &blob[DEVICE_FILE_MAGIC.len() + SALT_LEN..header];
    let ciphertext = &blob[header..];

    let wrap = derive_wrap_key(passphrase, salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let plaintext = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| CoreError::Key("wrong passphrase or corrupt device key file".to_string()))?;
    if plaintext.len() != DEVICE_KEYPAIR_LEN {
        return Err(CoreError::Key(
            "device key file has wrong length".to_string(),
        ));
    }
    Ok(Zeroizing::new(plaintext))
}

// --- Persona-pack signing seed (C5 #27 P4) ----------------------------------
//
// The pack-signing key is this device's long-lived ed25519 identity for SIGNING
// the persona packs it exports. Per the issue it lives in the OS keystore, NOT
// in the SQLite plaintext, and is persisted through the same `KeySource` the
// store uses so the OS-keystore desktop path and the headless passphrase-file
// path both work and stay testable. Only the 32-byte SEED (the secret half) is
// persisted; the ed25519 public key is derived from it on load. The on-disk
// fallback wraps the seed under the passphrase with XChaCha20-Poly1305, exactly
// like the DB and pairing keys. The caller zeroizes the seed buffer.

/// Store the persona-pack signing seed through `source`, failing closed. The
/// seed must be exactly [`PACK_SEED_LEN`] bytes.
pub fn store_pack_signing_seed(source: &KeySource, seed: &[u8]) -> Result<()> {
    if seed.len() != PACK_SEED_LEN {
        return Err(CoreError::Key(format!(
            "pack signing seed is {} bytes, expected {PACK_SEED_LEN}",
            seed.len()
        )));
    }
    match source {
        KeySource::OsKeystore => store_pack_signing_seed_os(seed),
        KeySource::EncryptedFile { path, passphrase } => {
            write_pack_key_file(&pack_key_path(path), passphrase, seed)
        }
    }
}

/// Load the persona-pack signing seed through `source`, or `None` if none has
/// been stored yet (first run). A malformed or undecryptable seed fails closed.
pub fn load_pack_signing_seed(source: &KeySource) -> Result<Option<Zeroizing<Vec<u8>>>> {
    match source {
        KeySource::OsKeystore => load_pack_signing_seed_os(),
        KeySource::EncryptedFile { path, passphrase } => {
            let p = pack_key_path(path);
            if p.exists() {
                read_pack_key_file(&p, passphrase).map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

/// The wrapped pack-signing-seed file path: the DB key-file path plus a suffix.
fn pack_key_path(db_key_path: &Path) -> PathBuf {
    let mut name = db_key_path.as_os_str().to_os_string();
    name.push(PACK_KEY_FILE_SUFFIX);
    PathBuf::from(name)
}

/// Store the pack-signing seed in the OS keystore under its own account.
fn store_pack_signing_seed_os(seed: &[u8]) -> Result<()> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_PACK_KEY_USER)
        .map_err(|e| CoreError::Keystore(format!("pack keyring entry: {e}")))?;
    entry
        .set_secret(seed)
        .map_err(|e| CoreError::Keystore(format!("pack keyring store: {e}")))?;
    Ok(())
}

/// Load the pack-signing seed from the OS keystore, or `None` on first run.
fn load_pack_signing_seed_os() -> Result<Option<Zeroizing<Vec<u8>>>> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_PACK_KEY_USER)
        .map_err(|e| CoreError::Keystore(format!("pack keyring entry: {e}")))?;
    match entry.get_secret() {
        Ok(secret) => {
            if secret.len() != PACK_SEED_LEN {
                return Err(CoreError::Key(format!(
                    "stored pack signing seed is {} bytes, expected {PACK_SEED_LEN}",
                    secret.len()
                )));
            }
            Ok(Some(Zeroizing::new(secret)))
        }
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Keystore(format!("pack keyring load: {e}"))),
    }
}

/// Wrap and atomically write the pack-signing seed to its file (headless
/// fallback), under the passphrase with XChaCha20-Poly1305.
fn write_pack_key_file(path: &Path, passphrase: &str, seed: &[u8]) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    fill_random(&mut nonce_bytes)?;

    let wrap = derive_wrap_key(passphrase, &salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), seed)
        .map_err(|e| CoreError::Key(format!("pack key wrap: {e}")))?;

    let mut blob =
        Vec::with_capacity(PACK_FILE_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(PACK_FILE_MAGIC);
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);

    write_atomic(path, &blob)?;
    Ok(())
}

/// Read and unwrap the pack-signing-seed file (headless fallback). A wrong
/// passphrase or malformed file fails closed.
fn read_pack_key_file(path: &Path, passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    let blob = std::fs::read(path)?;
    let header = PACK_FILE_MAGIC.len() + SALT_LEN + NONCE_LEN;
    if blob.len() < header || &blob[..PACK_FILE_MAGIC.len()] != PACK_FILE_MAGIC {
        return Err(CoreError::Key("malformed pack key file".to_string()));
    }
    let salt = &blob[PACK_FILE_MAGIC.len()..PACK_FILE_MAGIC.len() + SALT_LEN];
    let nonce = &blob[PACK_FILE_MAGIC.len() + SALT_LEN..header];
    let ciphertext = &blob[header..];

    let wrap = derive_wrap_key(passphrase, salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let plaintext = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| CoreError::Key("wrong passphrase or corrupt pack key file".to_string()))?;
    if plaintext.len() != PACK_SEED_LEN {
        return Err(CoreError::Key("pack key file has wrong length".to_string()));
    }
    Ok(Zeroizing::new(plaintext))
}

// --- Per-persona proxy credentials (C7 #30 N1) ------------------------------
//
// Proxy CREDENTIALS are sourced from the OS keystore, NEVER the database and
// never a log line. They live under their own keyring service, keyed by a
// per-egress account label the persisted egress row carries as a non-secret
// marker. The headless fallback wraps the credential blob under the passphrase
// with XChaCha20-Poly1305, like the DB and pairing keys. The blob is
// `username\0password`; the NUL separator is rejected inside the username so the
// split is unambiguous. Decoded plaintext is held in Zeroizing buffers.

/// Separator byte between the username and password in the credential blob.
const PROXY_CRED_SEP: u8 = 0;

/// Encode a `(username, password)` pair into the stored blob form (held in a
/// Zeroizing buffer so the plaintext is scrubbed on drop).
fn encode_proxy_cred(username: &str, password: &str) -> Result<Zeroizing<Vec<u8>>> {
    if username.as_bytes().contains(&PROXY_CRED_SEP) {
        return Err(CoreError::Key(
            "proxy username may not contain a NUL byte".to_string(),
        ));
    }
    let mut blob = Zeroizing::new(Vec::with_capacity(username.len() + 1 + password.len()));
    blob.extend_from_slice(username.as_bytes());
    blob.push(PROXY_CRED_SEP);
    blob.extend_from_slice(password.as_bytes());
    Ok(blob)
}

/// Decode a stored credential blob back into a Zeroizing `(username, password)`.
/// A blob with no separator fails closed.
fn decode_proxy_cred(blob: &[u8]) -> Result<(Zeroizing<String>, Zeroizing<String>)> {
    let sep = blob
        .iter()
        .position(|b| *b == PROXY_CRED_SEP)
        .ok_or_else(|| CoreError::Key("malformed proxy credential blob".to_string()))?;
    let username = decode_proxy_field(&blob[..sep], "proxy username")?;
    let password = decode_proxy_field(&blob[sep + 1..], "proxy password")?;
    Ok((username, password))
}

/// Decode one credential field straight into a `Zeroizing<String>`.
///
/// `String::from_utf8` reuses the owned `Vec`'s buffer, so the plaintext only
/// ever lives in the allocation that `Zeroizing` wipes on drop. The earlier
/// `str::from_utf8(..).to_string()` left an intermediate, un-zeroized `String`
/// holding the plaintext until it dropped.
fn decode_proxy_field(bytes: &[u8], what: &str) -> Result<Zeroizing<String>> {
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| CoreError::Key(format!("{what} is not valid UTF-8")))
}

/// Store an egress proxy's credentials under `account_label` through `source`,
/// failing closed. The secret never touches the DB or a log.
pub fn store_proxy_credentials(
    source: &KeySource,
    account_label: &str,
    username: &str,
    password: &str,
) -> Result<()> {
    let blob = encode_proxy_cred(username, password)?;
    match source {
        KeySource::OsKeystore => store_proxy_credentials_os(account_label, &blob),
        KeySource::EncryptedFile { path, passphrase } => {
            write_proxy_cred_file(&proxy_cred_path(path, account_label), passphrase, &blob)
        }
    }
}

/// Load an egress proxy's credentials for `account_label` through `source`, or
/// `None` if none has been stored. The returned secrets are Zeroizing. A
/// malformed or undecryptable blob fails closed.
#[allow(clippy::type_complexity)]
pub fn load_proxy_credentials(
    source: &KeySource,
    account_label: &str,
) -> Result<Option<(Zeroizing<String>, Zeroizing<String>)>> {
    match source {
        KeySource::OsKeystore => load_proxy_credentials_os(account_label),
        KeySource::EncryptedFile { path, passphrase } => {
            let p = proxy_cred_path(path, account_label);
            if p.exists() {
                let blob = read_proxy_cred_file(&p, passphrase)?;
                Ok(Some(decode_proxy_cred(&blob)?))
            } else {
                Ok(None)
            }
        }
    }
}

/// Delete an egress proxy's stored credentials for `account_label`. Returns
/// `true` if something was removed. Used when an egress is cleared so a secret
/// does not outlive its config.
pub fn delete_proxy_credentials(source: &KeySource, account_label: &str) -> Result<bool> {
    match source {
        KeySource::OsKeystore => delete_proxy_credentials_os(account_label),
        KeySource::EncryptedFile { path, .. } => {
            let p = proxy_cred_path(path, account_label);
            if p.exists() {
                std::fs::remove_file(&p)?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
}

/// The wrapped proxy-credential file path: the DB key-file path plus the
/// sanitized label and suffix.
fn proxy_cred_path(db_key_path: &Path, account_label: &str) -> PathBuf {
    let safe: String = account_label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut name = db_key_path.as_os_str().to_os_string();
    name.push(format!("{PROXY_CRED_FILE_SUFFIX}.{safe}"));
    PathBuf::from(name)
}

/// Store proxy credentials in the OS keystore under their per-label account.
fn store_proxy_credentials_os(account_label: &str, blob: &[u8]) -> Result<()> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_PROXY_SERVICE, account_label)
        .map_err(|e| CoreError::Keystore(format!("proxy keyring entry: {e}")))?;
    entry
        .set_secret(blob)
        .map_err(|e| CoreError::Keystore(format!("proxy keyring store: {e}")))?;
    Ok(())
}

/// Load proxy credentials from the OS keystore, or `None` on first use.
#[allow(clippy::type_complexity)]
fn load_proxy_credentials_os(
    account_label: &str,
) -> Result<Option<(Zeroizing<String>, Zeroizing<String>)>> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_PROXY_SERVICE, account_label)
        .map_err(|e| CoreError::Keystore(format!("proxy keyring entry: {e}")))?;
    match entry.get_secret() {
        Ok(mut secret) => {
            let decoded = decode_proxy_cred(&secret);
            secret.zeroize();
            decoded.map(Some)
        }
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Keystore(format!("proxy keyring load: {e}"))),
    }
}

/// Delete proxy credentials from the OS keystore. Returns `true` if a credential
/// existed and was removed.
fn delete_proxy_credentials_os(account_label: &str) -> Result<bool> {
    ensure_default_store()?;
    let entry = keyring_core::Entry::new(KEYRING_PROXY_SERVICE, account_label)
        .map_err(|e| CoreError::Keystore(format!("proxy keyring entry: {e}")))?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring_core::Error::NoEntry) => Ok(false),
        Err(e) => Err(CoreError::Keystore(format!("proxy keyring delete: {e}"))),
    }
}

/// Wrap and atomically write a proxy-credential blob to its file (headless
/// fallback), under the passphrase with XChaCha20-Poly1305.
fn write_proxy_cred_file(path: &Path, passphrase: &str, blob: &[u8]) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    fill_random(&mut nonce_bytes)?;

    let wrap = derive_wrap_key(passphrase, &salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), blob)
        .map_err(|e| CoreError::Key(format!("proxy cred wrap: {e}")))?;

    let mut out =
        Vec::with_capacity(PROXY_FILE_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(PROXY_FILE_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);

    write_atomic(path, &out)?;
    Ok(())
}

/// Read and unwrap a proxy-credential file (headless fallback). A wrong
/// passphrase or malformed file fails closed.
fn read_proxy_cred_file(path: &Path, passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    let blob = std::fs::read(path)?;
    let header = PROXY_FILE_MAGIC.len() + SALT_LEN + NONCE_LEN;
    if blob.len() < header || &blob[..PROXY_FILE_MAGIC.len()] != PROXY_FILE_MAGIC {
        return Err(CoreError::Key(
            "malformed proxy credential file".to_string(),
        ));
    }
    let salt = &blob[PROXY_FILE_MAGIC.len()..PROXY_FILE_MAGIC.len() + SALT_LEN];
    let nonce = &blob[PROXY_FILE_MAGIC.len() + SALT_LEN..header];
    let ciphertext = &blob[header..];

    let wrap = derive_wrap_key(passphrase, salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let plaintext = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| {
            CoreError::Key("wrong passphrase or corrupt proxy credential file".to_string())
        })?;
    Ok(Zeroizing::new(plaintext))
}

/// Generate a fresh key, wrap it under the passphrase, and atomically write the
/// key file. Returns the new key.
fn create_key_file(path: &Path, passphrase: &str) -> Result<DbKey> {
    let key = DbKey::generate()?;

    let mut salt = [0u8; SALT_LEN];
    fill_random(&mut salt)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    fill_random(&mut nonce_bytes)?;

    let wrap = derive_wrap_key(passphrase, &salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), key.as_bytes().as_slice())
        .map_err(|e| CoreError::Key(format!("key wrap: {e}")))?;

    // File layout: MAGIC(8) || salt(16) || nonce(24) || ciphertext(48).
    let mut blob = Vec::with_capacity(FILE_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(FILE_MAGIC);
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);

    write_atomic(path, &blob)?;
    Ok(key)
}

/// Read and unwrap the key file with the passphrase. A wrong passphrase fails
/// AEAD authentication and returns an error (fail closed).
fn read_key_file(path: &Path, passphrase: &str) -> Result<DbKey> {
    let blob = std::fs::read(path)?;
    let header = FILE_MAGIC.len() + SALT_LEN + NONCE_LEN;
    if blob.len() < header || &blob[..FILE_MAGIC.len()] != FILE_MAGIC {
        return Err(CoreError::Key("malformed key file".to_string()));
    }
    let salt = &blob[FILE_MAGIC.len()..FILE_MAGIC.len() + SALT_LEN];
    let nonce = &blob[FILE_MAGIC.len() + SALT_LEN..header];
    let ciphertext = &blob[header..];

    let wrap = derive_wrap_key(passphrase, salt)?;
    let cipher = XChaCha20Poly1305::new(wrap.as_slice().into());
    let mut plaintext = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| CoreError::Key("wrong passphrase or corrupt key file".to_string()))?;

    let key = key_from_secret(&plaintext);
    plaintext.zeroize();
    key
}

/// Write `data` to `path` atomically (write a temp sibling, then rename) so a
/// crash never leaves a half-written key file.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::Key("key-file path has no parent directory".to_string()))?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile_in(parent)?;
    tmp.write_all(data)?;
    tmp.flush()?;
    // Persist (rename) the temp file onto the final path.
    tmp.persist(path).map_err(|e| CoreError::Io(e.error))?;
    Ok(())
}

/// Create a named temporary file in `dir`. Factored out so [`write_atomic`]
/// stays readable; uses the std-only approach to avoid pulling `tempfile` into
/// the non-test dependency set.
fn tempfile_in(dir: &Path) -> Result<NamedTemp> {
    NamedTemp::new(dir)
}

/// Minimal atomic-write helper: a uniquely named file that is renamed into
/// place on `persist` and removed on drop otherwise. Avoids a runtime dep on
/// `tempfile` for production code (tests use the real `tempfile` crate).
struct NamedTemp {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl NamedTemp {
    fn new(dir: &Path) -> Result<Self> {
        // A name unique enough for a single-writer key file: pid + nanos.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(".keyfile.{}.{nanos}.tmp", std::process::id());
        let path = dir.join(name);
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.write_all(data)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
        }
        Ok(())
    }

    fn persist(mut self, dest: &Path) -> std::result::Result<(), PersistError> {
        // Close the handle before rename for cross-platform safety.
        self.file = None;
        match std::fs::rename(&self.path, dest) {
            Ok(()) => {
                // Mark as persisted so Drop does not remove the destination.
                self.path = PathBuf::new();
                Ok(())
            }
            Err(error) => Err(PersistError { error }),
        }
    }
}

impl Drop for NamedTemp {
    fn drop(&mut self) {
        self.file = None;
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Error returned when [`NamedTemp::persist`] cannot rename the temp file.
struct PersistError {
    error: std::io::Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_key_round_trips_with_same_passphrase() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("key.bin");
        let src = KeySource::EncryptedFile {
            path: path.clone(),
            passphrase: "correct horse battery staple".to_string(),
        };
        let first = src.load_or_create()?;
        let again = src.load_or_create()?;
        assert_eq!(*first.to_hex(), *again.to_hex());
        Ok(())
    }

    #[test]
    fn wrong_passphrase_fails_closed() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("key.bin");
        KeySource::EncryptedFile {
            path: path.clone(),
            passphrase: "right".to_string(),
        }
        .load_or_create()?;

        let wrong = KeySource::EncryptedFile {
            path,
            passphrase: "wrong".to_string(),
        }
        .load_or_create();
        assert!(matches!(wrong, Err(CoreError::Key(_))));
        Ok(())
    }

    #[test]
    fn malformed_file_is_rejected() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("key.bin");
        std::fs::write(&path, b"not a key file")?;
        let res = KeySource::EncryptedFile {
            path,
            passphrase: "x".to_string(),
        }
        .load_or_create();
        assert!(matches!(res, Err(CoreError::Key(_))));
        Ok(())
    }

    #[test]
    fn pack_signing_seed_round_trips_through_encrypted_file() -> Result<()> {
        let dir = tempdir()?;
        let src = KeySource::EncryptedFile {
            path: dir.path().join("key.bin"),
            passphrase: "pack-pass".to_string(),
        };
        // None before anything is stored.
        assert!(load_pack_signing_seed(&src)?.is_none());

        let seed = [9u8; PACK_SEED_LEN];
        store_pack_signing_seed(&src, &seed)?;
        let back = load_pack_signing_seed(&src)?
            .ok_or_else(|| CoreError::Key("seed missing after store".into()))?;
        assert_eq!(back.as_slice(), &seed[..]);

        // Wrong length fails closed.
        assert!(store_pack_signing_seed(&src, &[0u8; 10]).is_err());

        // Wrong passphrase fails closed on load.
        let wrong = KeySource::EncryptedFile {
            path: dir.path().join("key.bin"),
            passphrase: "nope".to_string(),
        };
        assert!(matches!(
            load_pack_signing_seed(&wrong),
            Err(CoreError::Key(_))
        ));
        Ok(())
    }

    #[test]
    fn proxy_credentials_round_trip_through_encrypted_file() -> Result<()> {
        let dir = tempdir()?;
        let src = KeySource::EncryptedFile {
            path: dir.path().join("key.bin"),
            passphrase: "proxy-pass".to_string(),
        };
        let label = "persona-1-egress";
        // None before anything is stored.
        assert!(load_proxy_credentials(&src, label)?.is_none());

        store_proxy_credentials(&src, label, "egress-user", "s3cr3t-pw")?;
        let (user, pw) = load_proxy_credentials(&src, label)?
            .ok_or_else(|| CoreError::Key("creds missing after store".into()))?;
        assert_eq!(user.as_str(), "egress-user");
        assert_eq!(pw.as_str(), "s3cr3t-pw");

        // Wrong passphrase fails closed on load.
        let wrong = KeySource::EncryptedFile {
            path: dir.path().join("key.bin"),
            passphrase: "nope".to_string(),
        };
        assert!(matches!(
            load_proxy_credentials(&wrong, label),
            Err(CoreError::Key(_))
        ));

        // Delete removes it; a second delete is a clean false.
        assert!(delete_proxy_credentials(&src, label)?);
        assert!(load_proxy_credentials(&src, label)?.is_none());
        assert!(!delete_proxy_credentials(&src, label)?);
        Ok(())
    }

    #[test]
    fn hex_is_64_lowercase_chars() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("key.bin");
        let key = KeySource::EncryptedFile {
            path,
            passphrase: "pw".to_string(),
        }
        .load_or_create()?;
        let hex = key.to_hex();
        assert_eq!(hex.len(), 64);
        assert!(hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        Ok(())
    }
}
