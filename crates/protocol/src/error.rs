//! Protocol error types.

use thiserror::Error;

/// Errors that can occur while encoding or decoding protocol data.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("unexpected end of buffer: needed {needed} bytes, had {available}")]
    Eof { needed: usize, available: usize },

    #[error("VarInt is too large (more than 5 bytes)")]
    VarIntTooLarge,

    #[error("VarLong is too large (more than 10 bytes)")]
    VarLongTooLarge,

    #[error("string is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("string length {length} exceeds maximum {max}")]
    StringTooLong { length: usize, max: usize },

    #[error("invalid boolean byte: {0}")]
    InvalidBool(u8),

    #[error("invalid enum discriminant {value} for {ty}")]
    InvalidEnum { ty: &'static str, value: i64 },

    #[error("packet payload too large: {0} bytes")]
    PacketTooLarge(usize),
}

/// Convenience result alias for protocol operations.
pub type Result<T> = std::result::Result<T, ProtocolError>;
