//! Pure-Rust decoders for the two meshoptimizer codecs Source 2 uses.
//!
//! CS2 stores mesh buffers in `MVTX` (vertices) and `MIDX` (indices) blocks
//! compressed with [meshoptimizer]'s vertex and index codecs. Both are
//! byte-exact formats; this module implements the decode side only.
//!
//! * Vertex codec versions 0 and 1 (header byte `0xa0` / `0xa1`). Every CS2
//!   buffer observed so far is version 1, which adds per-channel control bytes
//!   and 16/32-bit delta modes on top of version 0's per-byte-lane scheme.
//! * Index codec versions 0 and 1 (header byte `0xe0` / `0xe1`), for triangle
//!   lists.
//!
//! The format is documented by its reference implementation (MIT licensed):
//! <https://github.com/zeux/meshoptimizer>, `src/vertexcodec.cpp` and
//! `src/indexcodec.cpp`.
//!
//! # Robustness
//!
//! Both decoders are written for untrusted input: every read is bounds-checked
//! and malformed data yields [`Source2Error::Meshopt`] rather than a panic.
//! Decoded indices are *not* range-checked against a vertex count here; callers
//! that need that guarantee should validate separately.
//!
//! [meshoptimizer]: https://github.com/zeux/meshoptimizer

use crate::error::{Result, Source2Error};

/// High nibble of the first byte of a meshopt vertex buffer.
const VERTEX_HEADER: u8 = 0xa0;
/// High nibble of the first byte of a meshopt index buffer.
const INDEX_HEADER: u8 = 0xe0;

/// Target size of one vertex block, in bytes.
const VERTEX_BLOCK_SIZE_BYTES: usize = 8192;
/// Hard cap on the number of vertices in one block.
const VERTEX_BLOCK_MAX_SIZE: usize = 256;
/// Byte groups are always this many bytes wide.
const BYTE_GROUP_SIZE: usize = 16;
/// Worst-case bytes one byte group can consume (16 packed + 8 overflow).
const BYTE_GROUP_DECODE_LIMIT: usize = 24;
/// Minimum reserved tail for vertex codec version 0.
const TAIL_MIN_SIZE_V0: usize = 32;
/// Minimum reserved tail for vertex codec version 1.
const TAIL_MIN_SIZE_V1: usize = 24;
/// Largest vertex stride the codec supports.
const MAX_VERTEX_SIZE: usize = 256;

/// Bit widths selectable by a 2-bit group header in version 0.
const BITS_V0: [u32; 4] = [0, 2, 4, 8];
/// Bit widths for version 1. The 2-bit group header indexes a four-entry
/// *window* into this table chosen by the channel's control value, so control 0
/// sees `{0, 1, 2, 4}` and control 1 sees `{1, 2, 4, 8}`.
const BITS_V1: [u32; 5] = [0, 1, 2, 4, 8];

fn err(codec: &'static str, detail: impl Into<String>) -> Source2Error {
    Source2Error::Meshopt {
        codec,
        detail: detail.into(),
    }
}

/// Number of vertices encoded per block for a given stride.
fn vertex_block_size(vertex_size: usize) -> usize {
    let result = (VERTEX_BLOCK_SIZE_BYTES / vertex_size) & !(BYTE_GROUP_SIZE - 1);
    result.min(VERTEX_BLOCK_MAX_SIZE)
}

fn unzigzag8(v: u8) -> u8 {
    (v & 1).wrapping_neg() ^ (v >> 1)
}

fn unzigzag16(v: u16) -> u16 {
    (v & 1).wrapping_neg() ^ (v >> 1)
}

/// A forward-only cursor that reports overruns instead of panicking.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    codec: &'static str,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], codec: &'static str) -> Self {
        Self {
            data,
            pos: 0,
            codec,
        }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn byte(&mut self) -> Result<u8> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or_else(|| err(self.codec, "read past end of buffer"))?;
        self.pos += 1;
        Ok(b)
    }

    /// Read a byte at an absolute position without moving the cursor.
    fn byte_at(&self, pos: usize) -> Result<u8> {
        self.data
            .get(pos)
            .copied()
            .ok_or_else(|| err(self.codec, "read past end of buffer"))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| err(self.codec, "read past end of buffer"))?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}

// ---------------------------------------------------------------------------
// Vertex codec
// ---------------------------------------------------------------------------

/// Decode one 16-byte group packed at `bits` bits per value.
///
/// Values equal to the all-ones sentinel escape to a full byte taken from the
/// overflow run that follows the packed bytes.
fn decode_bytes_group(cur: &mut Cursor<'_>, out: &mut [u8], bits: u32) -> Result<()> {
    debug_assert_eq!(out.len(), BYTE_GROUP_SIZE);

    match bits {
        0 => {
            out.fill(0);
            Ok(())
        }
        8 => {
            out.copy_from_slice(cur.take(BYTE_GROUP_SIZE)?);
            Ok(())
        }
        1 | 2 | 4 => {
            // Values per packed byte, and how many packed bytes precede the
            // overflow run.
            let per_byte = 8 / bits as usize;
            let packed = BYTE_GROUP_SIZE / per_byte;
            let sentinel = (1u16 << bits) as u8 - 1;

            let mut overflow_pos = cur.pos + packed;
            let mut written = 0usize;

            for data_pos in cur.pos..cur.pos + packed {
                let mut byte = cur.byte_at(data_pos)?;
                // Width 1 is the odd one out: its eight values come out of the
                // byte least-significant bit first, while wider values are
                // taken from the top down.
                let low_first = bits == 1;
                for _ in 0..per_byte {
                    let enc = if low_first {
                        let e = byte & sentinel;
                        byte >>= bits;
                        e
                    } else {
                        let e = byte >> (8 - bits);
                        byte <<= bits;
                        e
                    };
                    out[written] = if enc == sentinel {
                        let v = cur.byte_at(overflow_pos)?;
                        overflow_pos += 1;
                        v
                    } else {
                        enc
                    };
                    written += 1;
                }
            }

            cur.pos = overflow_pos;
            Ok(())
        }
        _ => Err(err("vertex", format!("invalid group bit width {bits}"))),
    }
}

/// Decode one byte lane: `out.len()` bytes split into 16-byte groups, each
/// group's width selected by two bits of a shared header.
fn decode_bytes(cur: &mut Cursor<'_>, out: &mut [u8], bits: &[u32]) -> Result<()> {
    debug_assert_eq!(out.len() % BYTE_GROUP_SIZE, 0);

    let header_size = (out.len() / BYTE_GROUP_SIZE).div_ceil(4);
    let header_start = cur.pos;
    cur.take(header_size)?;

    for (group, chunk) in out.chunks_exact_mut(BYTE_GROUP_SIZE).enumerate() {
        if cur.remaining() < BYTE_GROUP_DECODE_LIMIT {
            return Err(err("vertex", "truncated byte group"));
        }
        let header = cur.byte_at(header_start + group / 4)?;
        let selector = ((header >> ((group % 4) * 2)) & 3) as usize;
        decode_bytes_group(cur, chunk, bits[selector])?;
    }

    Ok(())
}

/// Decode a version 0 block: one independent byte lane per byte of the stride,
/// each delta-coded against the previous vertex.
fn decode_block_v0(
    cur: &mut Cursor<'_>,
    out: &mut [u8],
    vertex_count: usize,
    vertex_size: usize,
    last_vertex: &mut [u8; MAX_VERTEX_SIZE],
) -> Result<()> {
    let aligned = (vertex_count + BYTE_GROUP_SIZE - 1) & !(BYTE_GROUP_SIZE - 1);
    let mut lane = [0u8; VERTEX_BLOCK_MAX_SIZE];

    for k in 0..vertex_size {
        decode_bytes(cur, &mut lane[..aligned], &BITS_V0)?;

        let mut prev = last_vertex[k];
        for i in 0..vertex_count {
            let v = unzigzag8(lane[i]).wrapping_add(prev);
            out[i * vertex_size + k] = v;
            prev = v;
        }
    }

    Ok(())
}

/// Decode a version 1 block.
///
/// The stride is split into four-byte channels. A control byte per channel
/// gives each of its four byte lanes an encoding mode, and a channel byte from
/// the buffer tail says whether the channel reconstructs as 8-bit deltas,
/// 16-bit deltas, or a rotated 32-bit XOR chain.
fn decode_block_v1(
    cur: &mut Cursor<'_>,
    out: &mut [u8],
    vertex_count: usize,
    vertex_size: usize,
    last_vertex: &mut [u8; MAX_VERTEX_SIZE],
    channels: &[u8],
) -> Result<()> {
    let aligned = (vertex_count + BYTE_GROUP_SIZE - 1) & !(BYTE_GROUP_SIZE - 1);
    let control = cur.take(vertex_size / 4)?.to_vec();
    let mut lanes = [[0u8; VERTEX_BLOCK_MAX_SIZE]; 4];

    for (group, &control_byte) in control.iter().enumerate() {
        let k = group * 4;

        for (j, lane) in lanes.iter_mut().enumerate() {
            let mode = (control_byte >> (j * 2)) & 3;
            match mode {
                // All bytes in the lane are zero, and nothing is stored.
                2 => lane[..aligned].fill(0),
                // The lane is stored verbatim.
                3 => lane[..vertex_count].copy_from_slice(cur.take(vertex_count)?),
                // Otherwise the mode picks the bit-width window.
                m => decode_bytes(cur, &mut lane[..aligned], &BITS_V1[m as usize..][..4])?,
            }
        }

        let channel = channels
            .get(group)
            .copied()
            .ok_or_else(|| err("vertex", "channel table shorter than stride"))?;

        match channel & 3 {
            // Per-byte zigzag deltas.
            0 => {
                for (j, lane) in lanes.iter().enumerate() {
                    let mut prev = last_vertex[k + j];
                    for i in 0..vertex_count {
                        let v = unzigzag8(lane[i]).wrapping_add(prev);
                        out[i * vertex_size + k + j] = v;
                        prev = v;
                    }
                }
            }
            // Two 16-bit zigzag delta chains.
            1 => {
                for half in 0..2 {
                    let j = half * 2;
                    let mut prev = u16::from_le_bytes([last_vertex[k + j], last_vertex[k + j + 1]]);
                    for i in 0..vertex_count {
                        let enc = u16::from_le_bytes([lanes[j][i], lanes[j + 1][i]]);
                        let v = unzigzag16(enc).wrapping_add(prev);
                        out[i * vertex_size + k + j..i * vertex_size + k + j + 2]
                            .copy_from_slice(&v.to_le_bytes());
                        prev = v;
                    }
                }
            }
            // One 32-bit XOR chain, with the encoded word rotated first.
            2 => {
                let rot = u32::from(channel >> 4) & 31;
                let mut prev = u32::from_le_bytes([
                    last_vertex[k],
                    last_vertex[k + 1],
                    last_vertex[k + 2],
                    last_vertex[k + 3],
                ]);
                for i in 0..vertex_count {
                    let enc =
                        u32::from_le_bytes([lanes[0][i], lanes[1][i], lanes[2][i], lanes[3][i]]);
                    let v = enc.rotate_right(rot) ^ prev;
                    out[i * vertex_size + k..i * vertex_size + k + 4]
                        .copy_from_slice(&v.to_le_bytes());
                    prev = v;
                }
            }
            other => {
                return Err(err(
                    "vertex",
                    format!("unsupported channel encoding {other}"),
                ))
            }
        }
    }

    Ok(())
}

/// Decode a meshoptimizer-compressed vertex buffer.
///
/// `vertex_size` is the stride in bytes; it must be a non-zero multiple of four
/// and at most 256. Returns `vertex_count * vertex_size` bytes.
///
/// # Errors
///
/// Returns [`Source2Error::Meshopt`] if the header is unrecognised, the codec
/// version is unsupported, or the stream is truncated or inconsistent.
pub fn decode_vertex_buffer(
    vertex_count: usize,
    vertex_size: usize,
    data: &[u8],
) -> Result<Vec<u8>> {
    if vertex_size == 0 || vertex_size > MAX_VERTEX_SIZE || vertex_size % 4 != 0 {
        return Err(err(
            "vertex",
            format!("invalid vertex stride {vertex_size}"),
        ));
    }

    let mut cur = Cursor::new(data, "vertex");
    let header = cur.byte()?;
    if header & 0xf0 != VERTEX_HEADER {
        return Err(err("vertex", format!("bad header byte {header:#04x}")));
    }
    let version = header & 0x0f;
    if version > 1 {
        return Err(err(
            "vertex",
            format!("unsupported codec version {version}"),
        ));
    }

    let mut out = vec![0u8; vertex_count.saturating_mul(vertex_size)];
    if vertex_count == 0 {
        return Ok(out);
    }

    // The tail carries the first vertex (the seed for every delta chain) and,
    // in version 1, the per-channel encoding table. It is front-padded to a
    // minimum size so the decoder can read ahead without bounds checks.
    let control_size = if version == 0 { 0 } else { vertex_size / 4 };
    let tail_size = vertex_size + control_size;
    let tail_min = if version == 0 {
        TAIL_MIN_SIZE_V0
    } else {
        TAIL_MIN_SIZE_V1
    };
    if data.len() < 1 + tail_size.max(tail_min) {
        return Err(err("vertex", "buffer too small to hold the tail"));
    }

    let tail = &data[data.len() - tail_size..];
    let mut last_vertex = [0u8; MAX_VERTEX_SIZE];
    last_vertex[..vertex_size].copy_from_slice(&tail[..vertex_size]);
    let channels = tail[vertex_size..].to_vec();

    let block_size = vertex_block_size(vertex_size);
    if block_size == 0 {
        return Err(err("vertex", "stride too large for a vertex block"));
    }

    let mut offset = 0usize;
    while offset < vertex_count {
        let count = block_size.min(vertex_count - offset);
        let block = &mut out[offset * vertex_size..(offset + count) * vertex_size];

        if version == 0 {
            decode_block_v0(&mut cur, block, count, vertex_size, &mut last_vertex)?;
        } else {
            decode_block_v1(
                &mut cur,
                block,
                count,
                vertex_size,
                &mut last_vertex,
                &channels,
            )?;
        }

        // Every chain continues from the block's final vertex.
        let last = &block[(count - 1) * vertex_size..count * vertex_size];
        last_vertex[..vertex_size].copy_from_slice(last);

        offset += count;
    }

    // The stream must land exactly on the reserved tail.
    let reserved = tail_size.max(tail_min);
    if cur.remaining() != reserved {
        return Err(err(
            "vertex",
            format!(
                "stream ended with {} bytes left, expected the {reserved} byte tail",
                cur.remaining()
            ),
        ));
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Index codec
// ---------------------------------------------------------------------------

/// Sentinel for an unused FIFO slot.
const INVALID_INDEX: u32 = u32::MAX;

fn push_edge_fifo(fifo: &mut [[u32; 2]; 16], a: u32, b: u32, offset: &mut usize) {
    fifo[*offset] = [a, b];
    *offset = (*offset + 1) & 15;
}

fn push_vertex_fifo(fifo: &mut [u32; 16], v: u32, offset: &mut usize, advance: bool) {
    fifo[*offset] = v;
    *offset = (*offset + usize::from(advance)) & 15;
}

/// Read meshoptimizer's variable-length integer encoding.
fn decode_v_byte(cur: &mut Cursor<'_>) -> Result<u32> {
    let lead = u32::from(cur.byte()?);
    if lead < 128 {
        return Ok(lead);
    }

    let mut result = lead & 127;
    let mut shift = 7;
    // At most four continuation bytes, so this always terminates.
    for _ in 0..4 {
        let group = u32::from(cur.byte()?);
        result |= (group & 127) << shift;
        shift += 7;
        if group < 128 {
            break;
        }
    }

    Ok(result)
}

/// Decode a delta-coded index relative to `last`.
fn decode_index(cur: &mut Cursor<'_>, last: u32) -> Result<u32> {
    let v = decode_v_byte(cur)?;
    let d = (v >> 1) ^ (v & 1).wrapping_neg();
    Ok(last.wrapping_add(d))
}

/// Decode a meshoptimizer-compressed triangle-list index buffer.
///
/// `index_count` must be a multiple of three.
///
/// # Errors
///
/// Returns [`Source2Error::Meshopt`] if the header is unrecognised, the codec
/// version is unsupported, or the stream is truncated. The returned indices are
/// not validated against any vertex count.
pub fn decode_index_buffer(index_count: usize, data: &[u8]) -> Result<Vec<u32>> {
    if index_count % 3 != 0 {
        return Err(err(
            "index",
            format!("index count {index_count} is not a multiple of three"),
        ));
    }

    if index_count == 0 {
        return Ok(Vec::new());
    }

    let triangles = index_count / 3;
    // Header byte, one code byte per triangle, and the 16-byte codeaux table.
    if data.len() < 1 + triangles + 16 {
        return Err(err(
            "index",
            "buffer too small for the declared index count",
        ));
    }
    if data[0] & 0xf0 != INDEX_HEADER {
        return Err(err("index", format!("bad header byte {:#04x}", data[0])));
    }
    let version = data[0] & 0x0f;
    if version > 1 {
        return Err(err("index", format!("unsupported codec version {version}")));
    }

    let mut out = vec![0u32; index_count];

    // After the header the stream splits: one code byte per triangle, then a
    // data run that ends with the 16-byte codeaux table.
    let (codes, rest) = data[1..].split_at(triangles);
    let data_safe_end = rest.len() - 16;
    let codeaux_table: [u8; 16] = rest[data_safe_end..]
        .try_into()
        .map_err(|_| err("index", "missing codeaux table"))?;
    let mut cur = Cursor::new(rest, "index");

    let mut edge_fifo = [[INVALID_INDEX; 2]; 16];
    let mut vertex_fifo = [INVALID_INDEX; 16];
    let mut edge_offset = 0usize;
    let mut vertex_offset = 0usize;
    let mut next = 0u32;
    let mut last = 0u32;

    // Version 1 reserves codes 13 and 14 for +/-1 deltas from the last index.
    let fec_max = if version >= 1 { 13 } else { 15 };

    for (triangle, dst) in out.chunks_exact_mut(3).enumerate() {
        if cur.pos > data_safe_end {
            return Err(err("index", "ran past the end of the triangle data"));
        }

        let code = codes[triangle];

        if code < 0xf0 {
            // An edge from the FIFO plus one more vertex.
            let slot = (edge_offset.wrapping_sub((code >> 4) as usize + 1)) & 15;
            let [a, b] = edge_fifo[slot];
            let fec = usize::from(code & 15);

            let c = if fec < fec_max {
                let cf = vertex_fifo[(vertex_offset.wrapping_sub(fec + 1)) & 15];
                let c = if fec == 0 { next } else { cf };
                if fec == 0 {
                    next += 1;
                }
                push_vertex_fifo(&mut vertex_fifo, c, &mut vertex_offset, fec == 0);
                c
            } else {
                // 13 and 14 decode to -1 and +1; 15 means a coded index.
                let c = if fec != 15 {
                    last.wrapping_add((fec.wrapping_sub(fec ^ 3)) as u32)
                } else {
                    decode_index(&mut cur, last)?
                };
                last = c;
                push_vertex_fifo(&mut vertex_fifo, c, &mut vertex_offset, true);
                c
            };

            dst.copy_from_slice(&[a, b, c]);
            push_edge_fifo(&mut edge_fifo, c, b, &mut edge_offset);
            push_edge_fifo(&mut edge_fifo, a, c, &mut edge_offset);
        } else if code < 0xfe {
            // Common vertex-ordering pattern taken from the codeaux table.
            let codeaux = codeaux_table[usize::from(code & 15)];
            let feb = usize::from(codeaux >> 4);
            let fec = usize::from(codeaux & 15);

            let a = next;
            next += 1;

            let bf = vertex_fifo[(vertex_offset.wrapping_sub(feb)) & 15];
            let b = if feb == 0 { next } else { bf };
            if feb == 0 {
                next += 1;
            }

            let cf = vertex_fifo[(vertex_offset.wrapping_sub(fec)) & 15];
            let c = if fec == 0 { next } else { cf };
            if fec == 0 {
                next += 1;
            }

            dst.copy_from_slice(&[a, b, c]);

            push_vertex_fifo(&mut vertex_fifo, a, &mut vertex_offset, true);
            push_vertex_fifo(&mut vertex_fifo, b, &mut vertex_offset, feb == 0);
            push_vertex_fifo(&mut vertex_fifo, c, &mut vertex_offset, fec == 0);

            push_edge_fifo(&mut edge_fifo, b, a, &mut edge_offset);
            push_edge_fifo(&mut edge_fifo, c, b, &mut edge_offset);
            push_edge_fifo(&mut edge_fifo, a, c, &mut edge_offset);
        } else {
            // Fallback: the aux byte is stored inline.
            let codeaux = cur.byte()?;
            let fea = if code == 0xfe { 0 } else { 15 };
            let feb = usize::from(codeaux >> 4);
            let fec = usize::from(codeaux & 15);

            // An all-zero aux byte outside the table signals a strip restart.
            if codeaux == 0 {
                next = 0;
            }

            let mut a = 0u32;
            if fea == 0 {
                a = next;
                next += 1;
            }
            let mut b = if feb == 0 {
                let v = next;
                next += 1;
                v
            } else {
                vertex_fifo[(vertex_offset.wrapping_sub(feb)) & 15]
            };
            let mut c = if fec == 0 {
                let v = next;
                next += 1;
                v
            } else {
                vertex_fifo[(vertex_offset.wrapping_sub(fec)) & 15]
            };

            if fea == 15 {
                a = decode_index(&mut cur, last)?;
                last = a;
            }
            if feb == 15 {
                b = decode_index(&mut cur, last)?;
                last = b;
            }
            if fec == 15 {
                c = decode_index(&mut cur, last)?;
                last = c;
            }

            dst.copy_from_slice(&[a, b, c]);

            push_vertex_fifo(&mut vertex_fifo, a, &mut vertex_offset, true);
            push_vertex_fifo(
                &mut vertex_fifo,
                b,
                &mut vertex_offset,
                feb == 0 || feb == 15,
            );
            push_vertex_fifo(
                &mut vertex_fifo,
                c,
                &mut vertex_offset,
                fec == 0 || fec == 15,
            );

            push_edge_fifo(&mut edge_fifo, b, a, &mut edge_offset);
            push_edge_fifo(&mut edge_fifo, c, b, &mut edge_offset);
            push_edge_fifo(&mut edge_fifo, a, c, &mut edge_offset);
        }
    }

    if cur.pos != data_safe_end {
        return Err(err(
            "index",
            format!(
                "stream ended at {} but the codeaux table starts at {data_safe_end}",
                cur.pos
            ),
        ));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_headers() {
        assert!(decode_vertex_buffer(1, 4, &[0x00; 64]).is_err());
        assert!(decode_index_buffer(3, &[0x00; 64]).is_err());
        // Version 2 of either codec is not something we can decode.
        assert!(decode_vertex_buffer(1, 4, &[0xa2; 64]).is_err());
        assert!(decode_index_buffer(3, &[0xe2; 64]).is_err());
    }

    #[test]
    fn rejects_bad_geometry_parameters() {
        assert!(decode_vertex_buffer(1, 0, &[0xa1; 64]).is_err());
        assert!(decode_vertex_buffer(1, 6, &[0xa1; 64]).is_err());
        assert!(decode_vertex_buffer(1, 512, &[0xa1; 64]).is_err());
        assert!(decode_index_buffer(4, &[0xe1; 64]).is_err());
    }

    #[test]
    fn zero_counts_are_empty() {
        assert!(decode_vertex_buffer(0, 4, &[0xa1; 64]).unwrap().is_empty());
        assert!(decode_index_buffer(0, &[0xe1; 64]).unwrap().is_empty());
    }

    /// A version 0 index buffer produced by the reference encoder.
    const INDEX_V0: [u8; 27] = [
        0xe0, 0xf0, 0x10, 0xfe, 0xff, 0xf0, 0x0c, 0xff, 0x02, 0x02, 0x02, 0x00, 0x76, 0x87, 0x56,
        0x67, 0x78, 0xa9, 0x86, 0x65, 0x89, 0x68, 0x98, 0x01, 0x69, 0x00, 0x00,
    ];

    #[test]
    fn decodes_reference_index_buffer() {
        let decoded = decode_index_buffer(12, &INDEX_V0).expect("decode");
        assert_eq!(decoded, vec![0, 1, 2, 2, 1, 3, 4, 6, 5, 7, 8, 9]);
    }

    #[test]
    fn truncation_never_panics() {
        for cut in 0..INDEX_V0.len() {
            let _ = decode_index_buffer(12, &INDEX_V0[..cut]);
        }
        let vertex = [0xa1u8; 96];
        for cut in 0..vertex.len() {
            let _ = decode_vertex_buffer(4, 12, &vertex[..cut]);
        }
    }

    #[test]
    fn bit_flips_never_panic() {
        for i in 0..INDEX_V0.len() {
            for mask in [0x01u8, 0x40, 0xFF] {
                let mut corrupt = INDEX_V0;
                corrupt[i] ^= mask;
                let _ = decode_index_buffer(12, &corrupt);
            }
        }

        let base = [0x5au8; 160];
        for i in 0..base.len() {
            for mask in [0x01u8, 0x40, 0xFF] {
                let mut corrupt = base;
                corrupt[0] = 0xa1;
                corrupt[i] ^= mask;
                let _ = decode_vertex_buffer(8, 16, &corrupt);
                let _ = decode_vertex_buffer(8, 4, &corrupt);
            }
        }
    }
}
