//! NBT value model plus binary (de)serialization and a fluent builder.

use std::collections::BTreeMap;

use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

/// Tag type ids as defined by the NBT specification.
mod tag {
    pub const END: u8 = 0;
    pub const BYTE: u8 = 1;
    pub const SHORT: u8 = 2;
    pub const INT: u8 = 3;
    pub const LONG: u8 = 4;
    pub const FLOAT: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const BYTE_ARRAY: u8 = 7;
    pub const STRING: u8 = 8;
    pub const LIST: u8 = 9;
    pub const COMPOUND: u8 = 10;
    pub const INT_ARRAY: u8 = 11;
    pub const LONG_ARRAY: u8 = 12;
}

/// Errors raised while decoding NBT.
#[derive(Debug, Error)]
pub enum NbtError {
    #[error("unexpected end of NBT data")]
    Eof,
    #[error("unknown tag id {0}")]
    UnknownTag(u8),
    #[error("invalid UTF-8 in NBT string")]
    Utf8,
    #[error("expected a compound root tag, found id {0}")]
    NotCompound(u8),
    #[error("heterogeneous NBT list")]
    HeterogeneousList,
}

/// An NBT value. `Compound` preserves a stable key order via `BTreeMap`, which
/// keeps generated registry codecs byte-for-byte deterministic.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Value>),
    Compound(BTreeMap<String, Value>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Value {
    fn tag_id(&self) -> u8 {
        match self {
            Value::Byte(_) => tag::BYTE,
            Value::Short(_) => tag::SHORT,
            Value::Int(_) => tag::INT,
            Value::Long(_) => tag::LONG,
            Value::Float(_) => tag::FLOAT,
            Value::Double(_) => tag::DOUBLE,
            Value::ByteArray(_) => tag::BYTE_ARRAY,
            Value::String(_) => tag::STRING,
            Value::List(_) => tag::LIST,
            Value::Compound(_) => tag::COMPOUND,
            Value::IntArray(_) => tag::INT_ARRAY,
            Value::LongArray(_) => tag::LONG_ARRAY,
        }
    }

    fn write_payload(&self, buf: &mut BytesMut) {
        match self {
            Value::Byte(v) => buf.put_i8(*v),
            Value::Short(v) => buf.put_i16(*v),
            Value::Int(v) => buf.put_i32(*v),
            Value::Long(v) => buf.put_i64(*v),
            Value::Float(v) => buf.put_f32(*v),
            Value::Double(v) => buf.put_f64(*v),
            Value::ByteArray(v) => {
                buf.put_i32(v.len() as i32);
                buf.put_slice(v);
            }
            Value::String(s) => write_nbt_string(buf, s),
            Value::List(items) => {
                let elem = items.first().map(Value::tag_id).unwrap_or(tag::END);
                buf.put_u8(elem);
                buf.put_i32(items.len() as i32);
                for item in items {
                    item.write_payload(buf);
                }
            }
            Value::Compound(map) => {
                for (name, value) in map {
                    buf.put_u8(value.tag_id());
                    write_nbt_string(buf, name);
                    value.write_payload(buf);
                }
                buf.put_u8(tag::END);
            }
            Value::IntArray(v) => {
                buf.put_i32(v.len() as i32);
                for n in v {
                    buf.put_i32(*n);
                }
            }
            Value::LongArray(v) => {
                buf.put_i32(v.len() as i32);
                for n in v {
                    buf.put_i64(*n);
                }
            }
        }
    }

    fn read_payload<B: Buf>(buf: &mut B, tag_id: u8) -> Result<Value, NbtError> {
        let need = |buf: &B, n: usize| -> Result<(), NbtError> {
            if buf.remaining() < n {
                Err(NbtError::Eof)
            } else {
                Ok(())
            }
        };
        Ok(match tag_id {
            tag::BYTE => {
                need(buf, 1)?;
                Value::Byte(buf.get_i8())
            }
            tag::SHORT => {
                need(buf, 2)?;
                Value::Short(buf.get_i16())
            }
            tag::INT => {
                need(buf, 4)?;
                Value::Int(buf.get_i32())
            }
            tag::LONG => {
                need(buf, 8)?;
                Value::Long(buf.get_i64())
            }
            tag::FLOAT => {
                need(buf, 4)?;
                Value::Float(buf.get_f32())
            }
            tag::DOUBLE => {
                need(buf, 8)?;
                Value::Double(buf.get_f64())
            }
            tag::BYTE_ARRAY => {
                need(buf, 4)?;
                let len = buf.get_i32().max(0) as usize;
                need(buf, len)?;
                let mut v = vec![0u8; len];
                buf.copy_to_slice(&mut v);
                Value::ByteArray(v)
            }
            tag::STRING => Value::String(read_nbt_string(buf)?),
            tag::LIST => {
                need(buf, 5)?;
                let elem = buf.get_u8();
                let len = buf.get_i32().max(0) as usize;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(Value::read_payload(buf, elem)?);
                }
                Value::List(items)
            }
            tag::COMPOUND => {
                let mut map = BTreeMap::new();
                loop {
                    need(buf, 1)?;
                    let child = buf.get_u8();
                    if child == tag::END {
                        break;
                    }
                    let name = read_nbt_string(buf)?;
                    map.insert(name, Value::read_payload(buf, child)?);
                }
                Value::Compound(map)
            }
            tag::INT_ARRAY => {
                need(buf, 4)?;
                let len = buf.get_i32().max(0) as usize;
                need(buf, len * 4)?;
                let mut v = Vec::with_capacity(len);
                for _ in 0..len {
                    v.push(buf.get_i32());
                }
                Value::IntArray(v)
            }
            tag::LONG_ARRAY => {
                need(buf, 4)?;
                let len = buf.get_i32().max(0) as usize;
                need(buf, len * 8)?;
                let mut v = Vec::with_capacity(len);
                for _ in 0..len {
                    v.push(buf.get_i64());
                }
                Value::LongArray(v)
            }
            other => return Err(NbtError::UnknownTag(other)),
        })
    }
}

fn write_nbt_string(buf: &mut BytesMut, s: &str) {
    buf.put_u16(s.len() as u16);
    buf.put_slice(s.as_bytes());
}

fn read_nbt_string<B: Buf>(buf: &mut B) -> Result<String, NbtError> {
    if buf.remaining() < 2 {
        return Err(NbtError::Eof);
    }
    let len = buf.get_u16() as usize;
    if buf.remaining() < len {
        return Err(NbtError::Eof);
    }
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    String::from_utf8(bytes).map_err(|_| NbtError::Utf8)
}

/// Ergonomic wrapper around a root compound `Value` with chained `put_*` setters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Nbt {
    map: BTreeMap<String, Value>,
}

impl Nbt {
    /// Start an empty compound.
    pub fn compound() -> Self {
        Nbt { map: BTreeMap::new() }
    }

    pub fn put(mut self, key: impl Into<String>, value: Value) -> Self {
        self.map.insert(key.into(), value);
        self
    }

    pub fn put_byte(self, key: impl Into<String>, v: i8) -> Self {
        self.put(key, Value::Byte(v))
    }

    pub fn put_bool(self, key: impl Into<String>, v: bool) -> Self {
        self.put(key, Value::Byte(v as i8))
    }

    pub fn put_short(self, key: impl Into<String>, v: i16) -> Self {
        self.put(key, Value::Short(v))
    }

    pub fn put_int(self, key: impl Into<String>, v: i32) -> Self {
        self.put(key, Value::Int(v))
    }

    pub fn put_long(self, key: impl Into<String>, v: i64) -> Self {
        self.put(key, Value::Long(v))
    }

    pub fn put_float(self, key: impl Into<String>, v: f32) -> Self {
        self.put(key, Value::Float(v))
    }

    pub fn put_double(self, key: impl Into<String>, v: f64) -> Self {
        self.put(key, Value::Double(v))
    }

    pub fn put_string(self, key: impl Into<String>, v: impl Into<String>) -> Self {
        self.put(key, Value::String(v.into()))
    }

    pub fn put_compound(self, key: impl Into<String>, v: Nbt) -> Self {
        self.put(key, v.into_value())
    }

    pub fn put_list(self, key: impl Into<String>, items: Vec<Value>) -> Self {
        self.put(key, Value::List(items))
    }

    pub fn put_long_array(self, key: impl Into<String>, v: Vec<i64>) -> Self {
        self.put(key, Value::LongArray(v))
    }

    /// Convert into the underlying compound [`Value`].
    pub fn into_value(self) -> Value {
        Value::Compound(self.map)
    }

    /// Serialize as a *named* root tag: `[id][name-len][name][payload]`. The
    /// 1.20.1 protocol uses an empty name (`""`) for the dimension codec.
    pub fn to_bytes_named(self, name: &str) -> BytesMut {
        let value = self.into_value();
        let mut buf = BytesMut::new();
        buf.put_u8(tag::COMPOUND);
        write_nbt_string(&mut buf, name);
        value.write_payload(&mut buf);
        buf
    }

    /// Decode a named root compound from `buf`.
    pub fn from_bytes<B: Buf>(buf: &mut B) -> Result<Nbt, NbtError> {
        if buf.remaining() < 1 {
            return Err(NbtError::Eof);
        }
        let id = buf.get_u8();
        if id != tag::COMPOUND {
            return Err(NbtError::NotCompound(id));
        }
        let _name = read_nbt_string(buf)?;
        match Value::read_payload(buf, tag::COMPOUND)? {
            Value::Compound(map) => Ok(Nbt { map }),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_named() {
        let nbt = Nbt::compound()
            .put_string("name", "overworld")
            .put_int("id", 7)
            .put_bool("natural", true)
            .put_long_array("packed", vec![1, 2, 3])
            .put_compound("nested", Nbt::compound().put_double("scale", 0.5));
        let mut bytes = nbt.clone().to_bytes_named("root");
        let decoded = Nbt::from_bytes(&mut bytes).unwrap();
        assert_eq!(decoded, nbt);
    }

    #[test]
    fn list_of_compounds() {
        let list = Value::List(vec![
            Nbt::compound().put_int("id", 0).into_value(),
            Nbt::compound().put_int("id", 1).into_value(),
        ]);
        let nbt = Nbt::compound().put("value", list);
        let mut bytes = nbt.clone().to_bytes_named("");
        assert_eq!(Nbt::from_bytes(&mut bytes).unwrap(), nbt);
    }
}
