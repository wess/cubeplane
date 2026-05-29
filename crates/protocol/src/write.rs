//! Writing primitives onto the Minecraft wire format.
//!
//! The [`ProtoWrite`] trait mirrors [`ProtoRead`](crate::read::ProtoRead) but
//! for any [`bytes::BufMut`]. Writes are infallible at this layer — capacity is
//! grown as needed by `BytesMut` — so methods return `()`.

use bytes::BufMut;
use uuid::Uuid;

/// Extension trait adding Minecraft protocol writes to any [`bytes::BufMut`].
pub trait ProtoWrite: BufMut {
    fn write_u8(&mut self, v: u8) {
        self.put_u8(v);
    }

    fn write_i8(&mut self, v: i8) {
        self.put_i8(v);
    }

    fn write_bool(&mut self, v: bool) {
        self.put_u8(v as u8);
    }

    fn write_u16(&mut self, v: u16) {
        self.put_u16(v);
    }

    fn write_i16(&mut self, v: i16) {
        self.put_i16(v);
    }

    fn write_i32(&mut self, v: i32) {
        self.put_i32(v);
    }

    fn write_i64(&mut self, v: i64) {
        self.put_i64(v);
    }

    fn write_u64(&mut self, v: u64) {
        self.put_u64(v);
    }

    fn write_f32(&mut self, v: f32) {
        self.put_f32(v);
    }

    fn write_f64(&mut self, v: f64) {
        self.put_f64(v);
    }

    /// Write a variable-length signed 32-bit integer.
    fn write_varint(&mut self, value: i32) {
        let mut val = value as u32;
        loop {
            if val & !0x7F == 0 {
                self.put_u8(val as u8);
                break;
            }
            self.put_u8(((val & 0x7F) | 0x80) as u8);
            val >>= 7;
        }
    }

    /// Write a variable-length signed 64-bit integer.
    fn write_varlong(&mut self, value: i64) {
        let mut val = value as u64;
        loop {
            if val & !0x7F == 0 {
                self.put_u8(val as u8);
                break;
            }
            self.put_u8(((val & 0x7F) | 0x80) as u8);
            val >>= 7;
        }
    }

    /// Write a VarInt-length-prefixed UTF-8 string.
    fn write_string(&mut self, s: &str) {
        self.write_varint(s.len() as i32);
        self.put_slice(s.as_bytes());
    }

    /// Write raw bytes verbatim.
    fn write_bytes(&mut self, b: &[u8]) {
        self.put_slice(b);
    }

    /// Write a 128-bit UUID (big-endian).
    fn write_uuid(&mut self, id: Uuid) {
        self.put_slice(id.as_bytes());
    }

    /// Write a packed block position (26/12/26 bits).
    fn write_position(&mut self, x: i32, y: i32, z: i32) {
        let val = ((x as i64 & 0x3FF_FFFF) << 38)
            | ((z as i64 & 0x3FF_FFFF) << 12)
            | (y as i64 & 0xFFF);
        self.put_i64(val);
    }
}

impl<T: BufMut + ?Sized> ProtoWrite for T {}

/// Number of bytes a VarInt of this value will occupy on the wire.
pub fn varint_len(value: i32) -> usize {
    let mut val = value as u32;
    let mut len = 1;
    while val & !0x7F != 0 {
        val >>= 7;
        len += 1;
    }
    len
}
