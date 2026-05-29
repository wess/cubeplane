//! Reading primitives off the Minecraft wire format.
//!
//! Everything is built on top of [`bytes::Buf`]. The [`ProtoRead`] extension
//! trait adds the Minecraft-specific encodings (VarInt, VarLong, length-prefixed
//! strings, positions, …) on top of the big-endian fixed-width integers that
//! `Buf` already provides.

use bytes::Buf;
use uuid::Uuid;

use crate::error::{ProtocolError, Result};

/// Maximum number of UTF-8 bytes we will read for a single string. Mirrors the
/// vanilla limit of 32767 characters, each up to 4 bytes, plus a little slack.
pub const MAX_STRING_BYTES: usize = 32767 * 4;

/// Extension trait adding Minecraft protocol reads to any [`bytes::Buf`].
pub trait ProtoRead: Buf {
    /// Ensure at least `n` bytes remain, returning [`ProtocolError::Eof`] otherwise.
    fn ensure(&self, n: usize) -> Result<()> {
        if self.remaining() < n {
            Err(ProtocolError::Eof {
                needed: n,
                available: self.remaining(),
            })
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> Result<u8> {
        self.ensure(1)?;
        Ok(self.get_u8())
    }

    fn read_i8(&mut self) -> Result<i8> {
        self.ensure(1)?;
        Ok(self.get_i8())
    }

    fn read_bool(&mut self) -> Result<bool> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(ProtocolError::InvalidBool(other)),
        }
    }

    fn read_u16(&mut self) -> Result<u16> {
        self.ensure(2)?;
        Ok(self.get_u16())
    }

    fn read_i16(&mut self) -> Result<i16> {
        self.ensure(2)?;
        Ok(self.get_i16())
    }

    fn read_i32(&mut self) -> Result<i32> {
        self.ensure(4)?;
        Ok(self.get_i32())
    }

    fn read_i64(&mut self) -> Result<i64> {
        self.ensure(8)?;
        Ok(self.get_i64())
    }

    fn read_u64(&mut self) -> Result<u64> {
        self.ensure(8)?;
        Ok(self.get_u64())
    }

    fn read_f32(&mut self) -> Result<f32> {
        self.ensure(4)?;
        Ok(self.get_f32())
    }

    fn read_f64(&mut self) -> Result<f64> {
        self.ensure(8)?;
        Ok(self.get_f64())
    }

    /// Read a variable-length signed 32-bit integer (LEB128-ish, 7 bits/byte).
    fn read_varint(&mut self) -> Result<i32> {
        let mut value: u32 = 0;
        let mut position = 0u32;
        loop {
            let byte = self.read_u8()?;
            value |= ((byte & 0x7F) as u32) << position;
            if byte & 0x80 == 0 {
                break;
            }
            position += 7;
            if position >= 32 {
                return Err(ProtocolError::VarIntTooLarge);
            }
        }
        Ok(value as i32)
    }

    /// Read a variable-length signed 64-bit integer.
    fn read_varlong(&mut self) -> Result<i64> {
        let mut value: u64 = 0;
        let mut position = 0u32;
        loop {
            let byte = self.read_u8()?;
            value |= ((byte & 0x7F) as u64) << position;
            if byte & 0x80 == 0 {
                break;
            }
            position += 7;
            if position >= 64 {
                return Err(ProtocolError::VarLongTooLarge);
            }
        }
        Ok(value as i64)
    }

    /// Read `n` raw bytes into a `Vec<u8>`.
    fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        self.ensure(n)?;
        let mut out = vec![0u8; n];
        self.copy_to_slice(&mut out);
        Ok(out)
    }

    /// Read a VarInt-length-prefixed UTF-8 string.
    fn read_string(&mut self) -> Result<String> {
        let len = self.read_varint()? as usize;
        if len > MAX_STRING_BYTES {
            return Err(ProtocolError::StringTooLong {
                length: len,
                max: MAX_STRING_BYTES,
            });
        }
        let bytes = self.read_bytes(len)?;
        Ok(String::from_utf8(bytes)?)
    }

    /// Read a 128-bit UUID (big-endian).
    fn read_uuid(&mut self) -> Result<Uuid> {
        self.ensure(16)?;
        let mut bytes = [0u8; 16];
        self.copy_to_slice(&mut bytes);
        Ok(Uuid::from_bytes(bytes))
    }

    /// Read a packed block position (26/12/26 bits: x, z, y on 1.20).
    fn read_position(&mut self) -> Result<(i32, i32, i32)> {
        let val = self.read_i64()?;
        let mut x = (val >> 38) as i32;
        let mut y = (val << 52 >> 52) as i32;
        let mut z = (val << 26 >> 38) as i32;
        // Sign-extend the 26/12/26 fields.
        if x >= 1 << 25 {
            x -= 1 << 26;
        }
        if y >= 1 << 11 {
            y -= 1 << 12;
        }
        if z >= 1 << 25 {
            z -= 1 << 26;
        }
        Ok((x, y, z))
    }
}

impl<T: Buf + ?Sized> ProtoRead for T {}
