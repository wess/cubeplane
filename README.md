<div align="center">

```
   ___      _           _
  / __\   _| |__   ___ | | __ _ _ __   ___
 / / | | | | '_ \ / _ \| |/ _` | '_ \ / _ \
/ /__| |_| | |_) |  __/| | (_| | | | |  __/
\____/\__,_|_.__/ \___||_|\__,_|_| |_|\___|
```

**A Minecraft server engine in Rust — with JavaScript mods and an Atlas admin panel.**

Minecraft Java Edition · **1.20.1** · protocol **763**

</div>

---

cubeplane is a from-scratch Minecraft server built around three cleanly
separated layers:

| Layer | Tech | Role |
| --- | --- | --- |
| **Engine** | Rust (Tokio) | The server: protocol, world, chunks, players, game loop |
| **Mods** | JavaScript via [QuickJS](https://bellard.org/quickjs/) ([`rquickjs`](https://github.com/DelSkayn/rquickjs)) | Drop-in `.js` mods that react to events and act on the world |
| **Control panel** | TypeScript + [Bun](https://bun.sh) on [Atlas](https://github.com/wess/atlas) | Web admin UI that drives the engine's control API |

The Rust side follows Atlas's "small composable pieces" philosophy: a Cargo
workspace of focused crates that snap together.

## Features

- **Real client compatibility** — a vanilla 1.20.1 client connects, logs in,
  and walks around a lit, generated world.
- **Server List Ping** with live MOTD and player count.
- **Login** with offline-mode UUIDs, packet **compression**, and optional
  **online-mode encryption** (RSA key exchange + AES-CFB8 stream).
- **World generation** — superflat or smooth value-noise terrain (no external
  noise crates), with paletted chunk sections and heightmaps.
- **Real lighting** — skylight and block-light flood-fill (lit caves, shadows,
  and glowing blocks), packed per section.
- **Full block registry** — all 1003 1.20.1 blocks resolvable by name for
  `/setblock`, `/give` and mods, with correct light opacity/emission.
- **Multiplayer** — players see each other move, chat, and build in real time.
- **Mobs & AI** — all ~80 living entity types spawn around players (hostiles at
  night), wander with gravity, and hostile mobs chase, melee, shoot arrows
  (skeletons) or explode (creepers), dropping loot and XP.
- **Combat & vitals** — health/food HUD, melee both ways (players hit mobs,
  mobs hit players), knockback, hurt and death animations, natural health
  regeneration.
- **Death & respawn** — fall and void damage, the death screen, and a full
  respawn that rebuilds the player's world view.
- **Items & inventory** — a real 46-slot inventory, stacks, creative item
  edits, drops on the ground with pickup, food you can eat, weapons whose
  damage matters, and armor that reduces it.
- **Survival** — health, hunger drain, fall/void/explosion/arrow damage,
  natural regeneration, XP from kills, death drops (or `keepInventory`).
- **Persistence** — block edits and player data (position, vitals, inventory,
  XP) are saved to disk and restored across restarts.
- **Building** — break and place blocks from your hotbar with proper
  block-change acknowledgement; survival breaks drop items.
- **Chests** — place a chest, right-click to open a 27-slot container, store
  items; contents persist and spill when the chest is broken.
- **World simulation** — crops grow, saplings become trees, grass spreads,
  water and lava flow, fire spreads and burns out.
- **Redstone** — wire carries power from blocks/torches/levers and lights
  redstone lamps; flip a lever to switch a circuit.
- **Vehicles** — spawn boats/minecarts, right-click to ride, jump to dismount.
- **Villager trading** — right-click a villager to open a trade window.
- **Persistence backends** — `delta` (edits over the generator) or `region`
  (full chunk columns, Anvil-style).
- **Commands** — `/help /list /pos /tp /xp` for everyone, plus op-gated
  `/gamemode /give /time /weather /summon /effect /heal /kill /clear`, with
  client-side tab-completion and any mod commands.
- **JavaScript mods** — a sandboxed QuickJS runtime with an event/action API.
- **Control API** — HTTP + WebSocket endpoints for status, players, broadcast,
  kick and set-block.
- **Atlas admin panel** — a live web dashboard for all of the above.

## Quick start

### 1. Run the engine

```bash
cargo run --release            # reads ./cubeplane.toml
```

You'll see the banner, mods loading, and:

```
cubeplane listening on 0.0.0.0:25565 — Minecraft 1.20.1 (protocol 763)
control API listening on http://127.0.0.1:8080
```

### 2. Connect

Open Minecraft **Java Edition 1.20.1**, add a server pointing at
`localhost`, and join. You'll spawn in a generated world with the example mods
greeting you.

### 3. Launch the admin panel

```bash
cd admin
bun install        # pulls Atlas from GitHub
bun run dev        # http://localhost:3000
```

## Configuration

All settings live in [`cubeplane.toml`](./cubeplane.toml) and are optional:

```toml
[server]
port = 25565
gamemode = "creative"        # survival | creative | adventure | spectator
view_distance = 8
compression_threshold = 256  # -1 to disable

[world]
generator = "terrain"        # terrain | flat
seed = 24317

[mods]
enabled = true
dir = "mods"

[control]
enabled = true
port = 8080
# token = "secret"           # require Authorization: Bearer <token>
```

## Mods

Drop a `.js` file in `mods/` and restart. Mods register handlers and act on the
world:

```js
cubeplane.on("player_join", (e) => {
  cubeplane.broadcast(e.player + " joined the cubeplane!");
});

cubeplane.command("tower", (ctx) => {
  for (let i = 0; i < 10; i++) cubeplane.setBlock(0, 4 + i, 0, "stone");
});
```

See [docs/MODDING.md](./docs/MODDING.md) for the full API and the examples in
[`mods/`](./mods).

## Workspace layout

```
cubeplane/
├── crates/
│   ├── protocol/   # VarInt/packet primitives (protocol 763)
│   ├── nbt/        # Named Binary Tag reader/writer/builder
│   ├── world/      # blocks, paletted chunks, generation
│   ├── mods/       # QuickJS mod runtime (rquickjs)
│   ├── server/     # networking, state machine, game loop, control API
│   └── cubeplane/  # the binary
├── mods/           # example JavaScript mods
├── admin/          # Atlas (Bun/TS) control panel
├── docs/           # architecture, modding & protocol references
└── cubeplane.toml  # configuration
```

More in [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) and
[docs/PROTOCOL.md](./docs/PROTOCOL.md).

## Development

```bash
cargo test          # all Rust crates incl. on-the-wire protocol tests
cargo build         # debug build
cd admin && bunx tsc --noEmit   # typecheck the panel
```

## License

MIT © wess
