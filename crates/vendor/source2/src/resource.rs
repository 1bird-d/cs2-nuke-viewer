//! The outer structure of a Source 2 compiled resource (`*_c`) file.
//!
//! Layout, following ValveResourceFormat's `Resource.cs`:
//!
//! ```text
//! u32 file_size
//! u16 header_version   (always 12)
//! u16 version
//! u32 block_offset     relative to its own position
//! u32 block_count
//! ... block_count entries of:
//!     u32 fourcc
//!     u32 offset       relative to its own position
//!     u32 size
//! ```

use crate::error::{Result, Source2Error};
use crate::kv3::{self, Kv3Document};
use crate::reader::Reader;
use crate::rerl::{self, ExternalReference};

/// The header version every Source 2 resource carries.
pub const KNOWN_HEADER_VERSION: u16 = 12;

/// Sanity ceiling on the block count; real resources have well under 20.
const MAX_BLOCKS: u32 = 1024;

/// A four-character block identifier such as `DATA` or `RERL`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    /// Build a `FourCc` from a byte string, e.g. `FourCc::new(b"DATA")`.
    #[must_use]
    pub const fn new(bytes: &[u8; 4]) -> Self {
        Self(*bytes)
    }

    /// External resource reference list.
    pub const RERL: Self = Self::new(b"RERL");
    /// Resource edit info (legacy, struct-based).
    pub const REDI: Self = Self::new(b"REDI");
    /// Resource edit info version 2 (KV3).
    pub const RED2: Self = Self::new(b"RED2");
    /// Introspection manifest describing NTRO struct layouts.
    pub const NTRO: Self = Self::new(b"NTRO");
    /// The resource's main payload.
    pub const DATA: Self = Self::new(b"DATA");
    /// Vertex and index buffers.
    pub const VBIB: Self = Self::new(b"VBIB");
    /// Voxel visibility.
    pub const VXVS: Self = Self::new(b"VXVS");

    /// The identifier as a string, with non-printable bytes escaped.
    #[must_use]
    pub fn as_str(&self) -> String {
        self.0
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() {
                    char::from(b).to_string()
                } else {
                    format!("\\x{b:02x}")
                }
            })
            .collect()
    }
}

impl std::fmt::Display for FourCc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl std::fmt::Debug for FourCc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FourCc({})", self.as_str())
    }
}

/// One entry of the block directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRef {
    /// Block identifier.
    pub fourcc: FourCc,
    /// Absolute offset of the block's payload within the file.
    pub offset: u64,
    /// Payload size in bytes.
    pub size: u32,
}

/// A parsed compiled resource. Borrows the file bytes.
#[derive(Debug, Clone)]
pub struct Resource<'a> {
    data: &'a [u8],
    /// Size the header claims the file is, excluding some trailing sections.
    pub file_size: u32,
    /// Header version; always [`KNOWN_HEADER_VERSION`].
    pub header_version: u16,
    /// Resource-specific version number.
    pub version: u16,
    /// The block directory, in file order.
    pub blocks: Vec<BlockRef>,
}

impl<'a> Resource<'a> {
    /// Parse the header and block directory.
    ///
    /// # Errors
    ///
    /// Returns [`Source2Error`] if the data is not a compiled resource or the
    /// block directory is malformed. Never panics.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);

        let file_size = r.u32("file size")?;
        if file_size == 0x55AA_1234 {
            return Err(Source2Error::IsVpkArchive);
        }

        let header_version = r.u16("header version")?;
        if header_version != KNOWN_HEADER_VERSION {
            return Err(Source2Error::BadHeaderVersion {
                found: header_version,
                expected: KNOWN_HEADER_VERSION,
            });
        }

        let version = r.u16("version")?;
        let block_offset = r.u32("block offset")?;
        let block_count = r.u32("block count")?;

        if block_count > MAX_BLOCKS {
            return Err(Source2Error::BadBlockCount { count: block_count });
        }

        // The offset is relative to its own position, i.e. 8 bytes back.
        r.skip_signed(i64::from(block_offset) - 8, "block directory")?;

        let mut blocks = Vec::with_capacity(block_count as usize);
        for _ in 0..block_count {
            let fourcc = FourCc(r.array::<4>("block fourcc")?);
            let position = r.pos() as u64;
            let offset = position + u64::from(r.u32("block offset")?);
            let size = r.u32("block size")?;

            // Skip past the two u32s regardless of what they contained.
            r.seek(position + 8, "block directory")?;

            let end = offset.saturating_add(u64::from(size));
            if end > data.len() as u64 {
                return Err(Source2Error::BlockOutOfBounds {
                    fourcc: fourcc.as_str(),
                    offset,
                    size: u64::from(size),
                    file_len: data.len(),
                });
            }

            blocks.push(BlockRef {
                fourcc,
                offset,
                size,
            });
        }

        Ok(Self {
            data,
            file_size,
            header_version,
            version,
            blocks,
        })
    }

    /// The bytes the resource was parsed from.
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Look up a block by identifier.
    #[must_use]
    pub fn block(&self, fourcc: FourCc) -> Option<&BlockRef> {
        self.blocks.iter().find(|b| b.fourcc == fourcc)
    }

    /// Whether a block with this identifier is present.
    #[must_use]
    pub fn has_block(&self, fourcc: FourCc) -> bool {
        self.block(fourcc).is_some()
    }

    /// Raw bytes of a block.
    ///
    /// Bounds were validated during parsing, so this cannot fail for a
    /// `BlockRef` that came from this resource.
    #[must_use]
    pub fn block_bytes(&self, block: &BlockRef) -> &'a [u8] {
        let start = usize::try_from(block.offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(block.size as usize);
        self.data.get(start..end).unwrap_or(&[])
    }

    /// Raw bytes of a block looked up by identifier.
    #[must_use]
    pub fn block_bytes_by_type(&self, fourcc: FourCc) -> Option<&'a [u8]> {
        self.block(fourcc).map(|b| self.block_bytes(b))
    }

    /// Whether a block's payload begins with a binary KV3 signature.
    #[must_use]
    pub fn block_is_kv3(&self, block: &BlockRef) -> bool {
        let bytes = self.block_bytes(block);
        bytes.len() >= 4
            && kv3::is_binary_kv3(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Decode a block as binary KV3.
    ///
    /// # Errors
    ///
    /// Returns an error if the block is absent or does not decode.
    pub fn kv3_block(&self, fourcc: FourCc) -> Option<Result<Kv3Document>> {
        let block = self.block(fourcc)?;
        if !self.block_is_kv3(block) {
            return None;
        }
        Some(kv3::decode(self.block_bytes(block)))
    }

    /// Decode the `DATA` block as binary KV3, if it is one.
    ///
    /// Returns `None` when there is no `DATA` block or it uses the older
    /// NTRO struct encoding rather than KV3.
    pub fn data_kv3(&self) -> Option<Result<Kv3Document>> {
        self.kv3_block(FourCc::DATA)
    }

    /// Decode the `RERL` external reference list.
    ///
    /// Returns an empty vector when the block is absent.
    ///
    /// # Errors
    ///
    /// Returns an error if the block is present but malformed.
    pub fn external_references(&self) -> Result<Vec<ExternalReference>> {
        match self.block(FourCc::RERL) {
            Some(block) => rerl::parse(self.data, block.offset),
            None => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a minimal resource with the given blocks (fourcc, payload).
    pub(crate) fn build_resource(version: u16, blocks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let header_len = 16usize;
        let dir_len = blocks.len() * 12;
        let mut payload_offset = header_len + dir_len;

        let mut out = Vec::new();
        let mut dir = Vec::new();
        let mut payloads = Vec::new();

        for (fourcc, data) in blocks {
            // Directory entry position of this block's offset field.
            let entry_pos = header_len + dir.len() + 4;
            dir.extend_from_slice(*fourcc);
            let relative = payload_offset - entry_pos;
            dir.extend_from_slice(&(relative as u32).to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            payloads.extend_from_slice(data);
            payload_offset += data.len();
        }

        let total = header_len + dir_len + payloads.len();
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&KNOWN_HEADER_VERSION.to_le_bytes());
        out.extend_from_slice(&version.to_le_bytes());
        // block_offset is relative to its own position (offset 8), so the
        // directory starting at 16 means a value of 8.
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir);
        out.extend_from_slice(&payloads);
        out
    }

    #[test]
    fn parses_block_directory() {
        let bytes = build_resource(
            4,
            &[
                (b"RERL", vec![0u8; 8]),
                (b"DATA", b"hello data".to_vec()),
                (b"VBIB", vec![1, 2, 3]),
            ],
        );
        let res = Resource::parse(&bytes).expect("parse");
        assert_eq!(res.header_version, KNOWN_HEADER_VERSION);
        assert_eq!(res.version, 4);
        assert_eq!(res.blocks.len(), 3);
        assert_eq!(res.blocks[0].fourcc, FourCc::RERL);
        assert_eq!(res.blocks[1].fourcc, FourCc::DATA);
        assert_eq!(res.blocks[2].fourcc, FourCc::VBIB);
        assert_eq!(res.block_bytes(&res.blocks[1]), b"hello data");
        assert_eq!(res.block_bytes(&res.blocks[2]), &[1, 2, 3]);
        assert!(res.has_block(FourCc::DATA));
        assert!(!res.has_block(FourCc::NTRO));
        // A DATA block that is not KV3 is reported as such rather than failing.
        assert!(res.data_kv3().is_none());
        // No RERL entries: the block is 8 zero bytes, meaning size 0.
        assert!(res.external_references().unwrap().is_empty());
    }

    #[test]
    fn rejects_non_resource() {
        let mut bytes = build_resource(1, &[(b"DATA", vec![0u8; 4])]);
        bytes[4] = 0xFF; // corrupt header version
        assert!(matches!(
            Resource::parse(&bytes),
            Err(Source2Error::BadHeaderVersion { .. })
        ));
    }

    #[test]
    fn rejects_vpk_input() {
        let mut bytes = vec![0u8; 32];
        bytes[..4].copy_from_slice(&0x55AA_1234u32.to_le_bytes());
        assert!(matches!(
            Resource::parse(&bytes),
            Err(Source2Error::IsVpkArchive)
        ));
    }

    #[test]
    fn rejects_out_of_bounds_block() {
        let mut bytes = build_resource(1, &[(b"DATA", vec![0u8; 4])]);
        // Inflate the block size past the end of the file.
        let size_pos = 16 + 8;
        bytes[size_pos..size_pos + 4].copy_from_slice(&0xFFFF_u32.to_le_bytes());
        assert!(matches!(
            Resource::parse(&bytes),
            Err(Source2Error::BlockOutOfBounds { .. })
        ));
    }

    #[test]
    fn fourcc_escapes_non_printable() {
        assert_eq!(FourCc::new(b"DATA").to_string(), "DATA");
        assert_eq!(FourCc(*b"AB\x00D").to_string(), "AB\\x00D");
    }

    #[test]
    fn truncation_never_panics() {
        let bytes = build_resource(
            4,
            &[(b"RERL", vec![0u8; 8]), (b"DATA", b"hello data".to_vec())],
        );
        for cut in 0..bytes.len() {
            if let Ok(res) = Resource::parse(&bytes[..cut]) {
                for block in &res.blocks {
                    let _ = res.block_bytes(block);
                    let _ = res.block_is_kv3(block);
                }
                let _ = res.external_references();
                let _ = res.data_kv3();
            }
        }
    }

    #[test]
    fn bit_flips_never_panic() {
        let bytes = build_resource(4, &[(b"RERL", vec![0u8; 24]), (b"DATA", vec![7u8; 16])]);
        for i in 0..bytes.len() {
            for mask in [0x01u8, 0x40, 0xFF] {
                let mut corrupt = bytes.clone();
                corrupt[i] ^= mask;
                if let Ok(res) = Resource::parse(&corrupt) {
                    for block in &res.blocks {
                        let _ = res.block_bytes(block);
                        let _ = res.block_is_kv3(block);
                    }
                    let _ = res.external_references();
                    let _ = res.data_kv3();
                }
            }
        }
    }
}
