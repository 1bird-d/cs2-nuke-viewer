//! Bounds-checked little-endian reader shared by the resource and KV3 parsers.

use crate::error::{Result, Source2Error};

/// A cursor over a byte slice. Every read is bounds-checked and returns an
/// error rather than panicking.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Move the cursor to an absolute position.
    pub(crate) fn seek(&mut self, pos: u64, what: &'static str) -> Result<()> {
        let pos = usize::try_from(pos).map_err(|_| Source2Error::UnexpectedEof {
            what,
            needed: 0,
            offset: usize::MAX,
            available: self.data.len(),
        })?;
        if pos > self.data.len() {
            return Err(Source2Error::UnexpectedEof {
                what,
                needed: 0,
                offset: pos,
                available: self.data.len(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    /// Advance the cursor by a signed delta.
    pub(crate) fn skip_signed(&mut self, delta: i64, what: &'static str) -> Result<()> {
        let target = i64::try_from(self.pos)
            .ok()
            .and_then(|p| p.checked_add(delta))
            .ok_or(Source2Error::UnexpectedEof {
                what,
                needed: 0,
                offset: self.pos,
                available: self.remaining(),
            })?;
        if target < 0 {
            return Err(Source2Error::UnexpectedEof {
                what,
                needed: 0,
                offset: self.pos,
                available: self.remaining(),
            });
        }
        self.seek(target as u64, what)
    }

    pub(crate) fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
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

    pub(crate) fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        let slice = self.take(N, what)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array::<2>(what)?))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array::<4>(what)?))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array::<4>(what)?))
    }

    pub(crate) fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array::<8>(what)?))
    }

    pub(crate) fn i64(&mut self, what: &'static str) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array::<8>(what)?))
    }

    pub(crate) fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array::<4>(what)?))
    }

    pub(crate) fn f64(&mut self, what: &'static str) -> Result<f64> {
        Ok(f64::from_le_bytes(self.array::<8>(what)?))
    }

    /// Read a NUL-terminated UTF-8 string starting at the cursor.
    pub(crate) fn cstr(&mut self, what: &'static str) -> Result<String> {
        let rest = self
            .data
            .get(self.pos..)
            .ok_or(Source2Error::UnexpectedEof {
                what,
                needed: 1,
                offset: self.pos,
                available: 0,
            })?;
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Source2Error::UnexpectedEof {
                what,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_bounds_checked() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert!(r.u32("x").is_err());
        assert_eq!(r.u16("x").unwrap(), 0x0201);
        assert_eq!(r.remaining(), 1);
        assert!(r.u16("x").is_err());
    }

    #[test]
    fn seek_rejects_out_of_range() {
        let mut r = Reader::new(&[0u8; 4]);
        assert!(r.seek(4, "x").is_ok());
        assert!(r.seek(5, "x").is_err());
        assert!(r.skip_signed(-10, "x").is_err());
    }

    #[test]
    fn cstr_requires_terminator() {
        let mut r = Reader::new(b"ok\0rest");
        assert_eq!(r.cstr("x").unwrap(), "ok");
        let mut r2 = Reader::new(b"nope");
        assert!(r2.cstr("x").is_err());
    }
}
