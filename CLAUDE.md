# CLAUDE.md — cubeplane working notes for AI sessions

This file is auto-loaded at the start of a Claude Code session. It exists so a
fresh session can pick up cubeplane with full context. Read it top to bottom.

## What cubeplane is

A from-scratch **Minecraft Java server engine in Rust**, native protocol
**763 / Minecraft 1.20.1**, with three layers:

| Layer | Tech | Role |
| --- | --- | --- |
| Engine | Rust (Tokio) | protocol, world/chunks/lighting, players, game loop, all gameplay |
| Mods | JavaScript via QuickJS (`rquickjs`) | drop-in `.js` mods reacting to events |
| Control panel | TypeScript + Bun on Atlas (`admin/`) | web admin UI over the engine's control API |

Headline "super feature": **AI villagers** — an optional toggle to plug in
Claude / OpenAI / Ollama so villagers role-play their profession (see
`docs/AI_VILLAGERS.md`, `crates/server/src/ai.rs`).

## Workspace layout

```
crates/
  protocol/  VarInt/packet framing, ProtoRead/ProtoWrite, version registry (lib.rs)
  nbt/       NBT read/write (Nbt builder, Value enum)
  world/     chunks, blocks (generated blocks_table.rs), lighting, generators
  server/    everything else — the bulk of the code (see key files below)
  cubeplane/ the binary
admin/        Atlas (Bun/TS) admin panel
mods/         example JS mods
docs/         ARCHITECTURE.md, PROTOCOL.md, MULTIVERSION.md, AI_VILLAGERS.md, MODDING.md
```

## Build / test / lint — the standard workflow

Every change must pass all three before commit; the repo has been kept at **zero
clippy warnings** and **all tests green** throughout:

```bash
cargo build
cargo clippy --workspace --all-targets    # must be 0 warnings
cargo test                                 # must be 0 failed (currently 94 pass)
```

Commit only when the user asks. Branch: `claude/cubeplane-minecraft-server-JOrA3`.
Push with `git push -u origin <branch>`. Do NOT open PRs unless asked.

## Current state (what's implemented & tested)

**1.20.1 survival server is feature-complete:** per-dimension worlds
(overworld/nether/end), real chunk + lighting, online-mode encryption, paletted
chunks, registry codec NBT. Gameplay: mobs (80 types) with health/death/respawn,
**breeding** (species foods, baby growth, cooldown), obstacle-avoidance mob
movement, combat/XP, hunger, status effects, potions/brewing, **full redstone**
(wire, repeaters, comparators, observers, pistons, plates, buttons, doors/lamps),
furnaces/anvil/crafting-table windows, **villager economy** (10 professions,
trade uses/restock, **XP leveling 1→5**), bone meal, shears, buckets/milk,
**statistics**, **advancements** (toasts), signs, vehicles, persistence,
QuickJS mods, Atlas panel, AI villagers.

**Multi-version:** a bidirectional translation engine (`crates/server/src/version.rs`)
serves **nine protocol versions across eleven MC releases (1.18 → 1.20.4)**.
See `docs/MULTIVERSION.md` for the full pipeline. Each version has an
integration test in `crates/server/src/connection.rs` (search `version_7`).

## Multi-version: the short version

- `SUPPORTED_PROTOCOLS` in `crates/protocol/src/lib.rs` lists connectable
  versions: 757, 758, 759, 760, 761, 762, 763 (native), 764, 765.
- Translation is centralized in `crates/server/src/version.rs`:
  - `translate_clientbound` (in the writer task) and `translate_serverbound`
    (in `play_loop`) rewrite the leading packet-id varint per version, then
    `rewrite_clientbound_body` applies per-version body rewriters.
  - Keystone transforms built: NBT named→anonymous (1.20.2),
    JSON-chat→NBT-text (1.20.3+), `trustEdges` chunk/light inserts (1.19.x),
    `player_info` bitmask→enum + `player_remove` fold (1.19.x/1.18), login
    dimension-as-NBT (1.18), `system_chat`→old `chat` packet (1.18).
- 1.20.2+ also needs the **Configuration phase** — handled in
  `finish_login` / `configuration_phase` in `connection.rs`.
- All id maps and body rewriters were generated from **authoritative
  PrismarineJS `minecraft-data`** via `curl`+`jq` (Bash has network access).

## Remaining work toward "full ecosystem parity" (1.8 → 1.21.1)

Not done, each blocked by something concrete (verified empirically — see
`docs/MULTIVERSION.md` for evidence and exact next steps):

1. **1.20.5–1.21.1 (766/767)** — geometry-compatible, but the config phase
   reworked registries into per-registry packets requiring the **full dynamic
   registry codec** (dimension_type, worldgen/biome, damage_type, chat_type,
   trim_pattern/material, banner_pattern, wolf/painting_variant). The data is
   sourceable from **misode/mcmeta** (`refs/tags/1.20.5-data/...`), but: (a) it's
   ~250 entry files, and (b) **JSON→NBT type fidelity** matters — the client
   validates exact NBT types (e.g. biome `temperature` must be `float`, not the
   `double`/`int` a generic converter emits). Needs per-field type tables and a
   real 1.20.5 client to validate. Don't ship a guessed codec — a wrong field
   disconnects the client.
2. **1.16–1.17 (754–756)** — different core world geometry (`0..256` vs
   cubeplane's `-64..320`, `MIN_Y`/`WORLD_HEIGHT` in `crates/world/src/chunk.rs`)
   and the pre-1.18 bitmask chunk format. A core-world change, not a translation
   adapter.
3. **1.8 (protocol 47)** — pre-flattening: all block *states* would remap to
   numeric id+metadata, with a different chunk and entity-metadata format. A
   ground-up second protocol backend (the scope of ViaRewind).

## Working principles established in this project

- **Never fabricate protocol/registry data.** Source it from `minecraft-data`
  (`pc/<ver>/protocol.json`) or `misode/mcmeta`, via `curl`+`jq`. Fabricating a
  packet id or registry field silently corrupts the stream and disconnects
  clients — worse than a clean "unsupported version" message. This standard
  caught two real bugs (a 763 `advancements` packet missing `criteria` +
  `sendsTelemetryData`, and the `entity_effect` id).
- **Verify every version with an integration test** that drives a simulated
  client through handshake→login→(config)→play and *parses the server's actual
  output* (see the `version_7xx_*` tests). Structural verification only — a real
  client of that version is the gold standard we lack in-sandbox.
- Adding an adjacent version is mechanical and cheap when it shares an era
  (e.g. 757 reused 758 wholesale; 759 reused 760). See `docs/MULTIVERSION.md`
  for the step-by-step recipe.
