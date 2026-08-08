//! The `RERL` block: the resource's list of external references.
//!
//! Layout, following ValveResourceFormat's `ResourceExtRefList.cs`:
//!
//! ```text
//! u32 offset   relative to its own position; start of the entry array
//! u32 count
//! ... count entries of:
//!     u64 id
//!     i64 name_offset   relative to its own position
//! ... NUL-terminated names, pointed at by the entries
//! ```

use crate::error::{Result, Source2Error};
use crate::reader::Reader;

/// Refuse absurd reference counts; real resources have at most a few thousand.
const MAX_REFERENCES: u64 = 1_000_000;

/// One external resource this file depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReference {
    /// Valve's 64-bit hash of the referenced resource name.
    pub id: u64,
    /// The referenced resource path, e.g. `materials/dev/reflectivity_30.vmat`.
    pub name: String,
}

/// Parse a `RERL` block.
///
/// `data` is the whole resource file and `block_offset` the block's absolute
/// offset, because name offsets can point outside the block's declared size.
///
/// # Errors
///
/// Returns [`Source2Error`] on malformed input. Never panics.
pub fn parse(data: &[u8], block_offset: u64) -> Result<Vec<ExternalReference>> {
    let mut r = Reader::new(data);
    r.seek(block_offset, "rerl block")?;

    let offset = r.u32("rerl offset")?;
    let count = r.u32("rerl count")?;

    if count == 0 {
        return Ok(Vec::new());
    }
    if u64::from(count) > MAX_REFERENCES {
        return Err(Source2Error::InvalidSize {
            what: "rerl count",
            value: i64::from(count),
        });
    }

    // The offset is relative to its own position, i.e. 8 bytes back.
    r.skip_signed(i64::from(offset) - 8, "rerl entries")?;

    // Each entry is 16 bytes; bail early if the file cannot hold them.
    let needed = u64::from(count).saturating_mul(16);
    if needed > data.len() as u64 {
        return Err(Source2Error::InvalidSize {
            what: "rerl count",
            value: i64::from(count),
        });
    }

    let mut refs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = r.u64("rerl id")?;
        let position = r.pos() as u64;
        let name_offset = r.i64("rerl name offset")?;

        // Jump to the name, read it, then return to just after the offset field.
        r.seek(position, "rerl name")?;
        r.skip_signed(name_offset, "rerl name")?;
        let name = r.cstr("rerl name")?;
        r.seek(position + 8, "rerl entries")?;

        refs.push(ExternalReference { id, name });
    }

    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a RERL block payload for the given references.
    fn build_rerl(names: &[(u64, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&8u32.to_le_bytes()); // entries start right after
        out.extend_from_slice(&(names.len() as u32).to_le_bytes());

        const ENTRY: usize = 16;
        let strings_start = 8 + names.len() * ENTRY;
        let mut string_offset = 0usize;
        let mut strings = Vec::new();

        for (i, (id, name)) in names.iter().enumerate() {
            out.extend_from_slice(&id.to_le_bytes());
            // Position of the name-offset field itself.
            let field_pos = 8 + i * ENTRY + 8;
            let target = strings_start + string_offset;
            out.extend_from_slice(&((target as i64) - (field_pos as i64)).to_le_bytes());
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
            string_offset += name.len() + 1;
        }

        out.extend_from_slice(&strings);
        out
    }

    #[test]
    fn parses_references() {
        let block = build_rerl(&[
            (0x1234_5678_9ABC_DEF0, "materials/dev/reflectivity_30.vmat"),
            (
                0x0FED_CBA9_8765_4321,
                "models/props/de_dust/hr_dust/palm.vmdl",
            ),
        ]);
        let refs = parse(&block, 0).expect("parse");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].id, 0x1234_5678_9ABC_DEF0);
        assert_eq!(refs[0].name, "materials/dev/reflectivity_30.vmat");
        assert_eq!(refs[1].name, "models/props/de_dust/hr_dust/palm.vmdl");
    }

    #[test]
    fn parses_at_non_zero_block_offset() {
        let block = build_rerl(&[(7, "a/b.vmat")]);
        let mut file = vec![0xAAu8; 64];
        file.extend_from_slice(&block);
        let refs = parse(&file, 64).expect("parse");
        assert_eq!(
            refs,
            vec![ExternalReference {
                id: 7,
                name: "a/b.vmat".into()
            }]
        );
    }

    #[test]
    fn empty_block_yields_no_references() {
        let block = [0u8; 8];
        assert!(parse(&block, 0).unwrap().is_empty());
    }

    #[test]
    fn truncation_never_panics() {
        let block = build_rerl(&[(1, "one.vmat"), (2, "two.vmdl")]);
        for cut in 0..block.len() {
            let _ = parse(&block[..cut], 0);
        }
    }

    #[test]
    fn bit_flips_never_panic() {
        let block = build_rerl(&[(1, "one.vmat"), (2, "two.vmdl")]);
        for i in 0..block.len() {
            for mask in [0x01u8, 0x80, 0xFF] {
                let mut corrupt = block.clone();
                corrupt[i] ^= mask;
                let _ = parse(&corrupt, 0);
            }
        }
    }
}
