//! The History key: an age X25519 identity that seals every Clip's content,
//! plus a keyed-hash key derived from it for dedup and blob names.

use age::secrecy::ExposeSecret as _;
use age::x25519::Identity;
use zeroize::Zeroizing;

use crate::{Error, Result};

/// blake3 `derive_key` context for the dedup / blob-name hash key.
const DEDUP_CONTEXT: &str = "Hoarder 2026-11 History dedup and blob-name key";

/// The key an encrypted History is sealed with. Its [`secret`](Key::secret)
/// is what the keyring holds; nothing of it is ever written to the History.
#[derive(Clone)]
pub struct Key {
    identity: Identity,
    dedup: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Key")
            .field("recipient", &self.recipient())
            .finish_non_exhaustive()
    }
}

impl Key {
    /// A fresh random key.
    pub fn generate() -> Key {
        Key::from_identity(Identity::generate())
    }

    /// The key from its secret string (`AGE-SECRET-KEY-1…`).
    pub fn parse(secret: &str) -> Result<Key> {
        secret
            .trim()
            .parse::<Identity>()
            .map(Key::from_identity)
            .map_err(|_| Error::Crypto("the History key is not an age X25519 identity".into()))
    }

    /// The secret string to keep in the keyring.
    pub fn secret(&self) -> Zeroizing<String> {
        Zeroizing::new(self.identity.to_string().expose_secret().to_owned())
    }

    fn from_identity(identity: Identity) -> Key {
        let secret = identity.to_string();
        let dedup = Zeroizing::new(blake3::derive_key(
            DEDUP_CONTEXT,
            secret.expose_secret().as_bytes(),
        ));
        Key { identity, dedup }
    }

    /// The public half (`age1…`), stored to tell a wrong key from the right one.
    pub(crate) fn recipient(&self) -> String {
        self.identity.to_public().to_string()
    }

    /// The key for keyed blake3 hashes, so hashes on disk confirm nothing.
    pub(crate) fn dedup(&self) -> &[u8; 32] {
        &self.dedup
    }

    pub(crate) fn seal(&self, plain: &[u8]) -> Result<Vec<u8>> {
        age::encrypt(&self.identity.to_public(), plain).map_err(|e| Error::Crypto(e.to_string()))
    }

    pub(crate) fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        age::decrypt(&self.identity, sealed).map_err(|e| Error::Crypto(e.to_string()))
    }
}

/// The content hash of `data`: plain blake3, or keyed under encryption.
pub(crate) fn hash(key: Option<&Key>, data: &[u8]) -> [u8; 32] {
    match key {
        Some(key) => *blake3::keyed_hash(key.dedup(), data).as_bytes(),
        None => *blake3::hash(data).as_bytes(),
    }
}

/// A hasher for a Clip's identity, keyed under encryption.
pub(crate) fn hasher(key: Option<&Key>) -> blake3::Hasher {
    match key {
        Some(key) => blake3::Hasher::new_keyed(key.dedup()),
        None => blake3::Hasher::new(),
    }
}
