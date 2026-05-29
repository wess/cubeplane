# Protocol reference (763 / 1.20.1)

cubeplane targets **Minecraft Java Edition 1.20.1**, protocol version **763**.
Packet ids below were taken from the PrismarineJS `minecraft-data` definition
for `pc/1.20` and are the ones cubeplane reads or writes. They live in
[`crates/server/src/ids.rs`](../crates/server/src/ids.rs).

## Connection flow

```
Handshake ──(next state)──▶ Status   (server list ping)
                          └▶ Login   ──▶ Play
```

In 1.20.1 the client enters **Play** immediately after Login Success (the
separate Configuration phase arrives in 1.20.2). cubeplane runs in offline mode
(no encryption); compression is negotiated with Set Compression.

## Handshake (serverbound)

| Id | Packet |
| --- | --- |
| `0x00` | Set Protocol (handshake) |

## Status

| Dir | Id | Packet |
| --- | --- | --- |
| C→S | `0x00` | Status Request |
| C→S | `0x01` | Ping Request |
| S→C | `0x00` | Status Response |
| S→C | `0x01` | Pong Response |

## Login

| Dir | Id | Packet |
| --- | --- | --- |
| C→S | `0x00` | Login Start |
| S→C | `0x00` | Disconnect |
| S→C | `0x02` | Login Success |
| S→C | `0x03` | Set Compression |

## Play — clientbound (implemented)

| Id | Packet |
| --- | --- |
| `0x03` | Spawn Player |
| `0x06` | Acknowledge Block Change |
| `0x0a` | Block Update |
| `0x1a` | Disconnect |
| `0x1e` | Unload Chunk |
| `0x1f` | Game Event |
| `0x23` | Keep Alive |
| `0x24` | Chunk Data and Update Light |
| `0x28` | Login (Join Game) |
| `0x34` | Player Abilities |
| `0x39` | Player Info Remove |
| `0x3a` | Player Info Update |
| `0x3c` | Synchronize Player Position |
| `0x3e` | Remove Entities |
| `0x42` | Set Head Rotation |
| `0x4d` | Set Held Item |
| `0x4e` | Set Center Chunk |
| `0x50` | Set Default Spawn Position |
| `0x5e` | Update Time |
| `0x64` | System Chat Message |
| `0x68` | Teleport Entity |

## Play — serverbound (handled)

| Id | Packet |
| --- | --- |
| `0x00` | Confirm Teleport |
| `0x04` | Chat Command |
| `0x05` | Chat Message |
| `0x07` | Client Command |
| `0x08` | Client Information |
| `0x12` | Keep Alive |
| `0x14` | Set Player Position |
| `0x15` | Set Player Position and Rotation |
| `0x16` | Set Player Rotation |
| `0x17` | Set Player On Ground |
| `0x1d` | Player Action (dig) |
| `0x28` | Set Held Item |
| `0x2f` | Swing Arm |
| `0x31` | Use Item On (place) |
| `0x32` | Use Item |

## Chunk format notes

- Columns are 24 sections tall (`min_y = -64`, `height = 384`).
- Each section is a **paletted container**: single-valued (0 bits), indirect
  (4–8 bits with a palette) or direct (15-bit global ids), packed into `i64`s
  **without** spanning long boundaries (the post-1.16 compact format).
- Biomes use a single-valued container per section.
- `MOTION_BLOCKING` / `WORLD_SURFACE` heightmaps are 9-bit packed.
- cubeplane floods full skylight (level 15) everywhere for a bright world.

## Registry codec

The Login packet ships a registry codec NBT with `minecraft:dimension_type`
(a complete overworld entry), `minecraft:worldgen/biome` (a `plains` biome at
id 1) and all seven vanilla `minecraft:chat_type` entries — see
[`crates/server/src/registry.rs`](../crates/server/src/registry.rs).
