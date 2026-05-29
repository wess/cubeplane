//! Packet traits and connection-state model.

use bytes::{Buf, BytesMut};

use crate::error::Result;
use crate::read::ProtoRead;
use crate::write::ProtoWrite;

/// The connection state a [`crate::Connection`] is in. The same packet id can
/// mean different things in different states, so decoding is always
/// state-relative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Initial state right after the TCP connection opens.
    Handshaking,
    /// Server List Ping flow.
    Status,
    /// Authentication / compression negotiation.
    Login,
    /// Normal gameplay.
    Play,
}

/// A packet that can be written to the wire. The `ID` is the protocol packet id
/// in the *outgoing* (clientbound) direction for play/login/status states.
pub trait Encode {
    /// The clientbound packet id.
    const ID: i32;

    /// Serialize the packet body (everything after the id VarInt).
    fn encode(&self, buf: &mut BytesMut);

    /// Serialize the full packet body including its id. The frame length and
    /// optional compression header are added later by the codec.
    fn encode_with_id(&self, buf: &mut BytesMut) {
        buf.write_varint(Self::ID);
        self.encode(buf);
    }
}

/// A packet that can be parsed from a serverbound payload (id already stripped).
pub trait Decode: Sized {
    /// The serverbound packet id this type corresponds to.
    const ID: i32;

    /// Parse the packet body from `buf`, which is positioned just past the id.
    fn decode<B: Buf>(buf: &mut B) -> Result<Self>;
}

/// A raw, undecoded packet: its id plus the remaining payload bytes.
#[derive(Debug, Clone)]
pub struct RawPacket {
    pub id: i32,
    pub body: BytesMut,
}

impl RawPacket {
    /// Split a full (decompressed) packet frame into id + body.
    pub fn parse(mut frame: BytesMut) -> Result<RawPacket> {
        let id = frame.read_varint()?;
        Ok(RawPacket { id, body: frame })
    }

    /// Decode this raw packet into a concrete type, asserting the id matches.
    pub fn into_decoded<P: Decode>(mut self) -> Result<P> {
        debug_assert_eq!(self.id, P::ID, "decoding packet with mismatched id");
        P::decode(&mut self.body)
    }
}
