//! Async packet framing: VarInt length prefix plus optional zlib compression.
//!
//! A "frame payload" here is the packet id VarInt followed by the packet body —
//! i.e. everything inside the length prefix. Compression, when enabled, wraps
//! that payload per the vanilla scheme (uncompressed-length VarInt + zlib).

use std::io::{self, Read, Write};

use bytes::{BufMut, BytesMut};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use cubeplane_protocol::write::varint_len;
use cubeplane_protocol::{ProtoRead, ProtoWrite};

/// Compression disabled sentinel for a threshold.
pub const NO_COMPRESSION: i32 = -1;

/// Hard cap on a single inbound frame to avoid memory-exhaustion attacks.
const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Read one VarInt-length-prefixed frame and return its (decompressed) payload.
///
/// `threshold` follows the vanilla convention: `< 0` means compression is off.
pub async fn read_frame<R>(reader: &mut R, threshold: i32) -> io::Result<BytesMut>
where
    R: AsyncReadExt + Unpin,
{
    let frame_len = read_varint_async(reader).await? as usize;
    if frame_len == 0 {
        return Ok(BytesMut::new());
    }
    if frame_len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds maximum size",
        ));
    }

    let mut frame = vec![0u8; frame_len];
    reader.read_exact(&mut frame).await?;

    if threshold < 0 {
        return Ok(BytesMut::from(&frame[..]));
    }

    // Compressed framing: leading VarInt is the uncompressed size (0 = stored).
    let mut cursor = &frame[..];
    let data_len = cursor.read_varint().map_err(invalid)? as usize;
    if data_len == 0 {
        return Ok(BytesMut::from(cursor));
    }
    if data_len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "decompressed frame too large",
        ));
    }
    let mut out = Vec::with_capacity(data_len);
    ZlibDecoder::new(cursor).read_to_end(&mut out)?;
    Ok(BytesMut::from(&out[..]))
}

/// Encode a payload (id + body) into a complete, length-prefixed frame.
pub fn encode_frame(payload: &[u8], threshold: i32) -> BytesMut {
    let mut out = BytesMut::new();
    if threshold < 0 {
        out.write_varint(payload.len() as i32);
        out.put_slice(payload);
        return out;
    }

    if payload.len() as i32 >= threshold {
        // Compress: data length = uncompressed size, then zlib stream.
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("zlib write");
        let compressed = encoder.finish().expect("zlib finish");
        let inner_len = varint_len(payload.len() as i32) + compressed.len();
        out.write_varint(inner_len as i32);
        out.write_varint(payload.len() as i32);
        out.put_slice(&compressed);
    } else {
        // Below threshold: store uncompressed with a 0 data-length marker.
        let inner_len = varint_len(0) + payload.len();
        out.write_varint(inner_len as i32);
        out.write_varint(0);
        out.put_slice(payload);
    }
    out
}

/// Write a fully-encoded frame to the stream and flush it.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8], threshold: i32) -> io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let frame = encode_frame(payload, threshold);
    writer.write_all(&frame).await?;
    writer.flush().await
}

/// Read a single VarInt directly off an async stream, one byte at a time.
async fn read_varint_async<R>(reader: &mut R) -> io::Result<i32>
where
    R: AsyncReadExt + Unpin,
{
    let mut value: u32 = 0;
    let mut position = 0u32;
    loop {
        let byte = reader.read_u8().await?;
        value |= ((byte & 0x7F) as u32) << position;
        if byte & 0x80 == 0 {
            break;
        }
        position += 7;
        if position >= 32 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "VarInt too large"));
        }
    }
    Ok(value as i32)
}

fn invalid<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn roundtrip_uncompressed() {
        let payload = b"\x28hello world payload";
        let frame = encode_frame(payload, NO_COMPRESSION);
        let mut cur = Cursor::new(frame.to_vec());
        let got = read_frame(&mut cur, NO_COMPRESSION).await.unwrap();
        assert_eq!(&got[..], payload);
    }

    #[tokio::test]
    async fn roundtrip_compressed_above_threshold() {
        let payload = vec![0x55u8; 1000];
        let frame = encode_frame(&payload, 256);
        let mut cur = Cursor::new(frame.to_vec());
        let got = read_frame(&mut cur, 256).await.unwrap();
        assert_eq!(&got[..], &payload[..]);
    }

    #[tokio::test]
    async fn roundtrip_compressed_below_threshold_is_stored() {
        let payload = vec![0x07u8; 10];
        let frame = encode_frame(&payload, 256);
        let mut cur = Cursor::new(frame.to_vec());
        let got = read_frame(&mut cur, 256).await.unwrap();
        assert_eq!(&got[..], &payload[..]);
    }
}
