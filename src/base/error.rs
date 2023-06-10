#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Key not set (secret key or public key)
    #[error("Key not set")]
    KeyNotSet,
    /// No unsaved changes to save
    #[error("No changes to save")]
    KeyNoChangeToSave,
    /// Saving not allowed
    #[error("Saving not allowed, check settings")]
    KeySaveNotAllowed,
    /// Loading not allowed
    #[error("Loading not allowed, check settings")]
    KeyLoadNotAllowed,
    /// Key error
    #[error(transparent)]
    KeyError(#[from] nostr::key::Error),
    /// Secp256k1 key error
    #[error(transparent)]
    KeyErrorSecp256k1(#[from] nostr::secp256k1::Error),
    /// Invalid encrypted key
    #[error("Invalid encrypted key")]
    KeyInvalidEncrypted,
    /// Encryption error
    #[error("Encryption error")]
    KeyEncryption,
    /// Invalid encryption version
    #[error("Invalid encryption version")]
    KeyInvalidEncryptionVersion,
    /// Mandatory encryption password missing
    #[error("Mandatory encryption password missing. Check password and security settings")]
    KeyEncryptionPasswordMissing,
    /// Encryption passwords don't match
    #[error("Encryption passwords don't match")]
    KeyEncryptionPasswordMismatch,
    /// Invalid BIP39 mnemonic
    #[error(transparent)]
    KeyMnemonic(#[from] bip39::Error),
    /// BIP32 key derivation error
    #[error(transparent)]
    KeyDerivation(#[from] bip32::Error),
    /// Nip19 error
    #[error(transparent)]
    SignatureError(#[from] nostr::nips::nip19::Error),
    /// Nip26 error
    #[error(transparent)]
    Nip26Error(#[from] nostr::nips::nip26::Error),
    /// IO error, e.g. file/folder error
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    /// JSON serialization error
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),
    /// Nostr connect error (NIP-46)
    #[error(transparent)]
    Nip46Error(#[from] nostr::nips::nip46::Error),
    /// Relay client error
    #[error(transparent)]
    RelayClientError(#[from] nostr_sdk::client::Error),
    /// Signer is already connected, disconnect first
    #[error("Signer is already connected, disconnect first")]
    SignerAlreadyConnected,
    /// Internal event queue receive error
    #[error(transparent)]
    InternalEventQueueReceive(#[from] crossbeam::channel::RecvError),
    /// Internal event queue send error
    #[error("Internal event queue send error")]
    InternalEventQueueSend,
    /// Internal EventBuilder error
    #[error(transparent)]
    EventBuilderError(#[from] nostr::event::builder::Error),
}
