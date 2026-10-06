/// Why a key operation failed. The variants are the classes the node must tell apart: only `NotFound` may ever lead to creating
/// something, and none of them carries a path, a key or a raw crypto message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRootError {
    /// The file (or the whole identity) is absent.
    NotFound,
    /// The ciphertext did not authenticate: a wrong key, a wrong machine binding, or a modified file.
    DecryptFailed,
    /// Not parseable, wrong lengths, inconsistent fields.
    Corrupt,
    /// A format version this code does not know.
    UnsupportedVersion(u32),
    /// A file or directory that is not safe to use (a link, another owner, readable by others).
    PermissionDenied,
    /// Any other I/O failure.
    Io,
    /// The device cannot open the root any more (new machine, lost device key): the recovery password is required.
    RecoveryRequired,
    /// The recovery password did not open the root.
    RecoveryFailed,
    /// The stored KDF parameters are weaker than the policy allows (a downgrade), or unusable.
    WeakParameters,
    /// A caller mistake (e.g. a recovery password that is too short).
    Invalid(&'static str),
    /// Creating something was refused because other key material exists that this code cannot open.
    Refused(&'static str),
}

impl KeyRootError {
    /// The stable category shown outside (contract-style: no paths, no keys, no raw crypto errors).
    pub fn category(&self) -> &'static str {
        match self {
            KeyRootError::NotFound => "identity_not_initialized",
            KeyRootError::DecryptFailed => "identity_locked",
            KeyRootError::Corrupt => "identity_corrupt",
            KeyRootError::UnsupportedVersion(_) => "identity_format_unsupported",
            KeyRootError::PermissionDenied | KeyRootError::Io => "identity_unreadable",
            KeyRootError::RecoveryRequired => "identity_recovery_required",
            KeyRootError::RecoveryFailed => "identity_recovery_failed",
            KeyRootError::WeakParameters => "identity_corrupt",
            KeyRootError::Invalid(_) => "identity_invalid_request",
            KeyRootError::Refused(_) => "identity_recovery_required",
        }
    }
}

impl std::fmt::Display for KeyRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyRootError::NotFound => write!(f, "no identity has been created here"),
            KeyRootError::DecryptFailed => write!(f, "the key material could not be opened (wrong key, or the file was changed)"),
            KeyRootError::Corrupt => write!(f, "the key file is damaged"),
            KeyRootError::UnsupportedVersion(v) => write!(f, "the key file has a format version this program does not know ({v})"),
            KeyRootError::PermissionDenied => write!(f, "a key file or directory is not safe to use (link, other owner, or open permissions)"),
            KeyRootError::Io => write!(f, "a key file could not be read or written"),
            KeyRootError::RecoveryRequired => write!(f, "this device cannot open the key by itself; the recovery password is required"),
            KeyRootError::RecoveryFailed => write!(f, "the recovery password did not open the key"),
            KeyRootError::WeakParameters => write!(f, "the stored key-derivation settings are too weak or unusable"),
            KeyRootError::Invalid(why) => write!(f, "{why}"),
            KeyRootError::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for KeyRootError {}

impl From<std::io::Error> for KeyRootError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => KeyRootError::NotFound,
            std::io::ErrorKind::PermissionDenied => KeyRootError::PermissionDenied,
            _ => KeyRootError::Io,
        }
    }
}

pub type Result<T> = std::result::Result<T, KeyRootError>;
