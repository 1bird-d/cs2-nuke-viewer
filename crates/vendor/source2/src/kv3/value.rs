//! The decoded KeyValues3 value tree.

use std::fmt;

/// A hint Valve attaches to a value describing how it should be interpreted.
///
/// Flags never change the binary encoding of the value itself, they only
/// annotate it (for example marking a string as a resource path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KvFlag {
    /// No annotation.
    #[default]
    None,
    /// The value is a resource reference.
    Resource,
    /// The value is a resource name.
    ResourceName,
    /// The value is a Panorama identifier.
    Panorama,
    /// The value is a sound event name.
    SoundEvent,
    /// The value names a subclass.
    SubClass,
    /// The value is an entity name.
    EntityName,
}

impl KvFlag {
    /// Map the flag byte used by KV3 version 3 and later.
    pub(crate) fn from_modern_byte(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::None,
            1 => Self::Resource,
            2 => Self::ResourceName,
            3 => Self::Panorama,
            4 => Self::SoundEvent,
            5 => Self::SubClass,
            6 => Self::EntityName,
            _ => return None,
        })
    }

    /// Map the bitmask-style flag byte used by KV3 versions 0 through 2.
    pub(crate) fn from_legacy_byte(b: u8) -> Option<Self> {
        // Bit 2 (0x04) means "multiline string" and carries no semantics here.
        let b = b & !0x04;
        Some(match b {
            0 => Self::None,
            1 => Self::Resource,
            2 => Self::ResourceName,
            8 => Self::Panorama,
            16 => Self::SoundEvent,
            32 => Self::SubClass,
            _ => return None,
        })
    }
}

/// A decoded KeyValues3 value.
///
/// `Array` and `TypedArray` hold the same kind of contents; they are kept
/// distinct because the binary format encodes them differently and round-tripping
/// (a later milestone) needs to know which was used.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum KvValue {
    /// The `null` literal.
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer.
    UInt(u64),
    /// A double (KV3 has no separate float type in its value model; `float`
    /// nodes are widened on decode).
    Double(f64),
    /// A UTF-8 string.
    String(String),
    /// An opaque binary blob.
    Binary(Vec<u8>),
    /// A heterogeneous array; each element carries its own type byte.
    Array(Vec<KvValue>),
    /// An array whose elements all share one type byte.
    TypedArray(Vec<KvValue>),
    /// An ordered key/value collection. Keys can repeat, so this is a list
    /// rather than a map.
    Object(Vec<(String, KvValue)>),
}

impl KvValue {
    /// A short human-readable name for the value's kind.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::UInt(_) => "uint",
            Self::Double(_) => "double",
            Self::String(_) => "string",
            Self::Binary(_) => "binary",
            Self::Array(_) => "array",
            Self::TypedArray(_) => "typed_array",
            Self::Object(_) => "object",
        }
    }

    /// Look up a key on an object. Returns the first match.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&KvValue> {
        match self {
            Self::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a slash-separated path of object keys, e.g. `"m_world/m_nodes"`.
    #[must_use]
    pub fn path(&self, path: &str) -> Option<&KvValue> {
        let mut current = self;
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            current = current.get(segment)?;
        }
        Some(current)
    }

    /// Index into any array-like value.
    #[must_use]
    pub fn index(&self, i: usize) -> Option<&KvValue> {
        self.as_array()?.get(i)
    }

    /// The elements of an `Array` or `TypedArray`.
    #[must_use]
    pub fn as_array(&self) -> Option<&[KvValue]> {
        match self {
            Self::Array(v) | Self::TypedArray(v) => Some(v),
            _ => None,
        }
    }

    /// The fields of an `Object`.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, KvValue)]> {
        match self {
            Self::Object(v) => Some(v),
            _ => None,
        }
    }

    /// The value as a string slice.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a bool.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as a signed integer, widening unsigned values when they fit.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::UInt(u) => i64::try_from(*u).ok(),
            Self::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    /// The value as an unsigned integer.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt(u) => Some(*u),
            Self::Int(i) => u64::try_from(*i).ok(),
            Self::Bool(b) => Some(u64::from(*b)),
            _ => None,
        }
    }

    /// The value as a float, accepting integer values too.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Double(d) => Some(*d),
            #[allow(clippy::cast_precision_loss)]
            Self::Int(i) => Some(*i as f64),
            #[allow(clippy::cast_precision_loss)]
            Self::UInt(u) => Some(*u as f64),
            _ => None,
        }
    }

    /// The value as a binary blob.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Binary(b) => Some(b),
            _ => None,
        }
    }

    /// Total number of values in the tree, including this one.
    #[must_use]
    pub fn node_count(&self) -> usize {
        match self {
            Self::Array(v) | Self::TypedArray(v) => {
                1 + v.iter().map(Self::node_count).sum::<usize>()
            }
            Self::Object(fields) => 1 + fields.iter().map(|(_, v)| v.node_count()).sum::<usize>(),
            _ => 1,
        }
    }
}

/// A 16-byte KV3 GUID identifying an encoding or a format.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kv3Id(pub [u8; 16]);

impl Kv3Id {
    /// `KV3_ENCODING_BINARY_UNCOMPRESSED`.
    pub const BINARY_UNCOMPRESSED: Self = Self([
        0x00, 0x05, 0x86, 0x1B, 0xD8, 0xF7, 0xC1, 0x40, 0xAD, 0x82, 0x75, 0xA4, 0x82, 0x67, 0xE7,
        0x14,
    ]);
    /// `KV3_ENCODING_BINARY_BLOCK_COMPRESSED`.
    pub const BINARY_BLOCK_COMPRESSED: Self = Self([
        0x46, 0x1A, 0x79, 0x95, 0xBC, 0x95, 0x6C, 0x4F, 0xA7, 0x0B, 0x05, 0xBC, 0xA1, 0xB7, 0xDF,
        0xD2,
    ]);
    /// `KV3_ENCODING_BINARY_BLOCK_LZ4`.
    pub const BINARY_BLOCK_LZ4: Self = Self([
        0x8A, 0x34, 0x47, 0x68, 0xA1, 0x63, 0x5C, 0x4F, 0xA1, 0x97, 0x53, 0x80, 0x6F, 0xD9, 0xB1,
        0x19,
    ]);
}

impl fmt::Debug for Kv3Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Kv3Id({self})")
    }
}

impl fmt::Display for Kv3Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Rendered in the mixed-endian layout Microsoft GUIDs use, matching how
        // Valve's tooling prints these.
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

/// A decoded binary KV3 payload.
#[derive(Debug, Clone)]
pub struct Kv3Document {
    /// KV3 binary version: 0 for the legacy `VKV3` container, 1-5 otherwise.
    pub version: u8,
    /// Encoding GUID. Only present for version 0, which stores the compression
    /// scheme in the header rather than in a method field.
    pub encoding: Option<Kv3Id>,
    /// Format GUID, identifying the schema of the contents.
    pub format: Kv3Id,
    /// Compression method: 0 = none, 1 = LZ4, 2 = zstd. Always 0 for version 0.
    pub compression_method: u32,
    /// The root value, normally an [`KvValue::Object`].
    pub root: KvValue,
}

impl Kv3Document {
    /// Shorthand for `self.root.get(key)`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&KvValue> {
        self.root.get(key)
    }

    /// Shorthand for `self.root.path(path)`.
    #[must_use]
    pub fn path(&self, path: &str) -> Option<&KvValue> {
        self.root.path(path)
    }

    /// Human-readable name of the compression method.
    #[must_use]
    pub fn compression_name(&self) -> &'static str {
        match self.compression_method {
            0 => "none",
            1 => "lz4",
            2 => "zstd",
            _ => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_walk_the_tree() {
        let v = KvValue::Object(vec![
            (
                "m_world".to_string(),
                KvValue::Object(vec![(
                    "m_nodes".to_string(),
                    KvValue::Array(vec![KvValue::Int(7), KvValue::String("x".into())]),
                )]),
            ),
            ("count".to_string(), KvValue::UInt(3)),
        ]);

        assert_eq!(
            v.path("m_world/m_nodes").unwrap().as_array().unwrap().len(),
            2
        );
        assert_eq!(
            v.path("m_world/m_nodes")
                .unwrap()
                .index(0)
                .unwrap()
                .as_i64(),
            Some(7)
        );
        assert_eq!(
            v.path("m_world/m_nodes")
                .unwrap()
                .index(1)
                .unwrap()
                .as_str(),
            Some("x")
        );
        assert_eq!(v.get("count").unwrap().as_u64(), Some(3));
        assert!(v.path("m_world/nope").is_none());
        assert!(v.path("").is_some());
        assert_eq!(v.node_count(), 6);
    }

    #[test]
    fn flag_mappings_match_valve() {
        assert_eq!(KvFlag::from_modern_byte(6), Some(KvFlag::EntityName));
        assert_eq!(KvFlag::from_modern_byte(7), None);
        assert_eq!(KvFlag::from_legacy_byte(32), Some(KvFlag::SubClass));
        // The multiline-string bit is stripped before mapping.
        assert_eq!(KvFlag::from_legacy_byte(0x04), Some(KvFlag::None));
        assert_eq!(KvFlag::from_legacy_byte(64), None);
    }

    #[test]
    fn guid_renders_in_mixed_endian() {
        assert_eq!(
            Kv3Id::BINARY_BLOCK_LZ4.to_string(),
            "6847348a-63a1-4f5c-a197-53806fd9b119"
        );
    }
}
