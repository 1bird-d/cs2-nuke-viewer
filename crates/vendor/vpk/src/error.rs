use std::path::PathBuf;

/// Errors produced while opening or reading a VPK archive.
#[derive(Debug, thiserror::Error)]
pub enum VpkError {
    /// An I/O error, annotated with the file that caused it.
    #[error("i/o error on {path}: {source}")]
    Io {
        /// File the operation was performed on.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The file does not start with the VPK signature `0x55AA1234`.
    #[error("not a VPK archive: expected signature 0x55AA1234, found {found:#010X}")]
    BadSignature {
        /// The four bytes that were found instead.
        found: u32,
    },

    /// The archive declares a version this reader does not implement.
    ///
    /// Notably `0x00030002` is Respawn's (Titanfall/Apex) fork, which is a
    /// different format despite sharing the signature.
    #[error("unsupported VPK version {version} ({note})")]
    UnsupportedVersion {
        /// Version field from the header.
        version: u32,
        /// Human-readable explanation.
        note: &'static str,
    },

    /// The file ended in the middle of a structure.
    #[error(
        "unexpected end of archive while reading {what} (needed {needed} bytes at offset {offset})"
    )]
    UnexpectedEof {
        /// What was being parsed.
        what: &'static str,
        /// Bytes required.
        needed: u64,
        /// Offset at which the read was attempted.
        offset: u64,
    },

    /// A directory entry did not end with the `0xFFFF` terminator.
    #[error("invalid directory entry terminator for {path}: expected 0xFFFF, found {found:#06X}")]
    BadTerminator {
        /// Path of the entry being parsed.
        path: String,
        /// Terminator value found.
        found: u16,
    },

    /// A string in the directory tree was not valid UTF-8.
    #[error("invalid UTF-8 in directory tree at offset {offset}")]
    InvalidUtf8 {
        /// Offset of the offending string.
        offset: u64,
    },

    /// The directory tree ran past the region the header reserved for it.
    #[error("directory tree overruns its declared size ({tree_size} bytes)")]
    TreeOverrun {
        /// Declared tree size from the header.
        tree_size: u32,
    },

    /// Requested a path that is not in the archive.
    #[error("file not found in archive: {path}")]
    NotFound {
        /// The path that was requested.
        path: String,
    },

    /// A multi-chunk archive referenced a chunk file that is missing.
    #[error("archive chunk {index:03} not found (expected at {path})")]
    MissingChunk {
        /// Archive index from the entry.
        index: u16,
        /// Path the chunk was expected at.
        path: PathBuf,
    },

    /// CRC verification was requested and the data did not match.
    #[error("CRC32 mismatch for {path}: expected {expected:#010X}, computed {actual:#010X}")]
    CrcMismatch {
        /// Path of the entry.
        path: String,
        /// CRC recorded in the directory.
        expected: u32,
        /// CRC computed over the data actually read.
        actual: u32,
    },

    /// An entry's offset/length pair does not fit inside its archive file.
    #[error("entry {path} is out of bounds: offset {offset} + length {length} exceeds archive size {archive_len}")]
    EntryOutOfBounds {
        /// Path of the entry.
        path: String,
        /// Entry offset within its archive.
        offset: u64,
        /// Entry length.
        length: u64,
        /// Size of the archive file the entry points into.
        archive_len: u64,
    },
}

/// Convenience result alias for this crate.
pub type Result<T> = std::result::Result<T, VpkError>;
