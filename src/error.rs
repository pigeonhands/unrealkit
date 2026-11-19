#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported UE version: {0}")]
    UnsupportedVersion(String),
    #[error("invalid GNames pointer")]
    InvalidGNames,
    #[error("invalid GObjects pointer")]
    InvalidGObjects,
    #[error("FName index {0} out of range")]
    FNameOutOfRange(u32),
    #[error("null pointer encountered reading {0}")]
    NullPointer(&'static str),
}
