//! Reader for Valve VPK archives.
//!
//! Supports VPK version 1 and version 2, in both layouts:
//!
//! * **single-file** archives, where the directory tree and all file data live
//!   in one `.vpk` (entries use archive index [`ARCHIVE_INDEX_SELF`]). CS2 map
//!   archives such as `de_dust2.vpk` are of this kind.
//! * **multi-chunk** archives, where `foo_dir.vpk` holds the tree and the data
//!   lives in sibling `foo_000.vpk`, `foo_001.vpk`, ... files.
//!
//! The byte layout implemented here follows Valve's own `ValvePak` reader.
//!
//! # Robustness
//!
//! Every parse step is bounds-checked and returns [`VpkError`] rather than
//! panicking: malformed or hostile archives produce errors, never aborts. The
//! reader also refuses to allocate based on unvalidated length fields.
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> Result<(), vpk::VpkError> {
//! let archive = vpk::Vpk::open("de_dust2.vpk")?;
//! println!("{} entries", archive.entries().len());
//! for entry in archive.entries().iter().take(5) {
//!     println!("{} ({} bytes)", entry.path, entry.total_len());
//! }
//! let bytes = archive.read(&archive.entries()[0])?;
//! # let _ = bytes;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

mod error;

pub use error::{Result, VpkError};

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Magic number every VPK starts with.
pub const VPK_SIGNATURE: u32 = 0x55AA_1234;

/// Archive index meaning "the data is in this same file, after the tree".
pub const ARCHIVE_INDEX_SELF: u16 = 0x7FFF;

/// Valve writes a single space where an extension or directory is absent.
const SPACE: &str = " ";

/// Upper bound on the directory tree we are willing to buffer (256 MiB).
///
/// Guards against a corrupt `tree_size` causing a huge allocation.
const MAX_TREE_SIZE: u32 = 256 * 1024 * 1024;

/// Parsed VPK header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Archive format version (1 or 2).
    pub version: u32,
    /// Size in bytes of the directory tree that follows the header.
    pub tree_size: u32,
    /// Bytes of file content embedded in this file (v2 only; 0 in CS:GO/CS2).
    pub file_data_section_size: u32,
    /// Size of the per-chunk MD5 section (v2 only).
    pub archive_md5_section_size: u32,
    /// Size of the tree/whole-file MD5 section (v2 only, normally 48).
    pub other_md5_section_size: u32,
    /// Size of the public key + signature section (v2 only).
    pub signature_section_size: u32,
    /// Number of bytes the header itself occupied (12 for v1, 28 for v2).
    pub header_size: u32,
}

impl Header {
    /// Absolute offset at which entry data for [`ARCHIVE_INDEX_SELF`] begins.
    #[must_use]
    pub fn data_base_offset(&self) -> u64 {
        u64::from(self.header_size) + u64::from(self.tree_size)
    }
}

/// One file stored in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Full path, e.g. `maps/de_dust2/entities/default_ents.vents_c`.
    pub path: String,
    /// Directory portion, empty for files at the archive root.
    pub directory: String,
    /// File name without extension.
    pub file_name: String,
    /// Extension without the dot, empty if the file has none.
    pub extension: String,
    /// CRC32 of the complete file contents (preload bytes included).
    pub crc32: u32,
    /// Bytes stored inline in the directory tree, preceding the archive data.
    pub preload: Vec<u8>,
    /// Which archive file holds the data, or [`ARCHIVE_INDEX_SELF`].
    pub archive_index: u16,
    /// Offset of the data within its archive file.
    ///
    /// For [`ARCHIVE_INDEX_SELF`] this is relative to
    /// [`Header::data_base_offset`], not to the start of the file.
    pub offset: u32,
    /// Length of the data stored in the archive (excludes preload bytes).
    pub length: u32,
}

impl Entry {
    /// Total size of the file: preload bytes plus archive bytes.
    #[must_use]
    pub fn total_len(&self) -> u64 {
        self.preload.len() as u64 + u64::from(self.length)
    }

    /// Whether this entry looks like a Source 2 compiled resource (`*_c`).
    #[must_use]
    pub fn is_compiled_resource(&self) -> bool {
        self.extension.ends_with("_c")
    }
}

/// An opened VPK archive.
///
/// Holds the directory tree in memory and the backing files open for reading.
pub struct Vpk {
    /// Path of the file that was opened.
    path: PathBuf,
    /// Path prefix used to derive chunk file names (`<prefix>_000.vpk`).
    chunk_prefix: PathBuf,
    header: Header,
    entries: Vec<Entry>,
    /// Lowercased path -> index into `entries`.
    by_path: HashMap<String, usize>,
    /// The directory file itself, kept open.
    dir_file: RefCell<File>,
    /// Length of the directory file.
    dir_len: u64,
    /// Lazily opened chunk files, keyed by archive index.
    chunks: RefCell<HashMap<u16, (File, u64)>>,
}

impl std::fmt::Debug for Vpk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vpk")
            .field("path", &self.path)
            .field("header", &self.header)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl Vpk {
    /// Open a VPK archive.
    ///
    /// Accepts either a single-file archive or the `_dir.vpk` of a multi-chunk
    /// set; in the latter case sibling chunk files are opened on demand.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path).map_err(|source| VpkError::Io {
            path: path.clone(),
            source,
        })?;
        let dir_len = file
            .metadata()
            .map_err(|source| VpkError::Io {
                path: path.clone(),
                source,
            })?
            .len();

        let mut head = [0u8; 28];
        // Read as much of the header as exists; v1 headers are only 12 bytes.
        let read = read_up_to(&mut file, &mut head).map_err(|source| VpkError::Io {
            path: path.clone(),
            source,
        })?;
        let head = &head[..read];

        let mut cur = Cursor::new(head);
        let signature = cur.u32("signature")?;
        if signature != VPK_SIGNATURE {
            return Err(VpkError::BadSignature { found: signature });
        }
        let version = cur.u32("version")?;
        let tree_size = cur.u32("tree size")?;

        let mut header = Header {
            version,
            tree_size,
            file_data_section_size: 0,
            archive_md5_section_size: 0,
            other_md5_section_size: 0,
            signature_section_size: 0,
            header_size: 12,
        };

        match version {
            1 => {}
            2 => {
                header.file_data_section_size = cur.u32("file data section size")?;
                header.archive_md5_section_size = cur.u32("archive md5 section size")?;
                header.other_md5_section_size = cur.u32("other md5 section size")?;
                header.signature_section_size = cur.u32("signature section size")?;
                header.header_size = 28;
            }
            0x0003_0002 => {
                return Err(VpkError::UnsupportedVersion {
                    version,
                    note: "Respawn (Titanfall/Apex) VPKs use a different format",
                })
            }
            _ => {
                return Err(VpkError::UnsupportedVersion {
                    version,
                    note: "only VPK version 1 and 2 are supported",
                })
            }
        }

        if tree_size > MAX_TREE_SIZE {
            return Err(VpkError::TreeOverrun { tree_size });
        }

        // Read the whole tree into memory; it is small relative to the archive.
        file.seek(SeekFrom::Start(u64::from(header.header_size)))
            .map_err(|source| VpkError::Io {
                path: path.clone(),
                source,
            })?;
        let mut tree = vec![0u8; tree_size as usize];
        file.read_exact(&mut tree)
            .map_err(|_| VpkError::UnexpectedEof {
                what: "directory tree",
                needed: u64::from(tree_size),
                offset: u64::from(header.header_size),
            })?;

        let entries = parse_tree(&tree)?;

        let mut by_path = HashMap::with_capacity(entries.len());
        for (i, entry) in entries.iter().enumerate() {
            by_path.entry(entry.path.to_ascii_lowercase()).or_insert(i);
        }

        let chunk_prefix = derive_chunk_prefix(&path);

        Ok(Self {
            path,
            chunk_prefix,
            header,
            entries,
            by_path,
            dir_file: RefCell::new(file),
            dir_len,
            chunks: RefCell::new(HashMap::new()),
        })
    }

    /// Path this archive was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Parsed header.
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// All entries, in directory-tree order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether any entry stores data outside the directory file.
    #[must_use]
    pub fn is_multi_chunk(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.archive_index != ARCHIVE_INDEX_SELF)
    }

    /// Sum of every entry's total length.
    #[must_use]
    pub fn total_entry_bytes(&self) -> u64 {
        self.entries.iter().map(Entry::total_len).sum()
    }

    /// Find an entry by path. Matching is case-insensitive and accepts either
    /// slash direction.
    #[must_use]
    pub fn find(&self, path: &str) -> Option<&Entry> {
        let normalised = path.replace('\\', "/").to_ascii_lowercase();
        self.by_path.get(&normalised).map(|&i| &self.entries[i])
    }

    /// Read an entry's full contents, without verifying its CRC.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        self.read_inner(entry, false)
    }

    /// Read an entry's full contents and verify the stored CRC32.
    pub fn read_verified(&self, entry: &Entry) -> Result<Vec<u8>> {
        self.read_inner(entry, true)
    }

    /// Read a file by path, without verifying its CRC.
    pub fn read_path(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self.find(path).ok_or_else(|| VpkError::NotFound {
            path: path.to_string(),
        })?;
        self.read(entry)
    }

    fn read_inner(&self, entry: &Entry, verify_crc: bool) -> Result<Vec<u8>> {
        let total = entry.total_len();
        let mut out = Vec::with_capacity(total as usize);
        out.extend_from_slice(&entry.preload);

        if entry.length > 0 {
            let start = out.len();
            out.resize(total as usize, 0);
            let dest = &mut out[start..];

            if entry.archive_index == ARCHIVE_INDEX_SELF {
                let offset = self.header.data_base_offset() + u64::from(entry.offset);
                Self::read_at(
                    &mut self.dir_file.borrow_mut(),
                    &self.path,
                    self.dir_len,
                    offset,
                    dest,
                    entry,
                )?;
            } else {
                let mut chunks = self.chunks.borrow_mut();
                let chunk_path = self.chunk_path(entry.archive_index);
                let (file, len) = match chunks.entry(entry.archive_index) {
                    std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        let file = File::open(&chunk_path).map_err(|_| VpkError::MissingChunk {
                            index: entry.archive_index,
                            path: chunk_path.clone(),
                        })?;
                        let len = file
                            .metadata()
                            .map_err(|source| VpkError::Io {
                                path: chunk_path.clone(),
                                source,
                            })?
                            .len();
                        slot.insert((file, len))
                    }
                };
                let len = *len;
                Self::read_at(file, &chunk_path, len, u64::from(entry.offset), dest, entry)?;
            }
        }

        if verify_crc {
            let actual = crc32fast::hash(&out);
            if actual != entry.crc32 {
                return Err(VpkError::CrcMismatch {
                    path: entry.path.clone(),
                    expected: entry.crc32,
                    actual,
                });
            }
        }

        Ok(out)
    }

    fn read_at(
        file: &mut File,
        file_path: &Path,
        file_len: u64,
        offset: u64,
        dest: &mut [u8],
        entry: &Entry,
    ) -> Result<()> {
        let end = offset.saturating_add(dest.len() as u64);
        if end > file_len {
            return Err(VpkError::EntryOutOfBounds {
                path: entry.path.clone(),
                offset,
                length: dest.len() as u64,
                archive_len: file_len,
            });
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|source| VpkError::Io {
                path: file_path.to_path_buf(),
                source,
            })?;
        file.read_exact(dest).map_err(|_| VpkError::UnexpectedEof {
            what: "entry data",
            needed: dest.len() as u64,
            offset,
        })
    }

    fn chunk_path(&self, index: u16) -> PathBuf {
        let mut p = self.chunk_prefix.clone().into_os_string();
        p.push(format!("_{index:03}.vpk"));
        PathBuf::from(p)
    }
}

/// Strip a trailing `_dir.vpk` / `.vpk` so chunk names can be derived.
fn derive_chunk_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let trimmed = if let Some(stripped) = strip_suffix_ci(&s, "_dir.vpk") {
        stripped
    } else if let Some(stripped) = strip_suffix_ci(&s, ".vpk") {
        stripped
    } else {
        &s
    };
    PathBuf::from(trimmed)
}

fn strip_suffix_ci<'a>(haystack: &'a str, suffix: &str) -> Option<&'a str> {
    if haystack.len() < suffix.len() {
        return None;
    }
    let split = haystack.len() - suffix.len();
    if !haystack.is_char_boundary(split) {
        return None;
    }
    let (head, tail) = haystack.split_at(split);
    tail.eq_ignore_ascii_case(suffix).then_some(head)
}

/// Read as many bytes as are available, up to `buf.len()`.
fn read_up_to(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Parse the extension -> directory -> file tree.
fn parse_tree(tree: &[u8]) -> Result<Vec<Entry>> {
    let mut cur = Cursor::new(tree);
    let mut entries = Vec::new();

    loop {
        let extension = cur.cstr("extension")?;
        if extension.is_empty() {
            break;
        }
        loop {
            let directory = cur.cstr("directory")?;
            if directory.is_empty() {
                break;
            }
            loop {
                let file_name = cur.cstr("file name")?;
                if file_name.is_empty() {
                    break;
                }

                let crc32 = cur.u32("entry crc")?;
                let preload_len = cur.u16("preload length")?;
                let archive_index = cur.u16("archive index")?;
                let offset = cur.u32("entry offset")?;
                let length = cur.u32("entry length")?;
                let terminator = cur.u16("entry terminator")?;

                let path = build_path(&directory, &file_name, &extension);

                if terminator != 0xFFFF {
                    return Err(VpkError::BadTerminator {
                        path,
                        found: terminator,
                    });
                }

                let preload = cur.bytes(preload_len as usize, "preload data")?.to_vec();

                entries.push(Entry {
                    path,
                    directory: if directory == SPACE {
                        String::new()
                    } else {
                        directory.clone()
                    },
                    file_name,
                    extension: if extension == SPACE {
                        String::new()
                    } else {
                        extension.clone()
                    },
                    crc32,
                    preload,
                    archive_index,
                    offset,
                    length,
                });
            }
        }
    }

    Ok(entries)
}

fn build_path(directory: &str, file_name: &str, extension: &str) -> String {
    let name = if extension == SPACE {
        file_name.to_string()
    } else {
        format!("{file_name}.{extension}")
    };
    if directory == SPACE {
        name
    } else {
        format!("{directory}/{name}")
    }
}

/// Bounds-checked little-endian reader over an in-memory slice.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(VpkError::UnexpectedEof {
            what,
            needed: n as u64,
            offset: self.pos as u64,
        })?;
        if end > self.data.len() {
            return Err(VpkError::UnexpectedEof {
                what,
                needed: n as u64,
                offset: self.pos as u64,
            });
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16> {
        let b = self.bytes(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32> {
        let b = self.bytes(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a NUL-terminated UTF-8 string.
    fn cstr(&mut self, what: &'static str) -> Result<String> {
        let start = self.pos;
        let rest = self.data.get(start..).ok_or(VpkError::UnexpectedEof {
            what,
            needed: 1,
            offset: start as u64,
        })?;
        let nul = rest.iter().position(|&b| b == 0).ok_or({
            // Ran off the end of the tree without a terminator.
            VpkError::UnexpectedEof {
                what,
                needed: 1,
                offset: start as u64,
            }
        })?;
        self.pos = start + nul + 1;
        std::str::from_utf8(&rest[..nul])
            .map(str::to_string)
            .map_err(|_| VpkError::InvalidUtf8 {
                offset: start as u64,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_building_handles_space_sentinels() {
        assert_eq!(build_path("maps", "de_dust2", "vpk"), "maps/de_dust2.vpk");
        assert_eq!(build_path(" ", "readme", "txt"), "readme.txt");
        assert_eq!(build_path("root", "LICENSE", " "), "root/LICENSE");
        assert_eq!(build_path(" ", "LICENSE", " "), "LICENSE");
    }

    #[test]
    fn chunk_prefix_strips_dir_suffix() {
        assert_eq!(
            derive_chunk_prefix(Path::new("/games/pak01_dir.vpk")),
            PathBuf::from("/games/pak01")
        );
        assert_eq!(
            derive_chunk_prefix(Path::new("/games/de_dust2.vpk")),
            PathBuf::from("/games/de_dust2")
        );
        // Case-insensitive, as Windows paths may be capitalised.
        assert_eq!(
            derive_chunk_prefix(Path::new("/games/PAK01_DIR.VPK")),
            PathBuf::from("/games/PAK01")
        );
    }

    #[test]
    fn cursor_rejects_truncated_reads() {
        let mut cur = Cursor::new(&[1, 2, 3]);
        assert!(cur.u32("x").is_err());
        assert!(cur.u16("x").is_ok());
    }

    #[test]
    fn cursor_rejects_unterminated_string() {
        let mut cur = Cursor::new(b"abc");
        assert!(cur.cstr("x").is_err());
    }

    #[test]
    fn cursor_reads_strings() {
        let mut cur = Cursor::new(b"vmdl_c\0maps\0\0");
        assert_eq!(cur.cstr("x").unwrap(), "vmdl_c");
        assert_eq!(cur.cstr("x").unwrap(), "maps");
        assert_eq!(cur.cstr("x").unwrap(), "");
    }
}
