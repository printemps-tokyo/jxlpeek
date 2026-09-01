//! LSB-first bit reader and the JPEG XL field coders.
//!
//! JPEG XL headers are a bit stream read least-significant-bit first within
//! each byte (ISO/IEC 18181-1, "Bit and byte ordering"). Field values are
//! coded with a handful of small schemes: `Bool`, `U32` (a 2-bit selector
//! choosing one of four sub-distributions), `U64`, `F16`, `Enum` (a fixed
//! `U32` configuration) and length-prefixed byte strings. All of them live
//! here; the header structs in `codestream.rs` are just sequences of these.

use std::fmt;

/// A header could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// The stream ended before the field at this bit offset.
    Truncated { at_bit: u64 },
    /// A field had a value the format does not allow.
    Invalid(String),
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderError::Truncated { at_bit } => {
                write!(f, "codestream truncated at bit {at_bit}")
            }
            HeaderError::Invalid(msg) => write!(f, "invalid header: {msg}"),
        }
    }
}

impl std::error::Error for HeaderError {}

pub type Result<T> = std::result::Result<T, HeaderError>;

/// One of the four sub-distributions of a `U32` field.
#[derive(Debug, Clone, Copy)]
pub enum U32 {
    /// The constant value, no further bits.
    Val(u32),
    /// `n` raw bits.
    Bits(u32),
    /// `n` raw bits plus an offset.
    BitsOffset(u32, u32),
}

/// The `U32` configuration every enum field uses
/// (libjxl `Visitor::Enum`: 00 -> 0, 01 -> 1, 10xxxx -> 2..17, 11yyyyyy -> 18..81).
pub const ENUM: [U32; 4] = [
    U32::Val(0),
    U32::Val(1),
    U32::BitsOffset(4, 2),
    U32::BitsOffset(6, 18),
];

/// The length distribution of a byte string field.
const STRING_LEN: [U32; 4] = [
    U32::Val(0),
    U32::Bits(4),
    U32::BitsOffset(5, 16),
    U32::BitsOffset(10, 48),
];

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: u64,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// Bits consumed so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Read `n` (0..=64) bits, least significant first.
    pub fn read(&mut self, n: u32) -> Result<u64> {
        debug_assert!(n <= 64);
        let mut value = 0u64;
        for i in 0..n {
            let byte = (self.pos / 8) as usize;
            if byte >= self.data.len() {
                return Err(HeaderError::Truncated { at_bit: self.pos });
            }
            let bit = (self.data[byte] >> (self.pos % 8)) & 1;
            value |= u64::from(bit) << i;
            self.pos += 1;
        }
        Ok(value)
    }

    pub fn skip(&mut self, n: u64) -> Result<()> {
        let end = self.pos.checked_add(n).ok_or(HeaderError::Invalid(
            "extension length overflows".to_string(),
        ))?;
        if end > self.data.len() as u64 * 8 {
            return Err(HeaderError::Truncated { at_bit: self.pos });
        }
        self.pos = end;
        Ok(())
    }

    /// Skip to the next byte boundary (no-op when already aligned).
    pub fn jump_to_byte_boundary(&mut self) -> Result<()> {
        let rem = self.pos % 8;
        if rem != 0 {
            self.skip(8 - rem)?;
        }
        Ok(())
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.read(1)? != 0)
    }

    pub fn bits(&mut self, n: u32) -> Result<u32> {
        Ok(self.read(n)? as u32)
    }

    /// A `U32` field: 2-bit selector, then the chosen sub-distribution.
    pub fn u32(&mut self, dists: [U32; 4]) -> Result<u32> {
        let selector = self.read(2)? as usize;
        Ok(match dists[selector] {
            U32::Val(v) => v,
            U32::Bits(n) => self.bits(n)?,
            U32::BitsOffset(n, off) => self.bits(n)? + off,
        })
    }

    /// A `U64` field (libjxl `U64Coder`).
    pub fn u64(&mut self) -> Result<u64> {
        match self.read(2)? {
            0 => Ok(0),
            1 => Ok(1 + self.read(4)?),
            2 => Ok(17 + self.read(8)?),
            _ => {
                let mut value = self.read(12)?;
                let mut shift = 12;
                while self.read(1)? != 0 {
                    if shift == 60 {
                        value |= self.read(4)? << 60;
                        break;
                    }
                    value |= self.read(8)? << shift;
                    shift += 8;
                }
                Ok(value)
            }
        }
    }

    /// A 16-bit IEEE half stored in 16 bits, widened to f32.
    pub fn f16(&mut self) -> Result<f32> {
        let value = half_to_f32(self.read(16)? as u16);
        if !value.is_finite() {
            return Err(HeaderError::Invalid("non-finite float field".to_string()));
        }
        Ok(value)
    }

    pub fn enum_value(&mut self) -> Result<u32> {
        self.u32(ENUM)
    }

    /// A length-prefixed byte string (extra channel names).
    pub fn string(&mut self) -> Result<String> {
        let len = self.u32(STRING_LEN)?;
        let mut bytes = Vec::with_capacity(len as usize);
        for _ in 0..len {
            bytes.push(self.read(8)? as u8);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The trailing `Extensions` field of a bundle: a `U64` bit mask, then one
    /// `U64` payload length per set bit, then the payloads, which an old
    /// decoder skips (libjxl `Bundle::BeginExtensions`/`EndExtensions`).
    /// Returns the mask.
    pub fn extensions(&mut self) -> Result<u64> {
        let mask = self.u64()?;
        if mask != 0 {
            let mut total: u64 = 0;
            for i in 0..64 {
                if (mask >> i) & 1 == 1 {
                    total = total.checked_add(self.u64()?).ok_or(HeaderError::Invalid(
                        "extension length overflows".to_string(),
                    ))?;
                }
            }
            self.skip(total)?;
        }
        Ok(mask)
    }
}

/// Zigzag-style signed unpacking used by CustomXY (libjxl `UnpackSigned`).
pub fn unpack_signed(value: u32) -> i32 {
    if value & 1 == 0 {
        (value >> 1) as i32
    } else {
        -((value >> 1) as i32) - 1
    }
}

/// IEEE 754 binary16 to f32, including subnormals.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let mant = u32::from(h & 0x3ff);
    let bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            // Subnormal: normalize by shifting the mantissa up until the
            // implicit leading one appears at bit 10.
            let mut e: i32 = 113;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        sign | 0x7f80_0000 | (mant << 13)
    } else {
        sign | ((exp + 112) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lsb_first() {
        // 0b1010_0110: bits in read order are 0,1,1,0,0,1,0,1.
        let data = [0b1010_0110u8, 0xff];
        let mut br = BitReader::new(&data);
        assert_eq!(br.read(1).unwrap(), 0);
        assert_eq!(br.read(2).unwrap(), 0b11);
        assert_eq!(br.read(5).unwrap(), 0b10100);
        assert_eq!(br.read(8).unwrap(), 0xff);
        assert!(matches!(
            br.read(1),
            Err(HeaderError::Truncated { at_bit: 16 })
        ));
    }

    #[test]
    fn u32_selector_picks_distribution() {
        // selector 2 (bits "01" LSB first = value 2), then 4 bits = 5 -> 2 + 5.
        // Bit layout: sel bits: b0=0, b1=1; then value bits 1,0,1,0.
        let data = [0b0001_0110u8];
        let mut br = BitReader::new(&data);
        assert_eq!(br.u32(ENUM).unwrap(), 7);
    }

    #[test]
    fn u64_small_values() {
        // selector 1 then 4 bits of 3 -> 1 + 3 = 4.
        let data = [0b0000_1101u8];
        let mut br = BitReader::new(&data);
        assert_eq!(br.u64().unwrap(), 4);
        // selector 3, 12 bits = 0xabc, then continuation 0.
        let mut w = BitWriter::default();
        w.write(2, 3);
        w.write(12, 0xabc);
        w.write(1, 0);
        let bytes = w.finish();
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.u64().unwrap(), 0xabc);
        // selector 3 with one 8-bit continuation.
        let mut w = BitWriter::default();
        w.write(2, 3);
        w.write(12, 0x123);
        w.write(1, 1);
        w.write(8, 0x45);
        w.write(1, 0);
        let bytes = w.finish();
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.u64().unwrap(), 0x123 | (0x45 << 12));
    }

    #[test]
    fn half_floats() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x5bf8), 255.0);
        assert_eq!(half_to_f32(0x63d0), 1000.0);
        // Smallest subnormal is 2^-24.
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert!(half_to_f32(0x7c00).is_infinite());
    }

    #[test]
    fn signed_unpacking() {
        assert_eq!(unpack_signed(0), 0);
        assert_eq!(unpack_signed(1), -1);
        assert_eq!(unpack_signed(2), 1);
        assert_eq!(unpack_signed(3), -2);
    }

    #[test]
    fn extensions_are_skipped() {
        let mut w = BitWriter::default();
        // mask = 1 (selector 1, 4 bits 0 -> 1 + 0)
        w.write(2, 1);
        w.write(4, 0);
        // length = 5 bits (selector 1, 4 bits 4 -> 5)
        w.write(2, 1);
        w.write(4, 4);
        w.write(5, 0b10101);
        w.write(3, 0b111);
        let bytes = w.finish();
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.extensions().unwrap(), 1);
        assert_eq!(br.read(3).unwrap(), 0b111);
    }

    /// Minimal LSB-first writer for building test streams.
    #[derive(Default)]
    pub struct BitWriter {
        bytes: Vec<u8>,
        nbits: u64,
    }

    impl BitWriter {
        pub fn write(&mut self, n: u32, value: u64) {
            for i in 0..n {
                let bit = ((value >> i) & 1) as u8;
                if self.nbits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let last = self.bytes.len() - 1;
                self.bytes[last] |= bit << (self.nbits % 8);
                self.nbits += 1;
            }
        }
        pub fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }
}
