//! Payload encoding v1 (ABI.md §5): a strict subset of MessagePack.
//!
//! Hand-written on purpose: every SDK runtime (C#, Java, Swift) and every core
//! (Rust, Go, Zig) carries the same ~200 lines, so no language depends on a
//! third-party MessagePack library's quirks.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct WireError(pub String);

impl WireError {
    pub fn msg(m: impl Into<String>) -> Self {
        WireError(m.into())
    }
    pub fn missing(field: &str) -> Self {
        WireError(format!("missing required field `{field}`"))
    }
    pub fn unknown_case(ty: &str, tag: u32) -> Self {
        WireError(format!("unknown case tag {tag} for `{ty}` (peer uses a newer contract?)"))
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "payload decode error: {}", self.0)
    }
}

impl std::error::Error for WireError {}

#[derive(Default, Debug)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
    pub fn nil(&mut self) {
        self.buf.push(0xc0);
    }
    pub fn bool(&mut self, v: bool) {
        self.buf.push(if v { 0xc3 } else { 0xc2 });
    }
    pub fn uint(&mut self, v: u64) {
        if v < 0x80 {
            self.buf.push(v as u8);
        } else if v <= u8::MAX as u64 {
            self.buf.extend_from_slice(&[0xcc, v as u8]);
        } else if v <= u16::MAX as u64 {
            self.buf.push(0xcd);
            self.buf.extend_from_slice(&(v as u16).to_be_bytes());
        } else if v <= u32::MAX as u64 {
            self.buf.push(0xce);
            self.buf.extend_from_slice(&(v as u32).to_be_bytes());
        } else {
            self.buf.push(0xcf);
            self.buf.extend_from_slice(&v.to_be_bytes());
        }
    }
    pub fn int(&mut self, v: i64) {
        if v >= 0 {
            self.uint(v as u64);
        } else if v >= -32 {
            self.buf.push(v as i8 as u8);
        } else if v >= i8::MIN as i64 {
            self.buf.extend_from_slice(&[0xd0, v as i8 as u8]);
        } else if v >= i16::MIN as i64 {
            self.buf.push(0xd1);
            self.buf.extend_from_slice(&(v as i16).to_be_bytes());
        } else if v >= i32::MIN as i64 {
            self.buf.push(0xd2);
            self.buf.extend_from_slice(&(v as i32).to_be_bytes());
        } else {
            self.buf.push(0xd3);
            self.buf.extend_from_slice(&v.to_be_bytes());
        }
    }
    pub fn f32(&mut self, v: f32) {
        self.buf.push(0xca);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.push(0xcb);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn len_header(&mut self, len: usize, fix: Option<(u8, usize)>, m8: Option<u8>, m16: u8, m32: u8) {
        match fix {
            Some((base, max)) if len <= max => self.buf.push(base | len as u8),
            _ => {
                if let (Some(m8), true) = (m8, len <= u8::MAX as usize) {
                    self.buf.extend_from_slice(&[m8, len as u8]);
                } else if len <= u16::MAX as usize {
                    self.buf.push(m16);
                    self.buf.extend_from_slice(&(len as u16).to_be_bytes());
                } else {
                    self.buf.push(m32);
                    self.buf.extend_from_slice(&(len as u32).to_be_bytes());
                }
            }
        }
    }
    /// Strings are always UTF-8 (Rust `str` guarantees it).
    pub fn str(&mut self, s: &str) {
        self.len_header(s.len(), Some((0xa0, 31)), Some(0xd9), 0xda, 0xdb);
        self.buf.extend_from_slice(s.as_bytes());
    }
    pub fn bin(&mut self, b: &[u8]) {
        self.len_header(b.len(), None, Some(0xc4), 0xc5, 0xc6);
        self.buf.extend_from_slice(b);
    }
    pub fn array_header(&mut self, n: usize) {
        self.len_header(n, Some((0x90, 15)), None, 0xdc, 0xdd);
    }
    pub fn map_header(&mut self, n: usize) {
        self.len_header(n, Some((0x80, 15)), None, 0xde, 0xdf);
    }
}

const MAX_DEPTH: u32 = 64;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

type R<T> = Result<T, WireError>;

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    fn byte(&mut self) -> R<u8> {
        let b = *self.buf.get(self.pos).ok_or_else(|| WireError::msg("unexpected end of payload"))?;
        self.pos += 1;
        Ok(b)
    }
    fn peek(&self) -> R<u8> {
        self.buf.get(self.pos).copied().ok_or_else(|| WireError::msg("unexpected end of payload"))
    }
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if self.remaining() < n {
            return Err(WireError::msg("length exceeds payload"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn be(&mut self, n: usize) -> R<u64> {
        Ok(self.take(n)?.iter().fold(0u64, |a, b| (a << 8) | *b as u64))
    }

    pub fn is_nil(&self) -> bool {
        self.peek().ok() == Some(0xc0)
    }
    pub fn nil(&mut self) -> R<()> {
        match self.byte()? {
            0xc0 => Ok(()),
            m => Err(WireError(format!("expected nil, found marker {m:#04x}"))),
        }
    }
    pub fn bool(&mut self) -> R<bool> {
        match self.byte()? {
            0xc2 => Ok(false),
            0xc3 => Ok(true),
            m => Err(WireError(format!("expected bool, found marker {m:#04x}"))),
        }
    }
    /// Any MessagePack integer, widened.
    pub fn integer(&mut self) -> R<i128> {
        let m = self.byte()?;
        Ok(match m {
            0x00..=0x7f => m as i128,
            0xe0..=0xff => (m as i8) as i128,
            0xcc => self.be(1)? as i128,
            0xcd => self.be(2)? as i128,
            0xce => self.be(4)? as i128,
            0xcf => self.be(8)? as i128,
            0xd0 => (self.be(1)? as u8 as i8) as i128,
            0xd1 => (self.be(2)? as u16 as i16) as i128,
            0xd2 => (self.be(4)? as u32 as i32) as i128,
            0xd3 => (self.be(8)? as i64) as i128,
            _ => return Err(WireError(format!("expected integer, found marker {m:#04x}"))),
        })
    }
    fn ranged<T: TryFrom<i128>>(&mut self, what: &str) -> R<T> {
        let v = self.integer()?;
        T::try_from(v).map_err(|_| WireError(format!("integer {v} out of range for {what}")))
    }
    pub fn u8(&mut self) -> R<u8> { self.ranged("u8") }
    pub fn u16(&mut self) -> R<u16> { self.ranged("u16") }
    pub fn u32(&mut self) -> R<u32> { self.ranged("u32") }
    pub fn u64(&mut self) -> R<u64> { self.ranged("u64") }
    pub fn i8(&mut self) -> R<i8> { self.ranged("s8") }
    pub fn i16(&mut self) -> R<i16> { self.ranged("s16") }
    pub fn i32(&mut self) -> R<i32> { self.ranged("s32") }
    pub fn i64(&mut self) -> R<i64> { self.ranged("s64") }
    pub fn char(&mut self) -> R<char> {
        let v = self.u32()?;
        char::from_u32(v).ok_or_else(|| WireError(format!("{v:#x} is not a unicode scalar value")))
    }
    pub fn f64(&mut self) -> R<f64> {
        match self.byte()? {
            0xca => Ok(f32::from_bits(self.be(4)? as u32) as f64),
            0xcb => Ok(f64::from_bits(self.be(8)?)),
            m => Err(WireError(format!("expected float, found marker {m:#04x}"))),
        }
    }
    pub fn f32(&mut self) -> R<f32> {
        Ok(self.f64()? as f32)
    }
    fn str_len(&mut self) -> R<usize> {
        let m = self.byte()?;
        Ok(match m {
            0xa0..=0xbf => (m & 0x1f) as usize,
            0xd9 => self.be(1)? as usize,
            0xda => self.be(2)? as usize,
            0xdb => self.be(4)? as usize,
            _ => return Err(WireError(format!("expected string, found marker {m:#04x}"))),
        })
    }
    /// Invalid UTF-8 is rejected, never replaced.
    pub fn str(&mut self) -> R<String> {
        let n = self.str_len()?;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| WireError::msg("string is not valid UTF-8"))
    }
    pub fn bin(&mut self) -> R<Vec<u8>> {
        let m = self.byte()?;
        let n = match m {
            0xc4 => self.be(1)?,
            0xc5 => self.be(2)?,
            0xc6 => self.be(4)?,
            _ => return Err(WireError(format!("expected bin, found marker {m:#04x}"))),
        } as usize;
        Ok(self.take(n)?.to_vec())
    }
    pub fn array_header(&mut self) -> R<usize> {
        let m = self.byte()?;
        let n = match m {
            0x90..=0x9f => (m & 0x0f) as usize,
            0xdc => self.be(2)? as usize,
            0xdd => self.be(4)? as usize,
            _ => return Err(WireError(format!("expected array, found marker {m:#04x}"))),
        };
        // Every element needs at least one byte: rejects absurd lengths early.
        if n > self.remaining() {
            return Err(WireError::msg("array length exceeds payload"));
        }
        Ok(n)
    }
    pub fn map_header(&mut self) -> R<usize> {
        let m = self.byte()?;
        let n = match m {
            0x80..=0x8f => (m & 0x0f) as usize,
            0xde => self.be(2)? as usize,
            0xdf => self.be(4)? as usize,
            _ => return Err(WireError(format!("expected map, found marker {m:#04x}"))),
        };
        if n.saturating_mul(2) > self.remaining() {
            return Err(WireError::msg("map length exceeds payload"));
        }
        Ok(n)
    }
    pub fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<Option<T>> {
        if self.is_nil() {
            self.pos += 1;
            Ok(None)
        } else {
            f(self).map(Some)
        }
    }
    pub fn list<T>(&mut self, mut f: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let n = self.array_header()?;
        (0..n).map(|_| f(self)).collect()
    }
    pub fn tuple_header(&mut self, n: usize) -> R<()> {
        match self.array_header()? {
            k if k == n => Ok(()),
            k => Err(WireError(format!("expected tuple of {n}, found {k} elements"))),
        }
    }
    /// Request payloads are an array with exactly one element per parameter.
    pub fn params(&mut self, n: usize) -> R<()> {
        match self.array_header()? {
            k if k == n => Ok(()),
            k => Err(WireError(format!("expected {n} parameters, found {k}"))),
        }
    }
    pub fn finish(&self) -> R<()> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(WireError(format!("{n} trailing bytes after payload"))),
        }
    }
    /// Skips one value of any type (used for unknown record fields).
    pub fn skip(&mut self) -> R<()> {
        self.skip_depth(0)
    }
    fn skip_depth(&mut self, depth: u32) -> R<()> {
        if depth > MAX_DEPTH {
            return Err(WireError::msg("payload nested too deeply"));
        }
        let m = self.peek()?;
        match m {
            0x00..=0x7f | 0xe0..=0xff | 0xcc..=0xd3 => {
                self.integer()?;
            }
            0xc0 | 0xc2 | 0xc3 => self.pos += 1,
            0xca | 0xcb => {
                self.f64()?;
            }
            0xa0..=0xbf | 0xd9..=0xdb => {
                let n = self.str_len()?;
                self.take(n)?;
            }
            0xc4..=0xc6 => {
                self.bin()?;
            }
            0x90..=0x9f | 0xdc | 0xdd => {
                for _ in 0..self.array_header()? {
                    self.skip_depth(depth + 1)?;
                }
            }
            0x80..=0x8f | 0xde | 0xdf => {
                for _ in 0..self.map_header()? * 2 {
                    self.skip_depth(depth + 1)?;
                }
            }
            0xd4..=0xd8 | 0xc7..=0xc9 => {
                self.pos += 1;
                let n = match m {
                    0xd4 => 1,
                    0xd5 => 2,
                    0xd6 => 4,
                    0xd7 => 8,
                    0xd8 => 16,
                    0xc7 => self.be(1)? as usize,
                    0xc8 => self.be(2)? as usize,
                    _ => self.be(4)? as usize,
                };
                self.take(1 + n)?;
            }
            0xc1 => return Err(WireError::msg("reserved marker 0xc1")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_scalars() {
        let mut w = Writer::new();
        for v in [0u64, 1, 127, 128, 255, 256, 65535, 65536, u32::MAX as u64, u64::MAX] {
            w.uint(v);
        }
        for v in [-1i64, -32, -33, -128, -129, -32768, -32769, i32::MIN as i64, i64::MIN] {
            w.int(v);
        }
        w.str("grüße 👋");
        w.bin(&[1, 2, 3]);
        w.f64(1.5);
        let buf = w.into_vec();
        let mut r = Reader::new(&buf);
        for v in [0u64, 1, 127, 128, 255, 256, 65535, 65536, u32::MAX as u64, u64::MAX] {
            assert_eq!(r.u64().unwrap(), v);
        }
        for v in [-1i64, -32, -33, -128, -129, -32768, -32769, i32::MIN as i64, i64::MIN] {
            assert_eq!(r.i64().unwrap(), v);
        }
        assert_eq!(r.str().unwrap(), "grüße 👋");
        assert_eq!(r.bin().unwrap(), vec![1, 2, 3]);
        assert_eq!(r.f64().unwrap(), 1.5);
        r.finish().unwrap();
    }

    #[test]
    fn rejects_garbage() {
        assert!(Reader::new(&[0xdd, 0xff, 0xff, 0xff, 0xff]).array_header().is_err());
        assert!(Reader::new(&[0xa2, 0xff, 0xfe]).str().is_err());
        assert!(Reader::new(&[0xcc]).u8().is_err());
        assert!(Reader::new(&[0xcd, 0x01, 0x00]).u8().is_err());
        let deep = vec![0x91u8; 200];
        assert!(Reader::new(&deep).skip().is_err());
    }
}
