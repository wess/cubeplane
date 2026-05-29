# Architecture

cubeplane is a Cargo workspace of small, single-responsibility crates plus a
Bun/TypeScript admin panel. Data flows in one direction at each boundary, which
keeps the pieces independently testable.

```
                 ┌─────────────────────────────────────────────┐
   Minecraft     │                cubeplane engine              │
   client  ◀────▶│  TCP  ──▶ codec ──▶ connection state machine │
                 │                         │      │             │
                 │              world ◀────┘      └────▶ players │
                 │                │                       table  │
                 │            generators                        │
                 │                                              │
                 │   mod runtime (QuickJS)  ◀── events           │
                 │        │  └── actions ──────────▶ engine      │
                 │   control API (HTTP/WS)                       │
                 └───────────────────┬──────────────────────────┘
                                     │ http (server-side)
                          ┌──────────▼───────────┐
                          │  Atlas admin panel    │ ◀──▶ browser
                          │  (Bun + @atlas/server)│
                          └──────────────────────┘
```

## Crates

### `cubeplane-protocol`
Runtime-agnostic wire primitives for protocol 763: VarInt/VarLong, the
`ProtoRead`/`ProtoWrite` extension traits over `bytes::Buf`/`BufMut`,
`Encode`/`Decode` packet traits, and the `State` enum (handshaking → status /
login → play). No async, no I/O.

### `cubeplane-nbt`
A complete Named Binary Tag implementation (all 12 tag types) with binary
(de)serialization and a fluent `Nbt` builder. Used for the Login registry codec
and chunk heightmaps. `Compound` uses a `BTreeMap` so generated NBT is
deterministic.

### `cubeplane-world`
- `block`: curated 1.20.1 block-state id registry with name lookup.
- `chunk`: 16×384×16 columns of paletted sections, `MOTION_BLOCKING`
  heightmaps, full-skylight light data, and the exact paletted-container wire
  encoding the client expects.
- `gen`: a `Generator` trait with superflat and value-noise terrain
  implementations (no external dependencies).
- `World`: lazy chunk generation/caching and global block get/set.

### `cubeplane-mods`
The QuickJS mod runtime. The context lives on its own OS thread; the engine
sends `ModEvent`s in and receives `ModAction`s out over channels. JS state
(handlers, commands) lives on the JS heap, so Rust just exchanges JSON across
the boundary — no lifetime gymnastics. See [MODDING.md](./MODDING.md).

### `cubeplane-server`
The engine proper:
- `codec`: async VarInt framing with zlib compression.
- `ids` / `clientbound` / `serverbound`: packet ids and (de)serializers.
- `registry`: the dimension/biome/chat-type Login codec.
- `connection`: the per-connection handshake → status/login → play lifecycle,
  chunk streaming, movement relay, chat, commands, building, combat, fall/void
  damage and respawn.
- `entity`: mob kinds (type ids, stats, hostility), `Mob`, dropped `ItemEntity`
  and `Projectile` state.
- `mobs`: spawning (night-gated hostiles) plus the AI tick — gravity,
  wandering, hostile chasing, melee, creeper explosions, skeleton arrows, loot
  drops, XP and despawning.
- `combat`: shared damage/heal/death/XP/hunger helpers used by fall, mob,
  projectile and explosion damage, with armor mitigation and death drops.
- `item` / `inventory`: the 1.20.1 item registry and the 46-slot player
  inventory (stacks, armor, add/merge).
- `drops`: dropped-item and projectile physics, pickup and hit detection.
- `commands`: built-in and op-gated slash commands.
- `persistence`: world block-edit delta and per-player JSON save/load.
- `state`: the `Arc<Shared>` everything hangs off — config, world, player
  table, mob table, broadcast helpers, the mod bridge.
- `control`: the HTTP + WebSocket admin API (axum).
- `lib`: `run()` boots the world, mods, the 20 TPS game loop, the control API
  and the accept loop.

### `cubeplane`
The binary: config loading, tracing setup, banner, graceful Ctrl-C shutdown.

## Concurrency model

- One Tokio task per connection. Reads happen in that task; writes go through an
  unbounded channel to a dedicated per-connection writer task, so broadcasts
  from other tasks never block a reader.
- Shared mutable state is a single `Arc<Shared>`. The world is behind a
  `Mutex` (held only for brief, synchronous chunk/block operations — never
  across an `.await`). The player table is an `RwLock<HashMap>`.
- The mod runtime is single-threaded QuickJS on its own OS thread, fed by
  channels. Mod actions are applied by a dedicated async task.
- A 20 TPS game loop drives world time, keep-alives and per-second mod ticks.

## Testing

- Unit tests in each crate (VarInt/NBT roundtrips, chunk packing, generation,
  config).
- An end-to-end mod test drives a real QuickJS dispatch.
- `crates/server/tests/protocol_e2e.rs` boots the real server on an ephemeral
  port and speaks raw TCP to verify the status ping and the login → play join
  sequence (Join Game + Chunk Data) byte-for-byte.
