//! Typed decoding of entity lumps (`vents_c`).
//!
//! A world names its entity lumps in `m_entityLumps`; each lump resource holds
//! `m_entityKeyValues`, one entry per entity, and `m_childLumps`, further lump
//! paths to recurse into. CS2's shipped maps put everything in one lump with no
//! children, but the recursion exists in the format and is followed here.
//!
//! # Two encodings
//!
//! Every entity entry carries its properties in one of two ways, and this
//! decoder reads both:
//!
//! * **KV3 object** — `keyValues3Data` is a nested object of the form
//!   `{ version, values { .. }, attributes { .. } }` whose keys are plain
//!   strings. This is what every CS2 map ships today, and it needs no special
//!   handling beyond walking the tree.
//! * **Legacy binary blob** — `m_keyValuesData` is an opaque byte string with
//!   its own little format: a version word, a count of hash-keyed fields, a
//!   count of string-keyed fields, then the fields themselves. Hash-keyed
//!   fields identify their key only by a 32-bit token, which is MurmurHash2 of
//!   the lower-cased key name seeded with `0x31415926`. The token is not
//!   invertible, so [`known_key_for_token`] hashes a table of key names that
//!   matter and matches against that; anything else keeps a `0x…` placeholder
//!   name so the entity is still usable.
//!
//! Both layouts follow ValveResourceFormat's `EntityLump`. The field type
//! numbering is Valve's `EntityFieldType`, of which the nine types their reader
//! implements are the nine implemented here.

use std::collections::HashMap;

use crate::error::{Result, Source2Error};
use crate::kv3::KvValue;
use crate::reader::Reader;
use crate::resource::Resource;

/// Version word the legacy binary blob must start with.
const LEGACY_BLOB_VERSION: u32 = 1;

/// Seed Valve uses for entity key string tokens. It is pi.
const MURMUR2_SEED: u32 = 0x3141_5926;

/// Refuse a blob claiming more fields than it could possibly hold. Each field
/// costs at least a 4-byte hash plus a 4-byte type.
const MIN_FIELD_BYTES: usize = 8;

/// Key names hashed into tokens so hash-keyed legacy fields can be named.
///
/// A token cannot be reversed, so recognising one means hashing a candidate and
/// comparing. This list covers the placement and identity keys the exporter
/// reads plus the common ones a later milestone will want; an unrecognised
/// token keeps its hexadecimal placeholder rather than being dropped.
const KNOWN_KEYS: &[&str] = &[
    "classname",
    "origin",
    "angles",
    "scales",
    "targetname",
    "model",
    "modelscale",
    "parentname",
    "spawnflags",
    "rendercolor",
    "renderamt",
    "hammeruniqueid",
    "priority",
    "enabled",
    "startdisabled",
    "target",
    "message",
    "skyname",
    "worldname",
    "mapusagetype",
    "bomb_site_designation",
    "bomb_damage_power",
    "team_number",
    "group_name",
    "is_buy_zone",
    "buy_zone_team",
    "cs_team_number",
];

/// Valve's `EntityFieldType`, restricted to the types their reader decodes.
///
/// The numbering is the full enum's; the variants here are the ones that carry
/// a payload this decoder knows the width of. An unlisted type stops the walk,
/// because field widths are not self-describing and guessing one would
/// desynchronise everything after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldType {
    Float = 0x01,
    Vector = 0x03,
    Integer = 0x05,
    Boolean = 0x06,
    Color32 = 0x09,
    Integer64 = 0x1a,
    CString = 0x1e,
    UInt64 = 0x21,
    Float64 = 0x22,
    UInt = 0x25,
    QAngle = 0x27,
}

impl FieldType {
    fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            0x01 => Self::Float,
            0x03 => Self::Vector,
            0x05 => Self::Integer,
            0x06 => Self::Boolean,
            0x09 => Self::Color32,
            0x1a => Self::Integer64,
            0x1e => Self::CString,
            0x21 => Self::UInt64,
            0x22 => Self::Float64,
            0x25 => Self::UInt,
            0x27 => Self::QAngle,
            _ => return None,
        })
    }
}

/// One decoded entity property.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityValue {
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer.
    UInt(u64),
    /// A float or double.
    Double(f64),
    /// A string. Numeric keys are often stored this way in Hammer's output, so
    /// [`Entity::number`] parses strings too.
    String(String),
    /// A three-component vector: a position, a set of Euler angles, or a scale.
    Vector([f32; 3]),
    /// An RGBA colour.
    Color([u8; 4]),
}

impl EntityValue {
    /// The value as a string slice, if it is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a number, parsing a string if that is how it is stored.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Double(d) => Some(*d),
            #[allow(clippy::cast_precision_loss)]
            Self::Int(i) => Some(*i as f64),
            #[allow(clippy::cast_precision_loss)]
            Self::UInt(u) => Some(*u as f64),
            Self::Bool(b) => Some(f64::from(u8::from(*b))),
            Self::String(s) => s.trim().parse().ok(),
            Self::Vector(_) | Self::Color(_) => None,
        }
    }

    /// The value as a three-component vector.
    #[must_use]
    pub fn as_vec3(&self) -> Option<[f32; 3]> {
        match self {
            Self::Vector(v) => Some(*v),
            #[allow(clippy::cast_lossless)]
            Self::Color(c) => Some([f32::from(c[0]), f32::from(c[1]), f32::from(c[2])]),
            _ => None,
        }
    }
}

/// One entity: an ordered list of key/value properties.
///
/// Keys can repeat in principle, so this is a list rather than a map, matching
/// [`KvValue::Object`]. Keys are lower-cased on the way in, because the two
/// encodings disagree on case — `hammerUniqueId` in KV3 is `hammeruniqueid`
/// once hashed — and lookups should not have to care.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entity {
    /// The entity's properties, in the order the file listed them.
    pub values: Vec<(String, EntityValue)>,
}

impl Entity {
    /// Look up a property by (lower-case) key, returning the first match.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&EntityValue> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The entity's `classname`, e.g. `info_player_terrorist`.
    #[must_use]
    pub fn classname(&self) -> Option<&str> {
        self.get("classname").and_then(EntityValue::as_str)
    }

    /// A property as a string.
    #[must_use]
    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(EntityValue::as_str)
    }

    /// A property as a number, parsing string-stored numbers.
    #[must_use]
    pub fn number(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(EntityValue::as_f64)
    }

    /// A property as a three-component vector.
    #[must_use]
    pub fn vec3(&self, key: &str) -> Option<[f32; 3]> {
        self.get(key).and_then(EntityValue::as_vec3)
    }

    /// The entity's position in Source units, from its `origin`.
    #[must_use]
    pub fn origin(&self) -> Option<[f32; 3]> {
        self.vec3("origin")
    }

    /// The entity's Euler angles in degrees as `[pitch, yaw, roll]`.
    #[must_use]
    pub fn angles(&self) -> Option<[f32; 3]> {
        self.vec3("angles")
    }

    /// The entity's yaw in degrees: rotation about Source's up axis, and the
    /// only one of the three angles a standing player uses.
    #[must_use]
    pub fn yaw_degrees(&self) -> Option<f32> {
        self.angles().map(|a| a[1])
    }
}

/// A decoded `vents_c` resource.
#[derive(Debug, Clone, Default)]
pub struct EntityLump {
    /// The entities this lump declares.
    pub entities: Vec<Entity>,
    /// Paths of further lumps to load, without the `_c` suffix.
    pub child_lumps: Vec<String>,
    /// Entries that could not be decoded. Malformed entities are skipped rather
    /// than failing the lump, and counted here so callers can report them.
    pub failed_entries: usize,
}

impl EntityLump {
    /// Decode an entity lump resource.
    ///
    /// Individual entities that fail to decode are skipped and counted in
    /// [`Self::failed_entries`]; only a missing or non-KV3 `DATA` block is an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns [`Source2Error`] if the resource has no KV3 `DATA` block.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource
            .data_kv3()
            .ok_or(Source2Error::MissingBlock { fourcc: "DATA" })??;

        let child_lumps = doc
            .get("m_childLumps")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect();

        let mut entities = Vec::new();
        let mut failed_entries = 0;
        for entry in doc
            .get("m_entityKeyValues")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
        {
            match Entity::from_entry(entry) {
                Some(entity) if !entity.values.is_empty() => entities.push(entity),
                _ => failed_entries += 1,
            }
        }

        Ok(Self {
            entities,
            child_lumps,
            failed_entries,
        })
    }

    /// Every entity whose classname matches, case-insensitively.
    pub fn by_classname<'a>(&'a self, classname: &'a str) -> impl Iterator<Item = &'a Entity> {
        self.entities.iter().filter(move |e| {
            e.classname()
                .is_some_and(|c| c.eq_ignore_ascii_case(classname))
        })
    }
}

impl Entity {
    /// Decode one `m_entityKeyValues` entry, whichever encoding it uses.
    fn from_entry(entry: &KvValue) -> Option<Self> {
        // Modern: a nested KV3 object. Preferred when both are present, since
        // CS2 writes an empty legacy blob alongside it.
        if let Some(kv3) = entry.get("keyValues3Data") {
            let mut values = Vec::new();
            // `values` holds the entity's properties; `attributes` holds the
            // handful Hammer stores separately. Both are flat objects.
            for section in ["values", "attributes"] {
                for (key, value) in kv3.get(section).and_then(KvValue::as_object).unwrap_or(&[]) {
                    if let Some(v) = EntityValue::from_kv(value) {
                        values.push((key.to_ascii_lowercase(), v));
                    }
                }
            }
            if !values.is_empty() {
                return Some(Self { values });
            }
        }

        // Legacy: an opaque blob. An empty one is not a failure, it just means
        // the entity lives in the KV3 form above.
        let blob = entry.get("m_keyValuesData").and_then(KvValue::as_bytes)?;
        if blob.is_empty() {
            return None;
        }
        parse_legacy_blob(blob).ok()
    }
}

impl EntityValue {
    /// Convert a KV3 value into an entity property, if it has a shape an
    /// entity property can take.
    fn from_kv(value: &KvValue) -> Option<Self> {
        Some(match value {
            KvValue::Bool(b) => Self::Bool(*b),
            KvValue::Int(i) => Self::Int(*i),
            KvValue::UInt(u) => Self::UInt(*u),
            KvValue::Double(d) => Self::Double(*d),
            KvValue::String(s) => Self::String(s.clone()),
            KvValue::Array(items) | KvValue::TypedArray(items) => {
                // Vectors and colours are both short numeric arrays. Three
                // components is a vector; four is RGBA.
                let numbers: Vec<f64> = items.iter().filter_map(KvValue::as_f64).collect();
                if numbers.len() != items.len() {
                    return None;
                }
                #[allow(clippy::cast_possible_truncation)]
                match numbers[..] {
                    [x, y, z] => Self::Vector([x as f32, y as f32, z as f32]),
                    [r, g, b, a] => {
                        Self::Color([clamp_u8(r), clamp_u8(g), clamp_u8(b), clamp_u8(a)])
                    }
                    _ => return None,
                }
            }
            KvValue::Null | KvValue::Binary(_) | KvValue::Object(_) => return None,
        })
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_u8(v: f64) -> u8 {
    if v.is_nan() {
        0
    } else {
        v.clamp(0.0, 255.0) as u8
    }
}

/// Decode the legacy `m_keyValuesData` blob.
fn parse_legacy_blob(blob: &[u8]) -> Result<Entity> {
    let mut r = Reader::new(blob);

    let version = r.u32("entity blob version")?;
    if version != LEGACY_BLOB_VERSION {
        return Err(Source2Error::UnsupportedEntityBlobVersion { version });
    }
    let hashed_count = r.u32("hashed field count")?;
    let string_count = r.u32("string field count")?;

    // Bound the counts by what is left, so a corrupt header cannot make us
    // spin or reserve wildly.
    let total = u64::from(hashed_count) + u64::from(string_count);
    if total.saturating_mul(MIN_FIELD_BYTES as u64) > r.remaining() as u64 {
        return Err(Source2Error::InvalidSize {
            what: "entity field count",
            value: i64::try_from(total).unwrap_or(i64::MAX),
        });
    }

    let tokens = token_table();
    let mut values = Vec::with_capacity(usize::try_from(total).unwrap_or(0));

    for _ in 0..hashed_count {
        let token = r.u32("entity key token")?;
        let name = tokens
            .get(&token)
            .map_or_else(|| format!("{token:#010x}"), |s| (*s).to_string());
        values.push((name, read_legacy_value(&mut r)?));
    }

    for _ in 0..string_count {
        // The hash is stored redundantly next to the name it hashes; it is read
        // to stay in step with the stream, and the name is authoritative.
        let _token = r.u32("entity key token")?;
        let name = r.cstr("entity key name")?.to_ascii_lowercase();
        values.push((name, read_legacy_value(&mut r)?));
    }

    Ok(Entity { values })
}

/// Read one type-tagged value from a legacy blob.
fn read_legacy_value(r: &mut Reader<'_>) -> Result<EntityValue> {
    let raw = r.u32("entity field type")?;
    let ty = FieldType::from_u32(raw).ok_or(Source2Error::UnknownEntityFieldType { value: raw })?;

    Ok(match ty {
        FieldType::Boolean => EntityValue::Bool(r.take(1, "entity bool")?[0] != 0),
        FieldType::Float => EntityValue::Double(f64::from(r.f32("entity float")?)),
        FieldType::Float64 => EntityValue::Double(r.f64("entity double")?),
        FieldType::Integer => EntityValue::Int(i64::from(r.i32("entity int")?)),
        FieldType::UInt => EntityValue::UInt(u64::from(r.u32("entity uint")?)),
        FieldType::Integer64 => EntityValue::Int(r.i64("entity int64")?),
        FieldType::UInt64 => EntityValue::UInt(r.u64("entity uint64")?),
        FieldType::Color32 => {
            let b = r.array::<4>("entity color")?;
            EntityValue::Color(b)
        }
        FieldType::Vector | FieldType::QAngle => EntityValue::Vector([
            r.f32("entity vector x")?,
            r.f32("entity vector y")?,
            r.f32("entity vector z")?,
        ]),
        FieldType::CString => EntityValue::String(r.cstr("entity string")?),
    })
}

/// Token to key-name table, built by hashing [`KNOWN_KEYS`].
fn token_table() -> HashMap<u32, &'static str> {
    KNOWN_KEYS
        .iter()
        .map(|name| (string_token(name), *name))
        .collect()
}

/// Valve's entity key token: MurmurHash2 of the lower-cased name, seeded with
/// pi.
#[must_use]
pub fn string_token(name: &str) -> u32 {
    murmur2(name.to_ascii_lowercase().as_bytes(), MURMUR2_SEED)
}

/// The key name a legacy hashed field's token refers to, if it is one this
/// decoder knows. Tokens are not invertible in general.
#[must_use]
pub fn known_key_for_token(token: u32) -> Option<&'static str> {
    KNOWN_KEYS
        .iter()
        .copied()
        .find(|name| string_token(name) == token)
}

/// MurmurHash2, 32-bit, as Valve uses it for string tokens.
fn murmur2(data: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;

    #[allow(clippy::cast_possible_truncation)]
    let mut h: u32 = seed ^ (data.len() as u32);

    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M);
        h ^= k;
    }

    let tail = chunks.remainder();
    if !tail.is_empty() {
        let mut last: u32 = 0;
        for (i, &byte) in tail.iter().enumerate() {
            last |= u32::from(byte) << (8 * i);
        }
        h ^= last;
        h = h.wrapping_mul(M);
    }

    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a legacy blob the way Valve's writer would.
    fn legacy_blob(hashed: &[(&str, u32, Vec<u8>)], strings: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&LEGACY_BLOB_VERSION.to_le_bytes());
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(hashed.len() as u32).to_le_bytes());
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        for (name, ty, payload) in hashed {
            out.extend_from_slice(&string_token(name).to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(payload);
        }
        for (name, ty, payload) in strings {
            out.extend_from_slice(&string_token(name).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(payload);
        }
        out
    }

    fn cstring(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    }

    fn vector(v: [f32; 3]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    #[test]
    fn murmur2_behaves_like_a_hash() {
        // The empty input collapses to the seed and then avalanches to zero,
        // which pins the final mixing steps.
        assert_eq!(murmur2(b"", 0), 0);

        // Every tail length 1..=3 takes the remainder branch; all four lengths
        // around a block boundary must stay distinct and deterministic.
        let lengths: Vec<u32> = (0..8)
            .map(|n| murmur2(&vec![b'a'; n], MURMUR2_SEED))
            .collect();
        for (i, a) in lengths.iter().enumerate() {
            assert_eq!(
                *a,
                murmur2(&vec![b'a'; i], MURMUR2_SEED),
                "not deterministic"
            );
            for b in &lengths[i + 1..] {
                assert_ne!(a, b, "length {i} collided");
            }
        }

        // Tokens are case insensitive because the name is lowered first.
        assert_eq!(string_token("Origin"), string_token("origin"));
        assert_ne!(string_token("origin"), string_token("angles"));
    }

    #[test]
    fn known_tokens_round_trip() {
        for name in KNOWN_KEYS {
            assert_eq!(known_key_for_token(string_token(name)), Some(*name));
        }
        // A token nothing hashes to stays unknown.
        assert_eq!(known_key_for_token(0), None);
    }

    #[test]
    fn legacy_blob_reads_both_field_kinds() {
        let blob = legacy_blob(
            &[
                ("classname", 0x1e, cstring("info_player_terrorist")),
                ("origin", 0x03, vector([1.0, 2.0, 3.0])),
            ],
            &[
                ("angles", 0x27, vector([0.0, 90.0, 0.0])),
                ("priority", 0x05, 7i32.to_le_bytes().to_vec()),
            ],
        );

        let entity = parse_legacy_blob(&blob).expect("parse");
        assert_eq!(entity.classname(), Some("info_player_terrorist"));
        assert_eq!(entity.origin(), Some([1.0, 2.0, 3.0]));
        assert_eq!(entity.yaw_degrees(), Some(90.0));
        assert_eq!(entity.number("priority"), Some(7.0));
    }

    #[test]
    fn legacy_blob_names_unknown_tokens_rather_than_dropping_them() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&LEGACY_BLOB_VERSION.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        blob.extend_from_slice(&0x05u32.to_le_bytes());
        blob.extend_from_slice(&42i32.to_le_bytes());

        let entity = parse_legacy_blob(&blob).expect("parse");
        assert_eq!(entity.values.len(), 1);
        assert_eq!(entity.values[0].0, "0xdeadbeef");
        assert_eq!(entity.number("0xdeadbeef"), Some(42.0));
    }

    #[test]
    fn legacy_blob_rejects_bad_version_and_absurd_counts() {
        let mut bad = 2u32.to_le_bytes().to_vec();
        bad.extend_from_slice(&[0; 8]);
        assert!(matches!(
            parse_legacy_blob(&bad),
            Err(Source2Error::UnsupportedEntityBlobVersion { version: 2 })
        ));

        let mut huge = LEGACY_BLOB_VERSION.to_le_bytes().to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        huge.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            parse_legacy_blob(&huge),
            Err(Source2Error::InvalidSize { .. })
        ));
    }

    #[test]
    fn legacy_blob_stops_on_an_unknown_field_type() {
        let blob = legacy_blob(&[("classname", 0x77, vec![0; 8])], &[]);
        assert!(matches!(
            parse_legacy_blob(&blob),
            Err(Source2Error::UnknownEntityFieldType { value: 0x77 })
        ));
    }

    #[test]
    fn truncating_a_legacy_blob_never_panics() {
        let blob = legacy_blob(
            &[("classname", 0x1e, cstring("info_player_terrorist"))],
            &[("origin", 0x03, vector([1.0, 2.0, 3.0]))],
        );
        for len in 0..blob.len() {
            let _ = parse_legacy_blob(&blob[..len]);
        }
    }

    #[test]
    fn kv3_entries_decode_and_lower_case_their_keys() {
        let entry = KvValue::Object(vec![(
            "keyValues3Data".to_string(),
            KvValue::Object(vec![
                ("version".to_string(), KvValue::Int(1)),
                (
                    "values".to_string(),
                    KvValue::Object(vec![
                        (
                            "classname".to_string(),
                            KvValue::String("info_player_counterterrorist".into()),
                        ),
                        (
                            "origin".to_string(),
                            KvValue::TypedArray(vec![
                                KvValue::Double(382.0),
                                KvValue::Double(2102.0),
                                KvValue::Double(-109.0),
                            ]),
                        ),
                        (
                            "angles".to_string(),
                            KvValue::TypedArray(vec![
                                KvValue::Double(0.0),
                                KvValue::Double(101.0),
                                KvValue::Double(0.0),
                            ]),
                        ),
                        ("hammerUniqueId".to_string(), KvValue::String("2:1".into())),
                    ]),
                ),
                ("attributes".to_string(), KvValue::Object(vec![])),
            ]),
        )]);

        let entity = Entity::from_entry(&entry).expect("entity");
        assert_eq!(entity.classname(), Some("info_player_counterterrorist"));
        assert_eq!(entity.origin(), Some([382.0, 2102.0, -109.0]));
        assert_eq!(entity.yaw_degrees(), Some(101.0));
        // Mixed-case keys are addressable in lower case.
        assert_eq!(entity.string("hammeruniqueid"), Some("2:1"));
    }

    #[test]
    fn empty_and_malformed_entries_are_skipped_not_fatal() {
        assert!(Entity::from_entry(&KvValue::Null).is_none());
        assert!(Entity::from_entry(&KvValue::Object(vec![])).is_none());
        // An empty legacy blob alongside no KV3 data yields nothing.
        let empty = KvValue::Object(vec![(
            "m_keyValuesData".to_string(),
            KvValue::Binary(Vec::new()),
        )]);
        assert!(Entity::from_entry(&empty).is_none());
    }

    #[test]
    fn values_convert_from_every_kv_shape() {
        assert_eq!(
            EntityValue::from_kv(&KvValue::Bool(true)),
            Some(EntityValue::Bool(true))
        );
        assert_eq!(
            EntityValue::from_kv(&KvValue::TypedArray(vec![
                KvValue::UInt(255),
                KvValue::UInt(128),
                KvValue::UInt(0),
                KvValue::UInt(255),
            ])),
            Some(EntityValue::Color([255, 128, 0, 255]))
        );
        // Objects, blobs and odd-length arrays have no entity representation.
        assert!(EntityValue::from_kv(&KvValue::Object(vec![])).is_none());
        assert!(EntityValue::from_kv(&KvValue::Array(vec![KvValue::Int(1)])).is_none());
        assert!(EntityValue::from_kv(&KvValue::Array(vec![
            KvValue::Int(1),
            KvValue::String("x".into()),
            KvValue::Int(3),
        ]))
        .is_none());
    }

    #[test]
    fn numbers_parse_out_of_strings() {
        let entity = Entity {
            values: vec![
                ("spawnflags".into(), EntityValue::String("  12 ".into())),
                ("nope".into(), EntityValue::String("abc".into())),
            ],
        };
        assert_eq!(entity.number("spawnflags"), Some(12.0));
        assert_eq!(entity.number("nope"), None);
        assert_eq!(entity.number("absent"), None);
    }
}
