/// Errors produced while decoding Source 2 resources.
#[derive(Debug, thiserror::Error)]
pub enum Source2Error {
    /// A read ran past the end of the buffer.
    #[error("unexpected end of data while reading {what}: needed {needed} bytes at offset {offset}, {available} available")]
    UnexpectedEof {
        /// What was being parsed.
        what: &'static str,
        /// Bytes required.
        needed: usize,
        /// Offset the read was attempted at.
        offset: usize,
        /// Bytes remaining in the buffer.
        available: usize,
    },

    /// The resource header version was not the expected value.
    #[error("not a Source 2 compiled resource: header version is {found}, expected {expected}")]
    BadHeaderVersion {
        /// Value found in the file.
        found: u16,
        /// Value the format requires (12).
        expected: u16,
    },

    /// The file looked like a VPK archive rather than a resource.
    #[error("this is a VPK archive, not a compiled resource")]
    IsVpkArchive,

    /// The block directory pointed outside the file.
    #[error("block {fourcc} is out of bounds: offset {offset} + size {size} exceeds file length {file_len}")]
    BlockOutOfBounds {
        /// FourCC of the offending block.
        fourcc: String,
        /// Block offset.
        offset: u64,
        /// Block size.
        size: u64,
        /// Length of the file.
        file_len: usize,
    },

    /// The declared block count is implausible.
    #[error("implausible block count {count}")]
    BadBlockCount {
        /// Count read from the header.
        count: u32,
    },

    /// The KV3 magic was not recognised.
    #[error("unrecognised KV3 signature {magic:#010X}")]
    BadKv3Magic {
        /// The four bytes found.
        magic: u32,
    },

    /// The KV3 version is outside the supported 0..=5 range.
    #[error("unsupported KV3 version {version} (supported: 0 through 5)")]
    UnsupportedKv3Version {
        /// Version nibble from the magic.
        version: u32,
    },

    /// A KV3 encoding GUID that this decoder does not implement.
    #[error("unsupported legacy KV3 encoding {0}")]
    UnsupportedKv3Encoding(String),

    /// The compression method byte was not 0, 1 or 2.
    #[error("unknown KV3 compression method {method}")]
    UnknownCompression {
        /// Method value from the header.
        method: u32,
    },

    /// A compression parameter combination this decoder does not implement.
    #[error("unhandled KV3 compression parameter {name} = {value}")]
    UnhandledCompressionParam {
        /// Parameter name.
        name: &'static str,
        /// Value found.
        value: u32,
    },

    /// Decompression failed or produced the wrong number of bytes.
    #[error("{algorithm} decompression failed: {detail}")]
    Decompress {
        /// `LZ4` or `zstd`.
        algorithm: &'static str,
        /// Description of the failure.
        detail: String,
    },

    /// A structural sentinel did not have the expected value.
    #[error("bad {what} trailer: expected {expected:#010X}, found {found:#010X}")]
    BadTrailer {
        /// Which trailer.
        what: &'static str,
        /// Expected value.
        expected: u32,
        /// Value found.
        found: u32,
    },

    /// A size or count field was negative or absurdly large.
    #[error("invalid {what} value {value}")]
    InvalidSize {
        /// Field name.
        what: &'static str,
        /// Value found.
        value: i64,
    },

    /// A declared buffer size exceeds the decoder's allocation limit.
    #[error("{what} of {value} bytes exceeds the {limit} byte limit")]
    AllocationTooLarge {
        /// Field name.
        what: &'static str,
        /// Requested size.
        value: u64,
        /// Configured limit.
        limit: u64,
    },

    /// A KV3 node type byte was not a known type.
    #[error("unknown KV3 node type {value}")]
    UnknownNodeType {
        /// Type byte.
        value: u8,
    },

    /// A KV3 flag byte was not a known flag.
    #[error("unknown KV3 flag {value}")]
    UnknownFlag {
        /// Flag byte.
        value: u8,
    },

    /// A string index referred to a slot outside the string table.
    #[error("string index {index} out of range ({count} strings)")]
    StringIndexOutOfRange {
        /// Index found in the data.
        index: i32,
        /// Number of strings in the table.
        count: usize,
    },

    /// A string in the KV3 string table was not valid UTF-8.
    #[error("invalid UTF-8 in KV3 string table")]
    InvalidUtf8,

    /// A meshoptimizer-compressed buffer failed to decode.
    #[error("meshopt {codec} buffer decode failed: {detail}")]
    Meshopt {
        /// Which codec failed: `vertex` or `index`.
        codec: &'static str,
        /// Description of the failure.
        detail: String,
    },

    /// The value tree nested deeper than the decoder allows.
    #[error("KV3 value nesting exceeded the depth limit of {limit}")]
    DepthLimitExceeded {
        /// Configured limit.
        limit: usize,
    },

    /// A block the decoder needs was not in the resource.
    #[error("resource has no {fourcc} block")]
    MissingBlock {
        /// FourCC of the missing block.
        fourcc: &'static str,
    },

    /// A block was present but not in the KV3 encoding the decoder expects.
    #[error("{fourcc} block is not binary KV3")]
    NotKv3 {
        /// FourCC of the offending block.
        fourcc: &'static str,
    },

    /// The `vtex` header version was not 1.
    #[error("unsupported vtex version {version} (supported: 1)")]
    UnsupportedTextureVersion {
        /// Version found in the header.
        version: u16,
    },

    /// The texture's pixel format has no decoder here.
    #[error("unsupported texture format {format} ({value})")]
    UnsupportedTextureFormat {
        /// Valve's name for the format.
        format: &'static str,
        /// Debug rendering of the parsed format.
        value: String,
    },

    /// The legacy entity key-values blob had an unexpected version word.
    #[error("unsupported entity key-values blob version {version} (supported: 1)")]
    UnsupportedEntityBlobVersion {
        /// Version found at the start of the blob.
        version: u32,
    },

    /// An entity field used a type whose payload width is not known, so the
    /// rest of the blob cannot be walked.
    #[error("unknown entity field type {value:#x}")]
    UnknownEntityFieldType {
        /// Value of the type word.
        value: u32,
    },

    /// A KV3 payload was missing a field the decoder needs.
    #[error("{resource} is missing {what}")]
    MissingField {
        /// Name of the field, or a description of what was expected.
        what: &'static str,
        /// Which kind of resource was being decoded.
        resource: &'static str,
    },

    /// An animation frame was requested that the clip does not have.
    #[error("animation frame {index} out of range ({count} frames)")]
    FrameOutOfRange {
        /// Frame requested.
        index: usize,
        /// Frames the clip has.
        count: usize,
    },

    /// A frame's tracks ran past the end of the compressed pose blob.
    #[error("compressed pose data ends part way through frame {frame}")]
    ClipDataTruncated {
        /// Which frame could not be completed.
        frame: usize,
    },

    /// A `.nav` file did not begin with `0xFEEDFACE`.
    #[error("not a navigation mesh: magic is {magic:#010X}, expected FEEDFACE")]
    BadNavMagic {
        /// The four bytes found.
        magic: u32,
    },

    /// A `.nav` version outside the range this reader implements.
    #[error("unsupported nav version {version} (supported: 30 through 36)")]
    UnsupportedNavVersion {
        /// Version found in the header.
        version: u32,
    },

    /// A `.nav` index pointed outside the table it addresses.
    #[error("nav {what} index {index} out of range ({count} entries)")]
    NavIndexOutOfRange {
        /// Which table.
        what: &'static str,
        /// Index found in the data.
        index: u32,
        /// Size of the table.
        count: usize,
    },

    /// A mip level was requested that the texture does not have.
    #[error("mip level {level} out of range ({count} levels)")]
    MipOutOfRange {
        /// Level requested.
        level: u32,
        /// Levels the texture stores.
        count: u32,
    },
}

/// Convenience result alias for this crate.
pub type Result<T> = std::result::Result<T, Source2Error>;
