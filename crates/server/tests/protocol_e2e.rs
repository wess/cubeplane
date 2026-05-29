//! End-to-end protocol tests: boot the real server on an ephemeral port and
//! drive it over TCP exactly as a vanilla client would for the status ping and
//! the login → play handshake.

use std::time::Duration;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use cubeplane_protocol::{ProtoRead, ProtoWrite, PROTOCOL_VERSION};
use cubeplane_server::Config;

async fn read_varint<R: AsyncReadExt + Unpin>(r: &mut R) -> i32 {
    let mut value: u32 = 0;
    let mut pos = 0;
    loop {
        let byte = r.read_u8().await.unwrap();
        value |= ((byte & 0x7F) as u32) << pos;
        if byte & 0x80 == 0 {
            break;
        }
        pos += 7;
    }
    value as i32
}

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> (i32, BytesMut) {
    let len = read_varint(r).await as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.unwrap();
    let mut body = BytesMut::from(&buf[..]);
    let id = body.read_varint().unwrap();
    (id, body)
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, payload: &[u8]) {
    let mut header = BytesMut::new();
    header.write_varint(payload.len() as i32);
    w.write_all(&header).await.unwrap();
    w.write_all(payload).await.unwrap();
    w.flush().await.unwrap();
}

fn handshake(next_state: i32) -> BytesMut {
    let mut b = BytesMut::new();
    b.write_varint(0x00);
    b.write_varint(PROTOCOL_VERSION);
    b.write_string("127.0.0.1");
    b.write_u16(25565);
    b.write_varint(next_state);
    b
}

async fn start_server() -> u16 {
    // Grab a free port, then hand it to the server.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut config = Config::default();
    config.server.host = "127.0.0.1".into();
    config.server.port = port;
    config.server.compression_threshold = -1; // keep the test wire-simple
    config.server.view_distance = 2;
    config.control.enabled = false;
    config.mods.enabled = false;
    config.world.generator = "flat".into();

    tokio::spawn(async move {
        let _ = cubeplane_server::run(config).await;
    });
    // Give the listener a moment to bind.
    tokio::time::sleep(Duration::from_millis(300)).await;
    port
}

#[tokio::test]
async fn status_ping_roundtrip() {
    let port = start_server().await;
    let mut conn = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

    write_frame(&mut conn, &handshake(1)).await;

    // Status request (empty body).
    let mut req = BytesMut::new();
    req.write_varint(0x00);
    write_frame(&mut conn, &req).await;

    let (id, mut body) = read_frame(&mut conn).await;
    assert_eq!(id, 0x00, "status response id");
    let json = body.read_string().unwrap();
    assert!(json.contains("1.20.1"), "status json: {json}");
    assert!(json.contains("\"max\""), "status json: {json}");

    // Ping/pong.
    let mut ping = BytesMut::new();
    ping.write_varint(0x01);
    ping.write_i64(0xCAFEBABE);
    write_frame(&mut conn, &ping).await;

    let (id, mut body) = read_frame(&mut conn).await;
    assert_eq!(id, 0x01, "pong id");
    assert_eq!(body.read_i64().unwrap(), 0xCAFEBABE);
}

#[tokio::test]
async fn login_then_play_join_sequence() {
    let port = start_server().await;
    let mut conn = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

    write_frame(&mut conn, &handshake(2)).await;

    // Login Start: username + absent optional UUID.
    let mut start = BytesMut::new();
    start.write_varint(0x00);
    start.write_string("Tester");
    start.write_bool(false);
    write_frame(&mut conn, &start).await;

    // Expect Login Success (0x02) with our username echoed back.
    let (id, mut body) = read_frame(&mut conn).await;
    assert_eq!(id, 0x02, "login success id");
    let _uuid = body.read_uuid().unwrap();
    assert_eq!(body.read_string().unwrap(), "Tester");

    // The play phase should then deliver Join Game (0x28) and at least one
    // Chunk Data (0x24) packet within a handful of frames.
    let mut saw_join = false;
    let mut saw_chunk = false;
    let mut saw_health = false;
    for _ in 0..60 {
        let (id, _body) = read_frame(&mut conn).await;
        if id == 0x28 {
            saw_join = true;
        }
        if id == 0x24 {
            saw_chunk = true;
        }
        if id == 0x57 {
            saw_health = true;
        }
        if saw_join && saw_chunk && saw_health {
            break;
        }
    }
    assert!(saw_join, "did not receive Join Game packet");
    assert!(saw_chunk, "did not receive Chunk Data packet");
    assert!(saw_health, "did not receive Set Health packet");
}
