//! Hand-built binary KV3 payloads.
//!
//! Current CS2 content is entirely KV3 version 5, so the older container
//! versions would otherwise go untested. Each fixture is assembled byte by byte
//! here with the layout spelled out in comments.

use source2::kv3::{self, KvValue};

const MAGIC_V0: u32 = 0x0356_4B56;
const MAGIC_BASE: u32 = 0x4B56_3300;
const TRAILER: u32 = 0xFFEE_DD00;
const LEGACY_TRAILER: u32 = 0xFFFF_FFFF;

/// An arbitrary format GUID; the decoder records it but does not interpret it.
const FORMAT_GUID: [u8; 16] = [
    0x7C, 0x16, 0x12, 0x74, 0xE9, 0x06, 0x98, 0x46, 0xAF, 0xF2, 0xE6, 0x3E, 0xB5, 0x90, 0x37, 0xE7,
];

/// `KV3_ENCODING_BINARY_UNCOMPRESSED`.
const ENCODING_UNCOMPRESSED: [u8; 16] = [
    0x00, 0x05, 0x86, 0x1B, 0xD8, 0xF7, 0xC1, 0x40, 0xAD, 0x82, 0x75, 0xA4, 0x82, 0x67, 0xE7, 0x14,
];
/// `KV3_ENCODING_BINARY_BLOCK_LZ4`.
const ENCODING_LZ4: [u8; 16] = [
    0x8A, 0x34, 0x47, 0x68, 0xA1, 0x63, 0x5C, 0x4F, 0xA1, 0x97, 0x53, 0x80, 0x6F, 0xD9, 0xB1, 0x19,
];

/// The value tree every version-1-through-4 fixture below encodes.
///
/// `{ a = 1, b = "hi" }`
fn build_simple_buffer() -> (Vec<u8>, usize, usize) {
    // 4-byte values, consumed strictly in this order by the decoder:
    //   [0] string count            = 3
    //   [1] root object length      = 2
    //   [2] member 1 key id         = 0  ("a")
    //   [3] member 1 value (int32)  = 1
    //   [4] member 2 key id         = 1  ("b")
    //   [5] member 2 value string id= 2  ("hi")
    let b4: [i32; 6] = [3, 2, 0, 1, 1, 2];

    let mut buf = Vec::new();
    // No 1-byte or 2-byte values; the 4-byte region starts at offset 0.
    for v in b4 {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    // 8-byte region is empty, but versions below 5 still align to 8 here.
    while buf.len() % 8 != 0 {
        buf.push(0);
    }

    let strings_start = buf.len();
    buf.extend_from_slice(b"a\0b\0hi\0");
    let strings_len = buf.len() - strings_start;

    // Type stream: OBJECT, then INT32 and STRING for the two members.
    let types: [u8; 3] = [9, 11, 6];
    buf.extend_from_slice(&types);

    buf.extend_from_slice(&TRAILER.to_le_bytes());

    (buf, strings_len, types.len())
}

fn assert_simple(value: &KvValue) {
    let fields = value.as_object().expect("root should be an object");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].0, "a");
    assert_eq!(fields[0].1, KvValue::Int(1));
    assert_eq!(fields[1].0, "b");
    assert_eq!(fields[1].1, KvValue::String("hi".to_string()));
    assert_eq!(value.get("b").and_then(KvValue::as_str), Some("hi"));
}

#[test]
fn decodes_version_1_uncompressed() {
    let (buf, _strings_len, _types_len) = build_simple_buffer();

    let mut out = Vec::new();
    out.extend_from_slice(&(MAGIC_BASE | 1).to_le_bytes());
    out.extend_from_slice(&FORMAT_GUID);
    out.extend_from_slice(&0u32.to_le_bytes()); // compression method: none
    out.extend_from_slice(&0i32.to_le_bytes()); // count bytes1
    out.extend_from_slice(&6i32.to_le_bytes()); // count bytes4
    out.extend_from_slice(&0i32.to_le_bytes()); // count bytes8
    out.extend_from_slice(&(buf.len() as i32).to_le_bytes()); // uncompressed size
    out.extend_from_slice(&buf);

    let doc = kv3::decode(&out).expect("decode v1");
    assert_eq!(doc.version, 1);
    assert_eq!(doc.compression_method, 0);
    assert_eq!(doc.compression_name(), "none");
    assert_simple(&doc.root);
}

/// The header fields versions 2, 3 and 4 share.
struct V2Header {
    version: u8,
    compression_method: u32,
    frame_size: u16,
    /// Counts of 1-, 2-, 4- and 8-byte values.
    counts: (i32, i32, i32, i32),
    count_types: i32,
    uncompressed: i32,
    compressed: i32,
}

impl V2Header {
    /// Uncompressed defaults for the shared `{ a = 1, b = "hi" }` fixture.
    fn simple(version: u8, count_types: i32, size: i32) -> Self {
        Self {
            version,
            compression_method: 0,
            frame_size: 0,
            counts: (0, 0, 6, 0),
            count_types,
            uncompressed: size,
            compressed: size,
        }
    }
}

fn write_v2_style_header(out: &mut Vec<u8>, h: &V2Header) {
    let V2Header {
        version,
        compression_method,
        frame_size,
        counts,
        count_types,
        uncompressed,
        compressed,
    } = *h;
    let (c1, c2, c4, c8) = counts;
    out.extend_from_slice(&(MAGIC_BASE | u32::from(version)).to_le_bytes());
    out.extend_from_slice(&FORMAT_GUID);
    out.extend_from_slice(&compression_method.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // compression dictionary id
    out.extend_from_slice(&frame_size.to_le_bytes());
    out.extend_from_slice(&c1.to_le_bytes());
    out.extend_from_slice(&c4.to_le_bytes());
    out.extend_from_slice(&c8.to_le_bytes());
    out.extend_from_slice(&count_types.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // object count (advisory)
    out.extend_from_slice(&0u16.to_le_bytes()); // array count (advisory)
    out.extend_from_slice(&uncompressed.to_le_bytes());
    out.extend_from_slice(&compressed.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // block count
    out.extend_from_slice(&0i32.to_le_bytes()); // binary blob size
    if version >= 4 {
        out.extend_from_slice(&c2.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes()); // block compressed sizes
    }
}

#[test]
fn decodes_version_2_uncompressed() {
    let (buf, strings_len, types_len) = build_simple_buffer();
    // From version 2 on, `count_types` spans the string table and type bytes.
    let count_types = (strings_len + types_len) as i32;

    let mut out = Vec::new();
    write_v2_style_header(
        &mut out,
        &V2Header::simple(2, count_types, buf.len() as i32),
    );
    out.extend_from_slice(&buf);

    let doc = kv3::decode(&out).expect("decode v2");
    assert_eq!(doc.version, 2);
    assert_simple(&doc.root);
}

#[test]
fn decodes_version_3_uncompressed() {
    // Version 3 only changes how flag bytes are interpreted; an unflagged
    // payload is byte-identical to version 2.
    let (buf, strings_len, types_len) = build_simple_buffer();
    let count_types = (strings_len + types_len) as i32;

    let mut out = Vec::new();
    write_v2_style_header(
        &mut out,
        &V2Header::simple(3, count_types, buf.len() as i32),
    );
    out.extend_from_slice(&buf);

    let doc = kv3::decode(&out).expect("decode v3");
    assert_eq!(doc.version, 3);
    assert_simple(&doc.root);
}

#[test]
fn decodes_version_2_lz4() {
    let (buf, strings_len, types_len) = build_simple_buffer();
    let count_types = (strings_len + types_len) as i32;
    let compressed = lz4_flex::block::compress(&buf);

    let mut out = Vec::new();
    write_v2_style_header(
        &mut out,
        &V2Header {
            compression_method: 1,
            // The frame size the format mandates from version 2 on.
            frame_size: 16384,
            compressed: compressed.len() as i32,
            ..V2Header::simple(2, count_types, buf.len() as i32)
        },
    );
    out.extend_from_slice(&compressed);

    let doc = kv3::decode(&out).expect("decode v2 lz4");
    assert_eq!(doc.compression_name(), "lz4");
    assert_simple(&doc.root);
}

#[test]
fn rejects_wrong_lz4_frame_size() {
    let (buf, strings_len, types_len) = build_simple_buffer();
    let count_types = (strings_len + types_len) as i32;
    let compressed = lz4_flex::block::compress(&buf);

    let mut out = Vec::new();
    write_v2_style_header(
        &mut out,
        &V2Header {
            compression_method: 1,
            frame_size: 4096, // not the mandated 16384
            compressed: compressed.len() as i32,
            ..V2Header::simple(2, count_types, buf.len() as i32)
        },
    );
    out.extend_from_slice(&compressed);

    assert!(kv3::decode(&out).is_err());
}

#[test]
fn decodes_version_4_with_narrow_types() {
    // `{ f = 1.5, s = -3, n = 200 }` using the FLOAT, INT16 and INT32_AS_BYTE
    // node types, which only exist from version 4 and exercise the 1- and
    // 2-byte value regions plus their alignment.
    let mut buf = Vec::new();

    // 1-byte region: the INT32_AS_BYTE value.
    buf.push(200);
    // 2-byte region, aligned to 2.
    while buf.len() % 2 != 0 {
        buf.push(0);
    }
    buf.extend_from_slice(&(-3i16).to_le_bytes());
    // 4-byte region, aligned to 4.
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
    buf.extend_from_slice(&3i32.to_le_bytes()); // string count
    buf.extend_from_slice(&3i32.to_le_bytes()); // root object length
    buf.extend_from_slice(&0i32.to_le_bytes()); // key "f"
    buf.extend_from_slice(&1.5f32.to_le_bytes()); // float value
    buf.extend_from_slice(&1i32.to_le_bytes()); // key "s"
    buf.extend_from_slice(&2i32.to_le_bytes()); // key "n"
                                                // Empty 8-byte region still forces alignment before the string table.
    while buf.len() % 8 != 0 {
        buf.push(0);
    }

    let strings_start = buf.len();
    buf.extend_from_slice(b"f\0s\0n\0");
    let strings_len = buf.len() - strings_start;

    let types: [u8; 4] = [9, 19, 20, 23]; // OBJECT, FLOAT, INT16, INT32_AS_BYTE
    buf.extend_from_slice(&types);
    buf.extend_from_slice(&TRAILER.to_le_bytes());

    let count_types = (strings_len + types.len()) as i32;

    let mut out = Vec::new();
    write_v2_style_header(
        &mut out,
        &V2Header {
            // One 1-byte and one 2-byte value, which only version 4 supports.
            counts: (1, 1, 6, 0),
            ..V2Header::simple(4, count_types, buf.len() as i32)
        },
    );
    out.extend_from_slice(&buf);

    let doc = kv3::decode(&out).expect("decode v4");
    assert_eq!(doc.version, 4);
    let fields = doc.root.as_object().expect("object");
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0], ("f".to_string(), KvValue::Double(1.5)));
    assert_eq!(fields[1], ("s".to_string(), KvValue::Int(-3)));
    assert_eq!(fields[2], ("n".to_string(), KvValue::Int(200)));
}

/// The legacy container stores everything inline rather than in split buffers.
fn build_legacy_payload() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&3u32.to_le_bytes()); // string count
    p.extend_from_slice(b"a\0b\0hi\0");

    p.push(9); // OBJECT
    p.extend_from_slice(&2i32.to_le_bytes()); // two members
                                              // Legacy reads the key id *before* the type byte.
    p.extend_from_slice(&0i32.to_le_bytes()); // key "a"
    p.push(11); // INT32
    p.extend_from_slice(&1i32.to_le_bytes());
    p.extend_from_slice(&1i32.to_le_bytes()); // key "b"
    p.push(6); // STRING
    p.extend_from_slice(&2i32.to_le_bytes()); // "hi"

    p.extend_from_slice(&LEGACY_TRAILER.to_le_bytes());
    p
}

#[test]
fn decodes_legacy_version_0_uncompressed() {
    let payload = build_legacy_payload();

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC_V0.to_le_bytes());
    out.extend_from_slice(&ENCODING_UNCOMPRESSED);
    out.extend_from_slice(&FORMAT_GUID);
    out.extend_from_slice(&payload);

    let doc = kv3::decode(&out).expect("decode v0");
    assert_eq!(doc.version, 0);
    assert!(doc.encoding.is_some());
    assert_simple(&doc.root);
}

#[test]
fn decodes_legacy_version_0_lz4() {
    let payload = build_legacy_payload();
    let compressed = lz4_flex::block::compress(&payload);

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC_V0.to_le_bytes());
    out.extend_from_slice(&ENCODING_LZ4);
    out.extend_from_slice(&FORMAT_GUID);
    out.extend_from_slice(&(payload.len() as i32).to_le_bytes());
    out.extend_from_slice(&compressed);

    let doc = kv3::decode(&out).expect("decode v0 lz4");
    assert_simple(&doc.root);
}

#[test]
fn detects_bad_trailer() {
    let (mut buf, strings_len, types_len) = build_simple_buffer();
    let count_types = (strings_len + types_len) as i32;
    let last = buf.len() - 1;
    buf[last] ^= 0xFF;

    let mut out = Vec::new();
    write_v2_style_header(
        &mut out,
        &V2Header::simple(2, count_types, buf.len() as i32),
    );
    out.extend_from_slice(&buf);

    assert!(kv3::decode(&out).is_err());
}

/// Truncating and corrupting a valid payload must produce errors, not panics.
#[test]
fn malformed_payloads_never_panic() {
    let (buf, strings_len, types_len) = build_simple_buffer();
    let count_types = (strings_len + types_len) as i32;

    let mut valid = Vec::new();
    write_v2_style_header(
        &mut valid,
        &V2Header::simple(2, count_types, buf.len() as i32),
    );
    valid.extend_from_slice(&buf);

    for cut in 0..valid.len() {
        let _ = kv3::decode(&valid[..cut]);
    }

    for i in 0..valid.len() {
        for mask in [0x01u8, 0x20, 0x80, 0xFF] {
            let mut corrupt = valid.clone();
            corrupt[i] ^= mask;
            let _ = kv3::decode(&corrupt);
        }
    }

    // Also throw structured garbage at it: valid magic, nonsense everything.
    for seed in 0u32..512 {
        let mut junk = (MAGIC_BASE | (seed % 5 + 1)).to_le_bytes().to_vec();
        junk.extend((0..96).map(|i| (seed.wrapping_mul(i * 7 + 13) & 0xFF) as u8));
        let _ = kv3::decode(&junk);
    }
}
