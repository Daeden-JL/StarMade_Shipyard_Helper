//! What can go wrong, said in sentences a player can read.

/// Why a file could not be read or turned into blocks. Every message is a
/// full sentence meant to be shown to the player as it is.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ImportError {
    /// Nothing about the file says what it is.
    #[error("{0}")]
    UnknownFormat(String),
    /// A known kind of file that cannot be read; the message says what to do.
    #[error("{0}")]
    Unsupported(String),
    /// The file is cut short or does not follow its format.
    #[error("{0}")]
    Malformed(String),
    /// The file or the result is beyond a limit.
    #[error("{0}")]
    TooLarge(String),
    /// The model needs a second file that was not uploaded with it.
    #[error("{0}")]
    Missing(String),
    /// The file was read but holds nothing to build from.
    #[error("{0}")]
    Empty(String),
}

pub type Result<T> = std::result::Result<T, ImportError>;

pub(crate) fn malformed(message: impl Into<String>) -> ImportError {
    ImportError::Malformed(message.into())
}

pub(crate) fn too_large(message: impl Into<String>) -> ImportError {
    ImportError::TooLarge(message.into())
}

pub(crate) fn unsupported(message: impl Into<String>) -> ImportError {
    ImportError::Unsupported(message.into())
}

/// The error for a file that ends before its contents do.
pub(crate) fn cut_short(format: &str) -> ImportError {
    malformed(format!("The {format} file is cut short or damaged."))
}
