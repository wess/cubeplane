//! Async packet framing: VarInt length prefix plus optional zlib compression.
//!
//! A "frame payload" here is the packet id VarInt followed by the packet body —
//! i.e. everything inside the length prefix. Compression, when enabled, wraps
//! that payload per the vanilla scheme (uncompressed-length VarInt + zlib).

use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{BufMut, BytesMut};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::encryption::Cfb8;

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

/// An [`AsyncRead`] that AES-CFB8-decrypts bytes as they arrive.
pub struct EncryptedReader<R> {
    inner: R,
    cipher: Cfb8,
}

impl<R> EncryptedReader<R> {
    pub fn new(inner: R, cipher: Cfb8) -> Self {
        EncryptedReader { inner, cipher }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for EncryptedReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let after = buf.filled().len();
                if after > before {
                    self.cipher.decrypt(&mut buf.filled_mut()[before..after]);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// An [`AsyncWrite`] that AES-CFB8-encrypts bytes before writing. Encrypted
/// bytes are buffered so the cipher state always matches what's on the wire,
/// even across partial writes.
pub struct EncryptedWriter<W> {
    inner: W,
    cipher: Cfb8,
    pending: Vec<u8>,
    pos: usize,
}

impl<W> EncryptedWriter<W> {
    pub fn new(inner: W, cipher: Cfb8) -> Self {
        EncryptedWriter { inner, cipher, pending: Vec::new(), pos: 0 }
    }
}

impl<W: AsyncWrite + Unpin> EncryptedWriter<W> {
    /// Drain buffered ciphertext to the inner writer. Returns `Pending` if the
    /// inner writer is not ready and bytes remain.
    fn drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.pos < self.pending.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &self.pending[self.pos..]) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::WriteZero, "write zero")))
                }
                Poll::Ready(Ok(n)) => self.pos += n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.pending.clear();
        self.pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for EncryptedWriter<W> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        // Flush any buffered ciphertext before accepting more.
        if me.drain(cx)?.is_pending() {
            return Poll::Pending;
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // Encrypt the whole input once (advancing state) and buffer it.
        let mut enc = data.to_vec();
        me.cipher.encrypt(&mut enc);
        me.pending = enc;
        me.pos = 0;
        let _ = me.drain(cx)?; // best-effort immediate write
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.drain(cx)?.is_pending() {
            return Poll::Pending;
        }
        Pin::new(&mut me.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.drain(cx)?.is_pending() {
            return Poll::Pending;
        }
        Pin::new(&mut me.inner).poll_shutdown(cx)
    }
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
