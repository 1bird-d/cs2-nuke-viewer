//! Hand-built entity lumps, decoded end to end.
//!
//! The `vents_c` files CS2 ships are KV3 version 5 with zstd compression, which
//! makes them useless as a fixture: the whole point is to exercise the layers
//! independently. So these tests build the value tree, encode it as a binary
//! KV3 version 1 payload, wrap that in a Source 2 resource container, and hand
//! the bytes to [`EntityLump::from_resource`] — the same path a real map takes.
//!
//! Version 1 is chosen because it is the simplest container: one uncompressed
//! buffer, no auxiliary regions, no block-compressed blobs.

use source2::entity::EntityValue;
use source2::{EntityLump, KvValue, Resource};

/// The three-byte-high water mark of a KV3 version 1 magic.
const MAGIC_V1: u32 = 0x4B56_3300 | 1;
/// Sentinel closing the value buffer.
const TRAILER: u32 = 0xFFEE_DD00;
/// Header version every Source 2 resource carries.
const HEADER_VERSION: u16 = 12;

/// An arbitrary format GUID; the decoder records it but does not interpret it.
const FORMAT_GUID: [u8; 16] = [
    0x7C, 0x16, 0x12, 0x74, 0xE9, 0x06, 0x98, 0x46, 0xAF, 0xF2, 0xE6, 0x3E, 0xB5, 0x90, 0x37, 0xE7,
];

/// KV3 node type tags used by the encoder below.
mod tag {
    pub const DOUBLE: u8 = 5;
    pub const STRING: u8 = 6;
    pub const ARRAY: u8 = 8;
    pub const OBJECT: u8 = 9;
    pub const ARRAY_TYPED: u8 = 10;
    pub const INT32: u8 = 11;
    pub const BOOL_TRUE: u8 = 13;
    pub const BOOL_FALSE: u8 = 14;
}

/// Encodes a [`KvValue`] tree as a binary KV3 version 1 payload.
///
/// The decoder pulls a value's operands from width-specific buffers in tree
/// order, so the encoder has to walk the tree in exactly the same order: an
/// object writes its length, then per member a type byte, a key id and the
/// member's own operands.
#[derive(Default)]
struct Kv3Writer {
    strings: Vec<String>,
    types: Vec<u8>,
    b4: Vec<i32>,
    b8: Vec<f64>,
}

impl Kv3Writer {
    fn intern(&mut self, s: &str) -> i32 {
        if let Some(i) = self.strings.iter().position(|existing| existing == s) {
            return i32::try_from(i).expect("string table index fits");
        }
        self.strings.push(s.to_string());
        i32::try_from(self.strings.len() - 1).expect("string table index fits")
    }

    fn tag_of(value: &KvValue) -> u8 {
        match value {
            KvValue::Bool(true) => tag::BOOL_TRUE,
            KvValue::Bool(false) => tag::BOOL_FALSE,
            KvValue::Int(_) => tag::INT32,
            KvValue::Double(_) => tag::DOUBLE,
            KvValue::String(_) => tag::STRING,
            KvValue::Array(_) => tag::ARRAY,
            KvValue::TypedArray(_) => tag::ARRAY_TYPED,
            KvValue::Object(_) => tag::OBJECT,
            other => panic!("no encoder for {}", other.type_name()),
        }
    }

    /// Write a value's operands. Its type byte has already been emitted.
    fn write(&mut self, value: &KvValue) {
        match value {
            // Boolean-true and -false are entirely carried by the type byte.
            KvValue::Bool(_) => {}
            KvValue::Int(i) => self
                .b4
                .push(i32::try_from(*i).expect("fixture ints fit in 32 bits")),
            KvValue::Double(d) => self.b8.push(*d),
            KvValue::String(s) => {
                let id = self.intern(s);
                self.b4.push(id);
            }
            KvValue::Array(items) => {
                self.b4
                    .push(i32::try_from(items.len()).expect("array length fits"));
                for item in items {
                    let tag = Self::tag_of(item);
                    self.types.push(tag);
                    self.write(item);
                }
            }
            KvValue::TypedArray(items) => {
                self.b4
                    .push(i32::try_from(items.len()).expect("array length fits"));
                // One shared type byte, then the bare elements.
                let tag = items.first().map_or(tag::INT32, Self::tag_of);
                self.types.push(tag);
                for item in items {
                    self.write(item);
                }
            }
            KvValue::Object(fields) => {
                self.b4
                    .push(i32::try_from(fields.len()).expect("object length fits"));
                for (key, member) in fields {
                    let tag = Self::tag_of(member);
                    self.types.push(tag);
                    let id = self.intern(key);
                    self.b4.push(id);
                    self.write(member);
                }
            }
            other => panic!("no encoder for {}", other.type_name()),
        }
    }

    /// Encode `root` into a complete KV3 version 1 payload.
    fn encode(root: &KvValue) -> Vec<u8> {
        let mut w = Self::default();
        // The decoder reads the string count as the very first 4-byte value,
        // so reserve its slot and patch it once the table is known.
        w.b4.push(0);
        let root_tag = Self::tag_of(root);
        w.types.push(root_tag);
        w.write(root);
        w.b4[0] = i32::try_from(w.strings.len()).expect("string count fits");

        let mut buf = Vec::new();
        // No 1-byte region; the 4-byte region starts at offset 0.
        for value in &w.b4 {
            buf.extend_from_slice(&value.to_le_bytes());
        }
        // The 8-byte region is 8-aligned, and versions below 5 align here even
        // when it is empty.
        while buf.len() % 8 != 0 {
            buf.push(0);
        }
        for value in &w.b8 {
            buf.extend_from_slice(&value.to_le_bytes());
        }
        for string in &w.strings {
            buf.extend_from_slice(string.as_bytes());
            buf.push(0);
        }
        buf.extend_from_slice(&w.types);
        buf.extend_from_slice(&TRAILER.to_le_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_V1.to_le_bytes());
        out.extend_from_slice(&FORMAT_GUID);
        out.extend_from_slice(&0u32.to_le_bytes()); // compression method: none
        out.extend_from_slice(&0i32.to_le_bytes()); // count bytes1
        out.extend_from_slice(
            &i32::try_from(w.b4.len())
                .expect("4-byte count fits")
                .to_le_bytes(),
        );
        out.extend_from_slice(
            &i32::try_from(w.b8.len())
                .expect("8-byte count fits")
                .to_le_bytes(),
        );
        out.extend_from_slice(
            &i32::try_from(buf.len())
                .expect("payload size fits")
                .to_le_bytes(),
        );
        out.extend_from_slice(&buf);
        out
    }
}

/// Wrap a payload as the `DATA` block of a Source 2 resource.
fn resource_with_data(data: &[u8]) -> Vec<u8> {
    // Header is 16 bytes, then one 12-byte directory entry, then the payload.
    const HEADER: u32 = 16;
    const ENTRY: u32 = 12;

    let mut out = Vec::new();
    let total = HEADER + ENTRY + u32::try_from(data.len()).expect("payload fits");
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&HEADER_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // resource version
                                                // Block offset is relative to its own position, which is 8 bytes in.
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // block count

    out.extend_from_slice(b"DATA");
    // The block's offset is relative to its own position: 4 bytes of fourcc
    // are behind us, and the offset and size words are ahead.
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(data.len())
            .expect("payload fits")
            .to_le_bytes(),
    );
    out.extend_from_slice(data);
    out
}

fn string(s: &str) -> KvValue {
    KvValue::String(s.to_string())
}

fn vector(v: [f64; 3]) -> KvValue {
    KvValue::TypedArray(v.iter().copied().map(KvValue::Double).collect())
}

/// One entity in the modern shape: a `keyValues3Data` object of plain keys.
fn entity(values: Vec<(String, KvValue)>) -> KvValue {
    KvValue::Object(vec![(
        "keyValues3Data".to_string(),
        KvValue::Object(vec![
            ("version".to_string(), KvValue::Int(1)),
            ("values".to_string(), KvValue::Object(values)),
            ("attributes".to_string(), KvValue::Object(vec![])),
        ]),
    )])
}

/// A lump holding the given entities and child lump paths.
fn lump(entities: Vec<KvValue>, children: &[&str]) -> Vec<u8> {
    let root = KvValue::Object(vec![
        ("m_entityKeyValues".to_string(), KvValue::Array(entities)),
        (
            "m_childLumps".to_string(),
            KvValue::Array(children.iter().map(|c| string(c)).collect()),
        ),
    ]);
    resource_with_data(&Kv3Writer::encode(&root))
}

#[test]
fn the_fixture_encoder_round_trips_through_the_kv3_decoder() {
    // If the encoder and the decoder disagree the rest of this file proves
    // nothing, so pin the two against each other first.
    let root = KvValue::Object(vec![
        ("name".to_string(), string("worldspawn")),
        ("count".to_string(), KvValue::Int(-7)),
        ("flag".to_string(), KvValue::Bool(true)),
        ("origin".to_string(), vector([1.5, -2.5, 3.5])),
        (
            "mixed".to_string(),
            KvValue::Array(vec![KvValue::Int(1), string("two")]),
        ),
    ]);
    let doc = source2::kv3::decode(&Kv3Writer::encode(&root)).expect("decode");
    assert_eq!(doc.version, 1);
    assert_eq!(doc.root, root);
}

#[test]
fn decodes_a_lump_of_spawn_points() {
    let bytes = lump(
        vec![
            entity(vec![
                ("classname".to_string(), string("worldspawn")),
                ("skyname".to_string(), string("sky_day01_01")),
            ]),
            entity(vec![
                (
                    "classname".to_string(),
                    string("info_player_counterterrorist"),
                ),
                ("origin".to_string(), vector([382.0, 2102.0, -109.0])),
                ("angles".to_string(), vector([0.0, 101.0, 0.0])),
                ("priority".to_string(), KvValue::Int(1)),
                ("enabled".to_string(), KvValue::Bool(true)),
            ]),
            entity(vec![
                ("classname".to_string(), string("info_player_terrorist")),
                ("origin".to_string(), vector([-822.0, -795.0, 150.0])),
                ("angles".to_string(), vector([0.0, 107.0, 0.0])),
            ]),
        ],
        &["maps/de_test/entities/more.vents"],
    );

    let resource = Resource::parse(&bytes).expect("resource parses");
    let lump = EntityLump::from_resource(&resource).expect("lump decodes");

    assert_eq!(lump.entities.len(), 3);
    assert_eq!(lump.failed_entries, 0);
    assert_eq!(lump.child_lumps, vec!["maps/de_test/entities/more.vents"]);

    let ct: Vec<_> = lump.by_classname("info_player_counterterrorist").collect();
    assert_eq!(ct.len(), 1);
    assert_eq!(ct[0].origin(), Some([382.0, 2102.0, -109.0]));
    assert_eq!(ct[0].yaw_degrees(), Some(101.0));
    assert_eq!(ct[0].number("priority"), Some(1.0));
    assert_eq!(ct[0].get("enabled"), Some(&EntityValue::Bool(true)));

    let t: Vec<_> = lump.by_classname("info_player_terrorist").collect();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].origin(), Some([-822.0, -795.0, 150.0]));

    // Classname matching ignores case, the way Valve's tooling does.
    assert_eq!(lump.by_classname("WORLDSPAWN").count(), 1);
    assert_eq!(lump.by_classname("info_player_nobody").count(), 0);
}

#[test]
fn an_empty_lump_decodes_to_nothing() {
    let bytes = lump(Vec::new(), &[]);
    let resource = Resource::parse(&bytes).expect("resource parses");
    let lump = EntityLump::from_resource(&resource).expect("lump decodes");
    assert!(lump.entities.is_empty());
    assert!(lump.child_lumps.is_empty());
    assert_eq!(lump.failed_entries, 0);
}

#[test]
fn entries_that_carry_nothing_usable_are_counted_not_fatal() {
    // A lump whose entries are the wrong shape entirely: the good one still
    // comes through and the rest are counted.
    let root = KvValue::Object(vec![(
        "m_entityKeyValues".to_string(),
        KvValue::Array(vec![
            KvValue::Int(0),
            KvValue::Object(vec![]),
            entity(vec![("classname".to_string(), string("func_buyzone"))]),
        ]),
    )]);
    let bytes = resource_with_data(&Kv3Writer::encode(&root));

    let resource = Resource::parse(&bytes).expect("resource parses");
    let lump = EntityLump::from_resource(&resource).expect("lump decodes");
    assert_eq!(lump.entities.len(), 1);
    assert_eq!(lump.entities[0].classname(), Some("func_buyzone"));
    assert_eq!(lump.failed_entries, 2);
}

#[test]
fn a_resource_without_a_data_block_is_an_error_not_a_panic() {
    // A resource whose only block is not DATA.
    let mut out = Vec::new();
    out.extend_from_slice(&28u32.to_le_bytes());
    out.extend_from_slice(&HEADER_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(b"RERL");
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    let resource = Resource::parse(&out).expect("resource parses");
    assert!(EntityLump::from_resource(&resource).is_err());
}

/// Truncating and corrupting a valid lump must produce errors, not panics.
#[test]
fn malformed_lumps_never_panic() {
    let good = lump(
        vec![entity(vec![
            ("classname".to_string(), string("info_player_terrorist")),
            ("origin".to_string(), vector([1.0, 2.0, 3.0])),
        ])],
        &["a"],
    );

    // Every truncation.
    for len in 0..good.len() {
        if let Ok(resource) = Resource::parse(&good[..len]) {
            let _ = EntityLump::from_resource(&resource);
        }
    }

    // Every single-byte increment, which walks type tags, counts and offsets
    // through their invalid values.
    for index in 0..good.len() {
        let mut corrupt = good.clone();
        corrupt[index] = corrupt[index].wrapping_add(1);
        if let Ok(resource) = Resource::parse(&corrupt) {
            let _ = EntityLump::from_resource(&resource);
        }
    }

    // And a sweep of high-entropy bytes over the payload, which is where the
    // counts and string indices live.
    for index in 0..good.len() {
        for byte in [0x00, 0x7F, 0x80, 0xFF] {
            let mut corrupt = good.clone();
            corrupt[index] = byte;
            if let Ok(resource) = Resource::parse(&corrupt) {
                let _ = EntityLump::from_resource(&resource);
            }
        }
    }
}
