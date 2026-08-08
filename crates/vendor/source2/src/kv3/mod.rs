//! Binary KeyValues3 decoding.
//!
//! Handles every binary KV3 container Valve has shipped:
//!
//! | Magic (on disk) | Version | Notes                                              |
//! | --------------- | ------- | -------------------------------------------------- |
//! | `VKV\x03`       | 0       | legacy; encoding GUID selects uncompressed or LZ4   |
//! | `\x013VK`       | 1       | no compression parameters, single buffer            |
//! | `\x023VK`       | 2       | adds compression method/frame size, binary blobs    |
//! | `\x033VK`       | 3       | flag bytes change meaning (enumerated, not bitmask) |
//! | `\x043VK`       | 4       | adds 2-byte value buffer and short/float/byte types |
//! | `\x053VK`       | 5       | splits into two buffers plus an auxiliary buffer    |
//!
//! Compression methods 0 (none), 1 (LZ4) and 2 (zstd) are implemented for all
//! versions that use them, including the chained-LZ4 framing used for binary
//! blob payloads.
//!
//! The layouts follow ValveResourceFormat's `BinaryKV3` reader.

mod value;

pub use value::{Kv3Document, Kv3Id, KvFlag, KvValue};

use crate::error::{Result, Source2Error};
use crate::reader::Reader;

/// Legacy `VKV\x03` container.
const MAGIC_V0: u32 = 0x0356_4B56;
/// The high three bytes shared by version 1 through 5 magics.
const MAGIC_BASE: u32 = 0x4B56_3300;
/// Sentinel terminating several sections.
const TRAILER: u32 = 0xFFEE_DD00;
/// Sentinel terminating the legacy version 0 payload.
const LEGACY_TRAILER: u32 = 0xFFFF_FFFF;
/// LZ4 frame size the format mandates from version 2 onwards.
const LZ4_FRAME_SIZE: u16 = 16384;

/// Refuse to allocate more than this from a single declared size field (512 MiB).
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
/// Maximum value nesting depth. Guards against stack exhaustion on hostile input.
const MAX_DEPTH: usize = 128;

/// Node type tags used by the binary encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeType {
    Null = 1,
    Boolean = 2,
    Int64 = 3,
    UInt64 = 4,
    Double = 5,
    String = 6,
    BinaryBlob = 7,
    Array = 8,
    Object = 9,
    ArrayTyped = 10,
    Int32 = 11,
    UInt32 = 12,
    BooleanTrue = 13,
    BooleanFalse = 14,
    Int64Zero = 15,
    Int64One = 16,
    DoubleZero = 17,
    DoubleOne = 18,
    Float = 19,
    Int16 = 20,
    UInt16 = 21,
    Unknown22 = 22,
    Int32AsByte = 23,
    ArrayTypeByteLength = 24,
    ArrayTypeAuxiliaryBuffer = 25,
}

impl NodeType {
    fn from_byte(b: u8) -> Result<Self> {
        use NodeType::{
            Array, ArrayTypeAuxiliaryBuffer, ArrayTypeByteLength, ArrayTyped, BinaryBlob, Boolean,
            BooleanFalse, BooleanTrue, Double, DoubleOne, DoubleZero, Float, Int16, Int32,
            Int32AsByte, Int64, Int64One, Int64Zero, Null, Object, String, UInt16, UInt32, UInt64,
            Unknown22,
        };
        Ok(match b {
            1 => Null,
            2 => Boolean,
            3 => Int64,
            4 => UInt64,
            5 => Double,
            6 => String,
            7 => BinaryBlob,
            8 => Array,
            9 => Object,
            10 => ArrayTyped,
            11 => Int32,
            12 => UInt32,
            13 => BooleanTrue,
            14 => BooleanFalse,
            15 => Int64Zero,
            16 => Int64One,
            17 => DoubleZero,
            18 => DoubleOne,
            19 => Float,
            20 => Int16,
            21 => UInt16,
            22 => Unknown22,
            23 => Int32AsByte,
            24 => ArrayTypeByteLength,
            25 => ArrayTypeAuxiliaryBuffer,
            _ => return Err(Source2Error::UnknownNodeType { value: b }),
        })
    }
}

/// An owned byte buffer with a read cursor.
#[derive(Debug, Default)]
struct Buf {
    data: Vec<u8>,
    pos: usize,
}

impl Buf {
    fn new(data: Vec<u8>) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&[u8]> {
        let end = self.pos.checked_add(n).ok_or(Source2Error::UnexpectedEof {
            what,
            needed: n,
            offset: self.pos,
            available: self.remaining(),
        })?;
        if end > self.data.len() {
            return Err(Source2Error::UnexpectedEof {
                what,
                needed: n,
                offset: self.pos,
                available: self.remaining(),
            });
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn arr<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, what)?);
        Ok(out)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }
    fn i16(&mut self, what: &'static str) -> Result<i16> {
        Ok(i16::from_le_bytes(self.arr(what)?))
    }
    fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr(what)?))
    }
    fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr(what)?))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr(what)?))
    }
    fn i64(&mut self, what: &'static str) -> Result<i64> {
        Ok(i64::from_le_bytes(self.arr(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr(what)?))
    }
    fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_le_bytes(self.arr(what)?))
    }
    fn f64(&mut self, what: &'static str) -> Result<f64> {
        Ok(f64::from_le_bytes(self.arr(what)?))
    }

    /// Read a NUL-terminated UTF-8 string from the cursor.
    fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Source2Error::UnexpectedEof {
                what: "kv3 string",
                needed: 1,
                offset: self.pos,
                available: rest.len(),
            })?;
        let s = std::str::from_utf8(&rest[..nul])
            .map_err(|_| Source2Error::InvalidUtf8)?
            .to_string();
        self.pos += nul + 1;
        Ok(s)
    }
}

/// The four value-width buffers a KV3 payload is split into.
#[derive(Debug, Default)]
struct Buffers {
    b1: Buf,
    b2: Buf,
    b4: Buf,
    b8: Buf,
}

/// Decoder state threaded through the recursive value reader.
struct Ctx {
    version: u8,
    types: Buf,
    object_lengths: Buf,
    blob_lengths: Buf,
    blobs: Buf,
    strings: Vec<String>,
    buf: Buffers,
    aux: Buffers,
    depth: usize,
}

impl Ctx {
    fn string(&self, id: i32) -> Result<String> {
        if id == -1 {
            return Ok(String::new());
        }
        usize::try_from(id)
            .ok()
            .and_then(|i| self.strings.get(i))
            .cloned()
            .ok_or(Source2Error::StringIndexOutOfRange {
                index: id,
                count: self.strings.len(),
            })
    }
}

/// Whether `magic` (read as a little-endian u32) starts a binary KV3 payload.
#[must_use]
pub fn is_binary_kv3(magic: u32) -> bool {
    magic == MAGIC_V0 || (magic & 0xFFFF_FF00 == MAGIC_BASE && (1..=5).contains(&(magic & 0xFF)))
}

/// Decode a binary KV3 payload.
///
/// `data` must be exactly the block's bytes: several versions derive sizes from
/// the total length.
///
/// # Errors
///
/// Returns [`Source2Error`] for any malformed input. This function never panics.
pub fn decode(data: &[u8]) -> Result<Kv3Document> {
    decode_prefix(data).map(|(doc, _)| doc)
}

/// Decode a binary KV3 payload that is followed by unrelated bytes.
///
/// Returns the document and how many bytes of `data` it occupied, so a caller
/// walking a stream — CS2's `.nav` files embed three of these back to back with
/// no length prefix — can carry on from where the payload ended.
///
/// Version 1 containers are the one exception: they derive the compressed
/// payload size from "everything left in the buffer", so they can only be
/// decoded when `data` ends exactly where they do. Those report the whole
/// slice as consumed.
///
/// # Errors
///
/// Returns [`Source2Error`] for any malformed input. This function never panics.
pub fn decode_prefix(data: &[u8]) -> Result<(Kv3Document, usize)> {
    let mut r = Reader::new(data);
    let magic = r.u32("kv3 magic")?;

    if magic == MAGIC_V0 {
        let doc = decode_v0(&mut r)?;
        return Ok((doc, r.pos()));
    }

    let version = magic & 0xFF;
    if magic & 0xFFFF_FF00 != MAGIC_BASE {
        return Err(Source2Error::BadKv3Magic { magic });
    }
    if !(1..=5).contains(&version) {
        return Err(Source2Error::UnsupportedKv3Version { version });
    }

    #[allow(clippy::cast_possible_truncation)]
    let doc = decode_modern(&mut r, version as u8)?;
    Ok((doc, r.pos()))
}

/// Reject negative sizes and cap absurd ones.
fn size(value: i32, what: &'static str) -> Result<usize> {
    if value < 0 {
        return Err(Source2Error::InvalidSize {
            what,
            value: i64::from(value),
        });
    }
    let v = value as u32;
    if u64::from(v) > MAX_ALLOC {
        return Err(Source2Error::AllocationTooLarge {
            what,
            value: u64::from(v),
            limit: MAX_ALLOC,
        });
    }
    Ok(v as usize)
}

fn align(offset: &mut usize, alignment: usize) {
    let mask = alignment - 1;
    *offset = (*offset + mask) & !mask;
}

/// Slice `span[offset..offset + len]`, erroring rather than panicking.
fn sub<'a>(span: &'a [u8], offset: usize, len: usize, what: &'static str) -> Result<&'a [u8]> {
    let end = offset.checked_add(len).ok_or(Source2Error::UnexpectedEof {
        what,
        needed: len,
        offset,
        available: span.len(),
    })?;
    span.get(offset..end).ok_or(Source2Error::UnexpectedEof {
        what,
        needed: len,
        offset,
        available: span.len().saturating_sub(offset.min(span.len())),
    })
}

fn decompress_lz4(input: &[u8], output: &mut [u8]) -> Result<()> {
    let written =
        lz4_flex::block::decompress_into(input, output).map_err(|e| Source2Error::Decompress {
            algorithm: "LZ4",
            detail: e.to_string(),
        })?;
    if written != output.len() {
        return Err(Source2Error::Decompress {
            algorithm: "LZ4",
            detail: format!("expected {} bytes, got {written}", output.len()),
        });
    }
    Ok(())
}

fn decompress_zstd(input: &[u8], output: &mut [u8]) -> Result<()> {
    use std::io::Read;

    let mut decoder =
        ruzstd::StreamingDecoder::new(input).map_err(|e| Source2Error::Decompress {
            algorithm: "zstd",
            detail: e.to_string(),
        })?;

    let mut written = 0;
    while written < output.len() {
        match decoder.read(&mut output[written..]) {
            Ok(0) => break,
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                return Err(Source2Error::Decompress {
                    algorithm: "zstd",
                    detail: e.to_string(),
                })
            }
        }
    }

    if written != output.len() {
        return Err(Source2Error::Decompress {
            algorithm: "zstd",
            detail: format!("expected {} bytes, got {written}", output.len()),
        });
    }
    Ok(())
}

/// Sizes read from the version-dependent header.
struct HeaderSizes {
    compression_method: u32,
    compression_dictionary_id: u16,
    compression_frame_size: u16,
    count_bytes1: usize,
    count_bytes2: usize,
    count_bytes4: usize,
    count_bytes8: usize,
    count_types: usize,
    size_uncompressed_total: usize,
    size_compressed_total: usize,
    count_blocks: usize,
    size_binary_blobs: usize,
    size_uncompressed_buffer1: usize,
    size_compressed_buffer1: usize,
    size_uncompressed_buffer2: usize,
    size_compressed_buffer2: usize,
    count_bytes1_buffer2: usize,
    count_bytes2_buffer2: usize,
    count_bytes4_buffer2: usize,
    count_bytes8_buffer2: usize,
    count_objects_buffer2: usize,
    size_block_compressed_sizes: usize,
}

fn read_header(r: &mut Reader<'_>, version: u8) -> Result<(Kv3Id, HeaderSizes)> {
    let format = Kv3Id(r.array::<16>("kv3 format guid")?);
    let compression_method = r.u32("compression method")?;

    let mut h = HeaderSizes {
        compression_method,
        compression_dictionary_id: 0,
        compression_frame_size: 0,
        count_bytes1: 0,
        count_bytes2: 0,
        count_bytes4: 0,
        count_bytes8: 0,
        count_types: 0,
        size_uncompressed_total: 0,
        size_compressed_total: 0,
        count_blocks: 0,
        size_binary_blobs: 0,
        size_uncompressed_buffer1: 0,
        size_compressed_buffer1: 0,
        size_uncompressed_buffer2: 0,
        size_compressed_buffer2: 0,
        count_bytes1_buffer2: 0,
        count_bytes2_buffer2: 0,
        count_bytes4_buffer2: 0,
        count_bytes8_buffer2: 0,
        count_objects_buffer2: 0,
        size_block_compressed_sizes: 0,
    };

    if version == 1 {
        // Version 1 predates the compression parameters entirely.
        h.count_bytes1 = size(r.i32("count bytes1")?, "count bytes1")?;
        h.count_bytes4 = size(r.i32("count bytes4")?, "count bytes4")?;
        h.count_bytes8 = size(r.i32("count bytes8")?, "count bytes8")?;
        h.size_uncompressed_total = size(r.i32("uncompressed size")?, "uncompressed size")?;
        // Whatever is left in the block is the compressed payload.
        h.size_compressed_total = r.remaining();
    } else {
        h.compression_dictionary_id = r.u16("compression dictionary id")?;
        h.compression_frame_size = r.u16("compression frame size")?;
        h.count_bytes1 = size(r.i32("count bytes1")?, "count bytes1")?;
        h.count_bytes4 = size(r.i32("count bytes4")?, "count bytes4")?;
        h.count_bytes8 = size(r.i32("count bytes8")?, "count bytes8")?;
        h.count_types = size(r.i32("count types")?, "count types")?;
        let _count_objects = r.u16("count objects")?;
        let _count_arrays = r.u16("count arrays")?;
        h.size_uncompressed_total = size(r.i32("uncompressed size")?, "uncompressed size")?;
        h.size_compressed_total = size(r.i32("compressed size")?, "compressed size")?;
        h.count_blocks = size(r.i32("block count")?, "block count")?;
        h.size_binary_blobs = size(r.i32("binary blob size")?, "binary blob size")?;
    }

    if version >= 4 {
        h.count_bytes2 = size(r.i32("count bytes2")?, "count bytes2")?;
        h.size_block_compressed_sizes = size(
            r.i32("block compressed sizes")?,
            "block compressed sizes size",
        )?;
    }

    if version >= 5 {
        h.size_uncompressed_buffer1 = size(r.i32("uncompressed buffer1")?, "uncompressed buffer1")?;
        h.size_compressed_buffer1 = size(r.i32("compressed buffer1")?, "compressed buffer1")?;
        h.size_uncompressed_buffer2 = size(r.i32("uncompressed buffer2")?, "uncompressed buffer2")?;
        h.size_compressed_buffer2 = size(r.i32("compressed buffer2")?, "compressed buffer2")?;
        h.count_bytes1_buffer2 = size(r.i32("count bytes1 buffer2")?, "count bytes1 buffer2")?;
        h.count_bytes2_buffer2 = size(r.i32("count bytes2 buffer2")?, "count bytes2 buffer2")?;
        h.count_bytes4_buffer2 = size(r.i32("count bytes4 buffer2")?, "count bytes4 buffer2")?;
        h.count_bytes8_buffer2 = size(r.i32("count bytes8 buffer2")?, "count bytes8 buffer2")?;
        let _unk13 = r.i32("unknown 13")?;
        h.count_objects_buffer2 = size(r.i32("count objects buffer2")?, "count objects buffer2")?;
        let _count_arrays_buffer2 = r.i32("count arrays buffer2")?;
        let _unk16 = r.i32("unknown 16")?;
    } else {
        h.size_compressed_buffer1 = h.size_compressed_total;
        h.size_uncompressed_buffer1 = h.size_uncompressed_total;
    }

    Ok((format, h))
}

/// Split a decompressed buffer into its 1/2/4/8-byte regions.
///
/// Returns the buffers and the offset just past the last one.
fn split_value_buffers(
    span: &[u8],
    counts: (usize, usize, usize, usize),
    start: usize,
    align_empty_b8: bool,
) -> Result<(Buffers, usize)> {
    let (c1, c2, c4, c8) = counts;
    let mut offset = start;
    let mut bufs = Buffers::default();

    if c1 > 0 {
        bufs.b1 = Buf::new(sub(span, offset, c1, "bytes1 region")?.to_vec());
        offset += c1;
    }
    if c2 > 0 {
        align(&mut offset, 2);
        let len = c2.checked_mul(2).ok_or(Source2Error::InvalidSize {
            what: "count bytes2",
            value: -1,
        })?;
        bufs.b2 = Buf::new(sub(span, offset, len, "bytes2 region")?.to_vec());
        offset += len;
    }
    if c4 > 0 {
        align(&mut offset, 4);
        let len = c4.checked_mul(4).ok_or(Source2Error::InvalidSize {
            what: "count bytes4",
            value: -1,
        })?;
        bufs.b4 = Buf::new(sub(span, offset, len, "bytes4 region")?.to_vec());
        offset += len;
    }
    if c8 > 0 {
        align(&mut offset, 8);
        let len = c8.checked_mul(8).ok_or(Source2Error::InvalidSize {
            what: "count bytes8",
            value: -1,
        })?;
        bufs.b8 = Buf::new(sub(span, offset, len, "bytes8 region")?.to_vec());
        offset += len;
    } else if align_empty_b8 {
        // Versions before 5 align here even when the region is empty.
        align(&mut offset, 8);
    }

    Ok((bufs, offset))
}

fn decode_modern(r: &mut Reader<'_>, version: u8) -> Result<Kv3Document> {
    let (format, h) = read_header(r, version)?;

    // Before version 5, zstd compressed the value buffer and the binary blobs
    // together, so the output buffer has to be large enough for both.
    let alloc1 = if version < 5 && h.compression_method == 2 {
        h.size_uncompressed_buffer1
            .checked_add(h.size_binary_blobs)
            .ok_or(Source2Error::InvalidSize {
                what: "buffer1 allocation",
                value: -1,
            })?
    } else {
        h.size_uncompressed_buffer1
    };
    if alloc1 as u64 > MAX_ALLOC {
        return Err(Source2Error::AllocationTooLarge {
            what: "buffer1",
            value: alloc1 as u64,
            limit: MAX_ALLOC,
        });
    }

    let mut buffer1_raw = vec![0u8; alloc1];

    match h.compression_method {
        0 => {
            if h.compression_dictionary_id != 0 {
                return Err(Source2Error::UnhandledCompressionParam {
                    name: "compression dictionary id",
                    value: u32::from(h.compression_dictionary_id),
                });
            }
            if h.compression_frame_size != 0 {
                return Err(Source2Error::UnhandledCompressionParam {
                    name: "compression frame size",
                    value: u32::from(h.compression_frame_size),
                });
            }
            let src = r.take(h.size_uncompressed_buffer1, "uncompressed buffer1")?;
            buffer1_raw[..h.size_uncompressed_buffer1].copy_from_slice(src);
        }
        1 => {
            if h.compression_dictionary_id != 0 {
                return Err(Source2Error::UnhandledCompressionParam {
                    name: "compression dictionary id",
                    value: u32::from(h.compression_dictionary_id),
                });
            }
            if version >= 2 && h.compression_frame_size != LZ4_FRAME_SIZE {
                return Err(Source2Error::UnhandledCompressionParam {
                    name: "compression frame size",
                    value: u32::from(h.compression_frame_size),
                });
            }
            let src = r.take(h.size_compressed_buffer1, "lz4 buffer1")?;
            decompress_lz4(src, &mut buffer1_raw[..h.size_uncompressed_buffer1])?;
        }
        2 => {
            if h.compression_dictionary_id != 0 {
                return Err(Source2Error::UnhandledCompressionParam {
                    name: "compression dictionary id",
                    value: u32::from(h.compression_dictionary_id),
                });
            }
            if h.compression_frame_size != 0 {
                return Err(Source2Error::UnhandledCompressionParam {
                    name: "compression frame size",
                    value: u32::from(h.compression_frame_size),
                });
            }
            let src = r.take(h.size_compressed_buffer1, "zstd buffer1")?;
            decompress_zstd(src, &mut buffer1_raw[..alloc1])?;
        }
        method => return Err(Source2Error::UnknownCompression { method }),
    }

    let span1 = &buffer1_raw[..h.size_uncompressed_buffer1];

    let (mut bufs1, mut offset) = split_value_buffers(
        span1,
        (
            h.count_bytes1,
            h.count_bytes2,
            h.count_bytes4,
            h.count_bytes8,
        ),
        0,
        version < 5,
    )?;

    // The first 4-byte value is always the string count.
    let count_strings = size(bufs1.b4.i32("string count")?, "string count")?;

    let mut ctx = Ctx {
        version,
        types: Buf::default(),
        object_lengths: Buf::default(),
        blob_lengths: Buf::default(),
        blobs: Buf::default(),
        strings: Vec::new(),
        buf: Buffers::default(),
        aux: Buffers::default(),
        depth: 0,
    };

    // Sanity: each string is at least one byte (its NUL).
    if count_strings > span1.len() + h.size_uncompressed_buffer2 {
        return Err(Source2Error::InvalidSize {
            what: "string count",
            value: count_strings as i64,
        });
    }
    ctx.strings = Vec::with_capacity(count_strings.min(4096));

    let mut blob_sizes_buf: Option<Buf> = None;

    if version >= 5 {
        // Strings live inside the 1-byte region of the auxiliary buffer.
        for _ in 0..count_strings {
            ctx.strings.push(bufs1.b1.cstr()?);
        }
        ctx.aux = bufs1;
    } else {
        let strings_start = offset;
        for _ in 0..count_strings {
            let rest = span1.get(offset..).ok_or(Source2Error::UnexpectedEof {
                what: "kv3 string table",
                needed: 1,
                offset,
                available: 0,
            })?;
            let nul = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or(Source2Error::UnexpectedEof {
                    what: "kv3 string table",
                    needed: 1,
                    offset,
                    available: rest.len(),
                })?;
            ctx.strings.push(
                std::str::from_utf8(&rest[..nul])
                    .map_err(|_| Source2Error::InvalidUtf8)?
                    .to_string(),
            );
            offset += nul + 1;
        }

        // Types follow the string table.
        let types_len = if version == 1 {
            // Everything up to the 4-byte trailer.
            h.size_uncompressed_total
                .checked_sub(offset)
                .and_then(|v| v.checked_sub(4))
                .ok_or(Source2Error::InvalidSize {
                    what: "types length",
                    value: -1,
                })?
        } else {
            // count_types covers the string table and the type bytes together.
            h.count_types
                .checked_add(strings_start)
                .and_then(|v| v.checked_sub(offset))
                .ok_or(Source2Error::InvalidSize {
                    what: "types length",
                    value: -1,
                })?
        };
        ctx.types = Buf::new(sub(span1, offset, types_len, "types region")?.to_vec());
        offset += types_len;

        if h.count_blocks == 0 {
            let trailer = u32::from_le_bytes(
                sub(span1, offset, 4, "buffer1 trailer")?
                    .try_into()
                    .map_err(|_| Source2Error::InvalidUtf8)?,
            );
            if trailer != TRAILER {
                return Err(Source2Error::BadTrailer {
                    what: "kv3 buffer",
                    expected: TRAILER,
                    found: trailer,
                });
            }
        } else {
            blob_sizes_buf = Some(Buf::new(span1.get(offset..).unwrap_or_default().to_vec()));
        }

        ctx.buf = bufs1;
    }

    if version >= 5 {
        let mut buffer2_raw = vec![0u8; h.size_uncompressed_buffer2];
        match h.compression_method {
            0 => {
                let src = r.take(h.size_uncompressed_buffer2, "uncompressed buffer2")?;
                buffer2_raw.copy_from_slice(src);
            }
            1 => {
                let src = r.take(h.size_compressed_buffer2, "lz4 buffer2")?;
                decompress_lz4(src, &mut buffer2_raw)?;
            }
            2 => {
                let src = r.take(h.size_compressed_buffer2, "zstd buffer2")?;
                decompress_zstd(src, &mut buffer2_raw)?;
            }
            method => return Err(Source2Error::UnknownCompression { method }),
        }

        let span2 = &buffer2_raw[..];
        // Object lengths come first in buffer 2.
        let obj_len_bytes =
            h.count_objects_buffer2
                .checked_mul(4)
                .ok_or(Source2Error::InvalidSize {
                    what: "object length region",
                    value: -1,
                })?;
        ctx.object_lengths = Buf::new(sub(span2, 0, obj_len_bytes, "object lengths")?.to_vec());

        let (bufs2, mut off2) = split_value_buffers(
            span2,
            (
                h.count_bytes1_buffer2,
                h.count_bytes2_buffer2,
                h.count_bytes4_buffer2,
                h.count_bytes8_buffer2,
            ),
            obj_len_bytes,
            false,
        )?;
        ctx.buf = bufs2;

        ctx.types = Buf::new(sub(span2, off2, h.count_types, "types region")?.to_vec());
        off2 += h.count_types;

        if h.count_blocks == 0 {
            let trailer = u32::from_le_bytes(
                sub(span2, off2, 4, "buffer2 trailer")?
                    .try_into()
                    .map_err(|_| Source2Error::InvalidUtf8)?,
            );
            if trailer != TRAILER {
                return Err(Source2Error::BadTrailer {
                    what: "kv3 buffer",
                    expected: TRAILER,
                    found: trailer,
                });
            }
        } else {
            blob_sizes_buf = Some(Buf::new(span2.get(off2..).unwrap_or_default().to_vec()));
        }
    }

    if h.count_blocks > 0 {
        let mut sizes = blob_sizes_buf.unwrap_or_default();

        let lengths_bytes = h
            .count_blocks
            .checked_mul(4)
            .ok_or(Source2Error::InvalidSize {
                what: "blob length region",
                value: -1,
            })?;
        ctx.blob_lengths = Buf::new(sizes.take(lengths_bytes, "blob lengths")?.to_vec());

        let trailer = sizes.u32("blob length trailer")?;
        if trailer != TRAILER {
            return Err(Source2Error::BadTrailer {
                what: "blob lengths",
                expected: TRAILER,
                found: trailer,
            });
        }

        if h.size_binary_blobs as u64 > MAX_ALLOC {
            return Err(Source2Error::AllocationTooLarge {
                what: "binary blobs",
                value: h.size_binary_blobs as u64,
                limit: MAX_ALLOC,
            });
        }

        match h.compression_method {
            0 => {
                let src = r.take(h.size_binary_blobs, "binary blobs")?;
                ctx.blobs = Buf::new(src.to_vec());
            }
            1 => {
                // Blobs are stored as a chain of LZ4 frames whose compressed
                // sizes are the u16s left in `sizes`. Each frame may reference
                // the previously decompressed bytes as its dictionary.
                let mut out = vec![0u8; h.size_binary_blobs];
                let frame_size = usize::from(h.compression_frame_size);
                if frame_size == 0 {
                    return Err(Source2Error::UnhandledCompressionParam {
                        name: "compression frame size",
                        value: 0,
                    });
                }
                let mut done = 0usize;
                while sizes.remaining() > 0 {
                    let compressed_len = usize::from(sizes.u16("blob frame size")?);
                    let want = frame_size.min(h.size_binary_blobs.saturating_sub(done));
                    if want == 0 {
                        break;
                    }
                    let src = r.take(compressed_len, "lz4 blob frame")?;
                    let (dict, rest) = out.split_at_mut(done);
                    let written =
                        lz4_flex::block::decompress_into_with_dict(src, &mut rest[..want], dict)
                            .map_err(|e| Source2Error::Decompress {
                                algorithm: "LZ4",
                                detail: e.to_string(),
                            })?;
                    if written == 0 {
                        return Err(Source2Error::Decompress {
                            algorithm: "LZ4",
                            detail: "blob frame decoded zero bytes".to_string(),
                        });
                    }
                    done += written;
                }
                ctx.blobs = Buf::new(out);
            }
            2 => {
                if version >= 5 {
                    if h.size_block_compressed_sizes != 0 {
                        return Err(Source2Error::UnhandledCompressionParam {
                            name: "block compressed sizes",
                            value: h.size_block_compressed_sizes as u32,
                        });
                    }
                    let compressed = h
                        .size_compressed_total
                        .saturating_sub(h.size_compressed_buffer1)
                        .saturating_sub(h.size_compressed_buffer2);
                    let mut out = vec![0u8; h.size_binary_blobs];
                    let src = r.take(compressed, "zstd binary blobs")?;
                    decompress_zstd(src, &mut out)?;
                    ctx.blobs = Buf::new(out);
                } else {
                    // Already decompressed alongside buffer 1.
                    let start = h.size_uncompressed_buffer1;
                    ctx.blobs = Buf::new(
                        sub(&buffer1_raw, start, h.size_binary_blobs, "binary blobs")?.to_vec(),
                    );
                }
            }
            method => return Err(Source2Error::UnknownCompression { method }),
        }

        let trailer = r.u32("binary blob trailer")?;
        if trailer != TRAILER {
            return Err(Source2Error::BadTrailer {
                what: "binary blobs",
                expected: TRAILER,
                found: trailer,
            });
        }
    }

    let (root_type, root_flag) = read_type(&mut ctx)?;
    let root = read_value(&mut ctx, root_type, root_flag)?;

    Ok(Kv3Document {
        version,
        encoding: None,
        format,
        compression_method: h.compression_method,
        root,
    })
}

/// Read a type byte plus its optional flag byte from the type stream.
fn read_type(ctx: &mut Ctx) -> Result<(NodeType, KvFlag)> {
    let mut databyte = ctx.types.u8("kv3 type")?;
    let mut flag = KvFlag::None;

    if ctx.version >= 3 {
        if databyte & 0x80 > 0 {
            databyte &= 0x3F;
            let raw = ctx.types.u8("kv3 flag")?;
            flag = KvFlag::from_modern_byte(raw).ok_or(Source2Error::UnknownFlag { value: raw })?;
        }
    } else if databyte & 0x80 > 0 {
        databyte &= 0x7F;
        let raw = ctx.types.u8("kv3 flag")?;
        flag = KvFlag::from_legacy_byte(raw).ok_or(Source2Error::UnknownFlag { value: raw })?;
    }

    Ok((NodeType::from_byte(databyte)?, flag))
}

/// Read one member of `parent`, which is either a keyed object or an array.
fn read_member(ctx: &mut Ctx, is_array: bool) -> Result<(String, KvValue)> {
    let (ty, flag) = read_type(ctx)?;
    if is_array {
        return Ok((String::new(), read_value(ctx, ty, flag)?));
    }
    // Note the ordering: modern KV3 reads the type before the key id.
    let string_id = ctx.buf.b4.i32("member key id")?;
    let name = ctx.string(string_id)?;
    Ok((name, read_value(ctx, ty, flag)?))
}

#[allow(clippy::too_many_lines)]
fn read_value(ctx: &mut Ctx, ty: NodeType, _flag: KvFlag) -> Result<KvValue> {
    ctx.depth += 1;
    if ctx.depth > MAX_DEPTH {
        ctx.depth -= 1;
        return Err(Source2Error::DepthLimitExceeded { limit: MAX_DEPTH });
    }
    let result = read_value_inner(ctx, ty);
    ctx.depth -= 1;
    result
}

fn read_value_inner(ctx: &mut Ctx, ty: NodeType) -> Result<KvValue> {
    use NodeType as T;
    Ok(match ty {
        T::Null => KvValue::Null,
        T::BooleanTrue => KvValue::Bool(true),
        T::BooleanFalse => KvValue::Bool(false),
        T::Int64Zero => KvValue::Int(0),
        T::Int64One => KvValue::Int(1),
        T::DoubleZero => KvValue::Double(0.0),
        T::DoubleOne => KvValue::Double(1.0),

        T::Boolean => KvValue::Bool(ctx.buf.b1.u8("bool")? == 1),
        T::Int32AsByte => KvValue::Int(i64::from(ctx.buf.b1.u8("int32 as byte")?)),

        T::Int16 => KvValue::Int(i64::from(ctx.buf.b2.i16("int16")?)),
        T::UInt16 => KvValue::UInt(u64::from(ctx.buf.b2.u16("uint16")?)),

        T::Int32 => KvValue::Int(i64::from(ctx.buf.b4.i32("int32")?)),
        T::UInt32 => KvValue::UInt(u64::from(ctx.buf.b4.u32("uint32")?)),
        T::Float => KvValue::Double(f64::from(ctx.buf.b4.f32("float")?)),

        T::Int64 => KvValue::Int(ctx.buf.b8.i64("int64")?),
        T::UInt64 => KvValue::UInt(ctx.buf.b8.u64("uint64")?),
        T::Double => KvValue::Double(ctx.buf.b8.f64("double")?),

        T::String => {
            let id = ctx.buf.b4.i32("string id")?;
            KvValue::String(ctx.string(id)?)
        }

        T::BinaryBlob if ctx.version < 2 => {
            // Before version 2 the blob length and bytes were inline.
            let len = size(ctx.buf.b4.i32("blob length")?, "blob length")?;
            KvValue::Binary(ctx.buf.b1.take(len, "blob bytes")?.to_vec())
        }
        T::BinaryBlob => {
            let len = size(ctx.blob_lengths.i32("blob length")?, "blob length")?;
            KvValue::Binary(ctx.blobs.take(len, "blob bytes")?.to_vec())
        }

        T::Array => {
            let len = size(ctx.buf.b4.i32("array length")?, "array length")?;
            let mut items = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                let (_, value) = read_member(ctx, true)?;
                items.push(value);
            }
            KvValue::Array(items)
        }

        T::ArrayTyped | T::ArrayTypeByteLength => {
            let len = if matches!(ty, T::ArrayTypeByteLength) {
                usize::from(ctx.buf.b1.u8("typed array length")?)
            } else {
                size(ctx.buf.b4.i32("typed array length")?, "typed array length")?
            };
            let (sub_type, sub_flag) = read_type(ctx)?;
            let mut items = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                items.push(read_value(ctx, sub_type, sub_flag)?);
            }
            KvValue::TypedArray(items)
        }

        T::ArrayTypeAuxiliaryBuffer => {
            let len = usize::from(ctx.buf.b1.u8("aux array length")?);
            let (sub_type, sub_flag) = read_type(ctx)?;
            let mut items = Vec::with_capacity(len.min(4096));
            // Elements are stored in the auxiliary buffer; swap, read, swap back.
            std::mem::swap(&mut ctx.aux, &mut ctx.buf);
            let mut error = None;
            for _ in 0..len {
                match read_value(ctx, sub_type, sub_flag) {
                    Ok(v) => items.push(v),
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            std::mem::swap(&mut ctx.aux, &mut ctx.buf);
            if let Some(e) = error {
                return Err(e);
            }
            KvValue::TypedArray(items)
        }

        T::Object => {
            let len = if ctx.version >= 5 {
                size(ctx.object_lengths.i32("object length")?, "object length")?
            } else {
                size(ctx.buf.b4.i32("object length")?, "object length")?
            };
            let mut fields = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                fields.push(read_member(ctx, false)?);
            }
            KvValue::Object(fields)
        }

        T::Unknown22 => {
            return Err(Source2Error::UnknownNodeType { value: 22 });
        }
    })
}

// ---------------------------------------------------------------------------
// Legacy (version 0, "VKV\x03") container
// ---------------------------------------------------------------------------

fn decode_v0(r: &mut Reader<'_>) -> Result<Kv3Document> {
    let encoding = Kv3Id(r.array::<16>("kv3 encoding guid")?);
    let format = Kv3Id(r.array::<16>("kv3 format guid")?);

    let payload = if encoding == Kv3Id::BINARY_UNCOMPRESSED {
        r.take(r.remaining(), "kv3 payload")?.to_vec()
    } else if encoding == Kv3Id::BINARY_BLOCK_LZ4 {
        let out_len = size(r.i32("uncompressed size")?, "uncompressed size")?;
        let compressed = r.remaining();
        let src = r.take(compressed, "lz4 payload")?;
        let mut out = vec![0u8; out_len];
        decompress_lz4(src, &mut out)?;
        out
    } else {
        // KV3_ENCODING_BINARY_BLOCK_COMPRESSED uses Valve's CBlockCompress,
        // which is not implemented; CS2 does not ship it.
        return Err(Source2Error::UnsupportedKv3Encoding(encoding.to_string()));
    };

    let mut buf = Buf::new(payload);
    let count_strings = size(
        i32::try_from(buf.u32("string count")?).unwrap_or(-1),
        "string count",
    )?;
    let mut strings = Vec::with_capacity(count_strings.min(4096));
    for _ in 0..count_strings {
        strings.push(buf.cstr()?);
    }

    let mut ctx = LegacyCtx {
        strings,
        buf,
        depth: 0,
    };

    let (ty, flag) = legacy_read_type(&mut ctx)?;
    let root = legacy_read_value(&mut ctx, ty, flag)?;

    let trailer = ctx.buf.u32("legacy trailer")?;
    if trailer != LEGACY_TRAILER {
        return Err(Source2Error::BadTrailer {
            what: "legacy kv3",
            expected: LEGACY_TRAILER,
            found: trailer,
        });
    }

    Ok(Kv3Document {
        version: 0,
        encoding: Some(encoding),
        format,
        compression_method: 0,
        root,
    })
}

struct LegacyCtx {
    strings: Vec<String>,
    buf: Buf,
    depth: usize,
}

impl LegacyCtx {
    fn string(&self, id: i32) -> Result<String> {
        if id == -1 {
            return Ok(String::new());
        }
        usize::try_from(id)
            .ok()
            .and_then(|i| self.strings.get(i))
            .cloned()
            .ok_or(Source2Error::StringIndexOutOfRange {
                index: id,
                count: self.strings.len(),
            })
    }
}

fn legacy_read_type(ctx: &mut LegacyCtx) -> Result<(NodeType, KvFlag)> {
    let mut databyte = ctx.buf.u8("kv3 type")?;
    let mut flag = KvFlag::None;
    if databyte & 0x80 > 0 {
        databyte &= 0x7F;
        let raw = ctx.buf.u8("kv3 flag")?;
        flag = KvFlag::from_legacy_byte(raw).ok_or(Source2Error::UnknownFlag { value: raw })?;
    }
    Ok((NodeType::from_byte(databyte)?, flag))
}

fn legacy_read_value(ctx: &mut LegacyCtx, ty: NodeType, _flag: KvFlag) -> Result<KvValue> {
    ctx.depth += 1;
    if ctx.depth > MAX_DEPTH {
        ctx.depth -= 1;
        return Err(Source2Error::DepthLimitExceeded { limit: MAX_DEPTH });
    }
    let result = legacy_read_value_inner(ctx, ty);
    ctx.depth -= 1;
    result
}

fn legacy_read_value_inner(ctx: &mut LegacyCtx, ty: NodeType) -> Result<KvValue> {
    use NodeType as T;
    Ok(match ty {
        T::Null => KvValue::Null,
        T::Boolean => KvValue::Bool(ctx.buf.u8("bool")? != 0),
        T::BooleanTrue => KvValue::Bool(true),
        T::BooleanFalse => KvValue::Bool(false),
        T::Int64Zero => KvValue::Int(0),
        T::Int64One => KvValue::Int(1),
        T::Int64 => KvValue::Int(ctx.buf.i64("int64")?),
        T::UInt64 => KvValue::UInt(ctx.buf.u64("uint64")?),
        T::Int32 => KvValue::Int(i64::from(ctx.buf.i32("int32")?)),
        T::UInt32 => KvValue::UInt(u64::from(ctx.buf.u32("uint32")?)),
        T::Double => KvValue::Double(ctx.buf.f64("double")?),
        T::DoubleZero => KvValue::Double(0.0),
        T::DoubleOne => KvValue::Double(1.0),
        T::String => {
            let id = ctx.buf.i32("string id")?;
            KvValue::String(ctx.string(id)?)
        }
        T::BinaryBlob => {
            let len = size(ctx.buf.i32("blob length")?, "blob length")?;
            KvValue::Binary(ctx.buf.take(len, "blob bytes")?.to_vec())
        }
        T::Array => {
            let len = size(ctx.buf.i32("array length")?, "array length")?;
            let mut items = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                let (ty, flag) = legacy_read_type(ctx)?;
                items.push(legacy_read_value(ctx, ty, flag)?);
            }
            KvValue::Array(items)
        }
        T::ArrayTyped => {
            let len = size(ctx.buf.i32("typed array length")?, "typed array length")?;
            let (sub_type, sub_flag) = legacy_read_type(ctx)?;
            let mut items = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                items.push(legacy_read_value(ctx, sub_type, sub_flag)?);
            }
            KvValue::TypedArray(items)
        }
        T::Object => {
            let len = size(ctx.buf.i32("object length")?, "object length")?;
            let mut fields = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                // Legacy reads the key id *before* the type byte.
                let id = ctx.buf.i32("member key id")?;
                let name = ctx.string(id)?;
                let (ty, flag) = legacy_read_type(ctx)?;
                fields.push((name, legacy_read_value(ctx, ty, flag)?));
            }
            KvValue::Object(fields)
        }
        other => {
            return Err(Source2Error::UnknownNodeType { value: other as u8 });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_magics() {
        assert!(is_binary_kv3(MAGIC_V0));
        for v in 1..=5u32 {
            assert!(is_binary_kv3(MAGIC_BASE | v));
        }
        assert!(!is_binary_kv3(MAGIC_BASE | 6));
        assert!(!is_binary_kv3(0xDEAD_BEEF));
    }

    #[test]
    fn rejects_bad_magic() {
        let data = 0xDEAD_BEEFu32.to_le_bytes();
        assert!(matches!(
            decode(&data),
            Err(Source2Error::BadKv3Magic { .. })
        ));
    }

    #[test]
    fn rejects_unsupported_version() {
        let data = (MAGIC_BASE | 9).to_le_bytes();
        assert!(matches!(
            decode(&data),
            Err(Source2Error::UnsupportedKv3Version { version: 9 })
        ));
    }

    #[test]
    fn empty_input_errors() {
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn alignment_matches_valve() {
        let mut o = 1;
        align(&mut o, 4);
        assert_eq!(o, 4);
        let mut o = 8;
        align(&mut o, 8);
        assert_eq!(o, 8);
        let mut o = 9;
        align(&mut o, 8);
        assert_eq!(o, 16);
    }

    #[test]
    fn size_rejects_negative_and_huge() {
        assert!(size(-1, "x").is_err());
        assert!(size(i32::MAX, "x").is_err());
        assert_eq!(size(10, "x").unwrap(), 10);
    }

    #[test]
    fn node_types_round_trip() {
        for b in 1..=25u8 {
            assert!(NodeType::from_byte(b).is_ok(), "type {b} should be known");
        }
        assert!(NodeType::from_byte(0).is_err());
        assert!(NodeType::from_byte(26).is_err());
    }
}
