use crate::nbt::NbtError;

/// Why parsing failed. The `Display` text is meant for developers; messages for end users are up to the caller.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Neither a compressed Java structure nor a file name that tells the edition; or Java NBT that is neither a vanilla
    /// structure nor a Litematica schematic (for example `.schem`).
    #[error("unrecognized file format (expected a .mcstructure or .nbt exported by the game, or a Litematica .litematic)")]
    UnknownFormat,
    /// An NBT-level error (truncated, wrong type, over a limit, …) with the byte offset and the tag path.
    #[error(transparent)]
    Nbt(#[from] NbtError),
    /// Decompression failed, or the decompressed data exceeded the limit.
    #[error("decompression failed: {0}")]
    Inflate(String),
    /// The NBT is readable but does not describe a valid structure.
    #[error("{path}: {message}")]
    Invalid {
        /// The offending field, such as `structure.block_indices`.
        path: String,
        /// What is wrong with it.
        message: String,
    },
    /// A resource limit was exceeded.
    #[error("{0}")]
    TooLarge(String),
}

impl Error {
    pub(crate) fn invalid(path: &str, message: impl Into<String>) -> Self {
        Self::Invalid { path: path.to_owned(), message: message.into() }
    }
}
